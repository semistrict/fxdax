//! A DAX window's second stage in host memory that a hypervisor follows, such as the memory
//! backing a KVM memory slot, whose faults userfaultfd reports.
//!
//! KVM maps a guest's memory from the host memory backing its memory slot, and follows changes to
//! the host's mappings. On arm64 it reports a fault it cannot resolve there only as `EFAULT`, with
//! no address, so the memory must never fault that way. What the window has not mapped, or maps
//! read-only, is instead registered with userfaultfd, which stops the faulting thread (a vCPU, or
//! KVM on its behalf) and reports the address:
//!
//! - An unmapped page is anonymous memory registered for missing faults.
//! - A mapped page maps the window's page again, read-write, registered for write-protect faults,
//!   and is write-protected while the window maps it read-only.
//!
//! A fault thread passes the faults to the window, which maps the page (and so maps it here), and
//! wakes the faulting thread. A fault the window cannot resolve is fatal, as it is to a Zircon
//! guest: the faulting thread cannot go on.
//!
//! The userfaultfd must handle faults taken in the kernel, such as KVM's. That needs access to
//! `/dev/userfaultfd`, or the `vm.unprivileged_userfaultfd` sysctl.

use super::DaxWindow;
use crate::errno::{Error, Result, linux};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::thread::JoinHandle;
use zircon::hypervisor::aspace::SecondStage;

// From `include/uapi/linux/userfaultfd.h`.
const UFFD_API: u64 = 0xaa;
const UFFD_EVENT_PAGEFAULT: u8 = 0x12;
const UFFD_PAGEFAULT_FLAG_WRITE: u64 = 1 << 0;
const UFFD_PAGEFAULT_FLAG_WP: u64 = 1 << 1;
const UFFD_FEATURE_WP_HUGETLBFS_SHMEM: u64 = 1 << 12;
const UFFDIO_REGISTER_MODE_MISSING: u64 = 1 << 0;
const UFFDIO_REGISTER_MODE_WP: u64 = 1 << 1;
const UFFDIO_WRITEPROTECT_MODE_WP: u64 = 1 << 0;

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | (0xaa << 8) | nr
}
const IOC_NONE: u64 = 0;
const IOC_READ: u64 = 2;
const IOC_READ_WRITE: u64 = 3;
const USERFAULTFD_IOC_NEW: u64 = ioc(IOC_NONE, 0x00, 0);
const UFFDIO_API: u64 = ioc(IOC_READ_WRITE, 0x3f, size_of::<UffdioApi>());
const UFFDIO_REGISTER: u64 = ioc(IOC_READ_WRITE, 0x00, size_of::<UffdioRegister>());
const UFFDIO_WAKE: u64 = ioc(IOC_READ, 0x02, size_of::<UffdioRange>());
const UFFDIO_WRITEPROTECT: u64 = ioc(IOC_READ_WRITE, 0x06, size_of::<UffdioWriteprotect>());

#[repr(C)]
struct UffdioApi {
    api: u64,
    features: u64,
    ioctls: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UffdioRange {
    start: u64,
    len: u64,
}

#[repr(C)]
struct UffdioRegister {
    range: UffdioRange,
    mode: u64,
    ioctls: u64,
}

#[repr(C)]
struct UffdioWriteprotect {
    range: UffdioRange,
    mode: u64,
}

/// `struct uffd_msg`, for page faults.
#[repr(C)]
struct UffdMsg {
    event: u8,
    reserved1: u8,
    reserved2: u16,
    reserved3: u32,
    flags: u64,
    address: u64,
    ptid: u32,
    reserved4: u32,
}

const _: () = assert!(size_of::<UffdMsg>() == 32);

/// A DAX window's second stage in host memory that a hypervisor follows. See the module docs.
pub struct UffdSecondStage {
    base: usize,
    size: usize,
    uffd: OwnedFd,
}

impl UffdSecondStage {
    /// Takes over the `size` bytes of host memory at `base`, replacing them with memory whose
    /// faults userfaultfd reports, as nothing is mapped yet.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping that nothing else in the process uses.
    pub unsafe fn new(base: usize, size: usize) -> Result<Arc<Self>> {
        let uffd = open_userfaultfd()?;
        let mut api =
            UffdioApi { api: UFFD_API, features: UFFD_FEATURE_WP_HUGETLBFS_SHMEM, ioctls: 0 };
        ioctl(&uffd, UFFDIO_API, &mut api).map_err(os_error)?;
        let stage = Arc::new(Self { base, size, uffd });
        stage.unmap(0, size);
        Ok(stage)
    }

    /// Serves the faults on the memory with `window`, on a thread of its own, until the returned
    /// thread is dropped.
    pub fn serve(self: &Arc<Self>, window: Arc<DaxWindow>) -> Result<FaultThread> {
        assert_eq!(window.size(), self.size as u64, "the window is not the second stage's size");
        // SAFETY: Creates an event file descriptor, which is owned here.
        let stop = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
        if stop < 0 {
            return Err(os_error(std::io::Error::last_os_error()));
        }
        // SAFETY: `stop` was just created, and is owned here.
        let stop = Arc::new(unsafe { File::from_raw_fd(stop) });
        let thread_stop = stop.clone();
        let stage = self.clone();
        let thread = std::thread::Builder::new()
            .name("dax faults".into())
            .spawn(move || stage.serve_faults(&window, &thread_stop))
            .map_err(os_error)?;
        Ok(FaultThread { stop, thread: Some(thread) })
    }

    /// Passes the memory's faults to `window` until `stop` is signalled.
    fn serve_faults(&self, window: &DaxWindow, stop: &File) {
        let mut fds = [
            libc::pollfd { fd: self.uffd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            // SAFETY: `fds` is an array of `pollfd`s of its length.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ready < 0 {
                let err = std::io::Error::last_os_error();
                assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "poll userfaultfd: {err}");
                continue;
            }
            if fds[1].revents != 0 {
                return;
            }
            while let Some(msg) = self.read_event() {
                if msg.event == UFFD_EVENT_PAGEFAULT {
                    self.resolve(window, msg.address as usize, msg.flags);
                }
            }
        }
    }

    /// Reads the next event, if there is one.
    fn read_event(&self) -> Option<UffdMsg> {
        let mut msg = std::mem::MaybeUninit::<UffdMsg>::uninit();
        // SAFETY: Reads one message into `msg`, which is its size.
        let read = unsafe {
            libc::read(self.uffd.as_raw_fd(), msg.as_mut_ptr().cast(), size_of::<UffdMsg>())
        };
        if read < 0 {
            let err = std::io::Error::last_os_error();
            assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "read userfaultfd: {err}");
            return None;
        }
        assert_eq!(read as usize, size_of::<UffdMsg>(), "short read of a userfaultfd message");
        // SAFETY: The kernel wrote a whole message.
        Some(unsafe { msg.assume_init() })
    }

    /// Resolves the fault at host address `address` described by `flags`, and wakes the faulting
    /// thread.
    fn resolve(&self, window: &DaxWindow, address: usize, flags: u64) {
        let page_size = super::page_size() as usize;
        let addr = address - address % page_size;
        let offset = (addr - self.base) as u64;
        let write = flags & (UFFD_PAGEFAULT_FLAG_WRITE | UFFD_PAGEFAULT_FLAG_WP) != 0;
        if let Err(err) = window.fault(offset, write) {
            log::error!(
                "unresolved DAX window fault at offset {offset:#x} (write: {write}): {err}"
            );
            std::process::abort();
        }
        let mut range = UffdioRange { start: addr as u64, len: page_size as u64 };
        ioctl(&self.uffd, UFFDIO_WAKE, &mut range).expect("wake a faulting thread");
    }

    /// Registers the `len` bytes at host address `addr` for faults of `mode`.
    fn register(&self, addr: usize, len: usize, mode: u64) {
        let range = UffdioRange { start: addr as u64, len: len as u64 };
        let mut register = UffdioRegister { range, mode, ioctls: 0 };
        ioctl(&self.uffd, UFFDIO_REGISTER, &mut register)
            .unwrap_or_else(|err| panic!("register {addr:#x} with userfaultfd: {err}"));
    }

    /// Write-protects the `len` bytes at host address `addr`, or removes their write protection
    /// and wakes the threads faulting on it.
    fn write_protect(&self, addr: usize, len: usize, protect: bool) {
        let range = UffdioRange { start: addr as u64, len: len as u64 };
        let mode = if protect { UFFDIO_WRITEPROTECT_MODE_WP } else { 0 };
        let mut write_protect = UffdioWriteprotect { range, mode };
        ioctl(&self.uffd, UFFDIO_WRITEPROTECT, &mut write_protect)
            .unwrap_or_else(|err| panic!("write-protect {addr:#x} with userfaultfd: {err}"));
    }

    /// The host address of the page at `vaddr`.
    fn addr(&self, vaddr: usize, len: usize) -> usize {
        assert!(vaddr <= self.size && len <= self.size - vaddr, "out of the second stage");
        self.base + vaddr
    }
}

impl SecondStage for UffdSecondStage {
    fn map(&self, vaddr: usize, host_addr: usize, len: usize, prot: libc::c_int) {
        let addr = self.addr(vaddr, len);
        // SAFETY: `host_addr` is the window's shared mapping of its pages, which `mremap` maps
        // again over this second stage's memory at `addr`.
        let mapped = unsafe {
            libc::mremap(
                host_addr as *mut _,
                0,
                len,
                libc::MREMAP_MAYMOVE | libc::MREMAP_FIXED,
                addr as *mut libc::c_void,
            )
        };
        assert_eq!(mapped as usize, addr, "map {addr:#x}: {}", std::io::Error::last_os_error());
        // Stores are tracked by write-protecting the page, rather than by its protection, whose
        // faults userfaultfd does not report.
        // SAFETY: As above.
        let status =
            unsafe { libc::mprotect(addr as *mut _, len, libc::PROT_READ | libc::PROT_WRITE) };
        assert_eq!(status, 0, "protect {addr:#x}: {}", std::io::Error::last_os_error());
        self.register(addr, len, UFFDIO_REGISTER_MODE_WP);
        if prot & libc::PROT_WRITE == 0 {
            self.write_protect(addr, len, true);
        }
    }

    fn protect(&self, vaddr: usize, len: usize, prot: libc::c_int) {
        assert_ne!(prot & libc::PROT_READ, 0, "a guest's pages are always readable");
        self.write_protect(self.addr(vaddr, len), len, prot & libc::PROT_WRITE == 0);
    }

    fn unmap(&self, vaddr: usize, len: usize) {
        let addr = self.addr(vaddr, len);
        // SAFETY: Replaces this second stage's memory at `addr`.
        let mapped = unsafe {
            libc::mmap(
                addr as *mut _,
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_eq!(mapped as usize, addr, "unmap {addr:#x}: {}", std::io::Error::last_os_error());
        self.register(addr, len, UFFDIO_REGISTER_MODE_MISSING);
    }
}

/// The thread serving a second stage's faults, which stops when this is dropped.
pub struct FaultThread {
    stop: Arc<File>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FaultThread {
    fn drop(&mut self) {
        use std::io::Write;
        (&*self.stop).write_all(&1u64.to_ne_bytes()).expect("stop the fault thread");
        if let Err(panic) = self.thread.take().unwrap().join() {
            std::panic::resume_unwind(panic);
        }
    }
}

/// Opens a userfaultfd that handles faults taken in the kernel as well as in user mode.
fn open_userfaultfd() -> Result<OwnedFd> {
    let flags = libc::O_CLOEXEC | libc::O_NONBLOCK;
    // `/dev/userfaultfd`'s permissions grant handling kernel faults, where the system call needs
    // the `vm.unprivileged_userfaultfd` sysctl or CAP_SYS_PTRACE.
    let fd = match File::options().read(true).write(true).open("/dev/userfaultfd") {
        // SAFETY: Creates a userfaultfd, which is owned below.
        Ok(dev) => unsafe { libc::ioctl(dev.as_raw_fd(), USERFAULTFD_IOC_NEW as _, flags) },
        // SAFETY: As above.
        Err(_) => unsafe { libc::syscall(libc::SYS_userfaultfd, flags) as libc::c_int },
    };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        log::error!(
            "userfaultfd: {err}: handling a guest's faults needs access to /dev/userfaultfd, or \
             vm.unprivileged_userfaultfd=1"
        );
        return Err(os_error(err));
    }
    // SAFETY: `fd` was just created, and is owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Issues userfaultfd ioctl `request` with `arg`.
fn ioctl<T>(uffd: &OwnedFd, request: u64, arg: &mut T) -> std::io::Result<()> {
    // SAFETY: `arg` is the structure `request` takes.
    if unsafe { libc::ioctl(uffd.as_raw_fd(), request as _, arg as *mut T) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The error for the host's `err`, whose errno values are Linux's.
fn os_error(err: std::io::Error) -> Error {
    Error::new(err.raw_os_error().unwrap_or(linux::EIO))
}
