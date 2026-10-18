// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A DAX window's second stage in host memory that a hypervisor follows, such as the memory
//! backing a KVM memory slot, whose faults userfaultfd reports.
//!
//! Nothing here is particular to a VMM: any whose hypervisor maps guest memory from host memory
//! and follows the host's mappings of it can hand the memory over.
//!
//! KVM maps a guest's memory from the host memory backing its memory slot, and follows changes to
//! the host's mappings. On arm64 it reports a fault it cannot resolve there only as `EFAULT`, with
//! no address, so the memory must never fault that way. What the window has not mapped, or maps
//! read-only, is instead registered with userfaultfd, which stops the faulting thread (a vCPU, or
//! KVM on its behalf) and reports the address:
//!
//! - An unmapped page is anonymous memory registered for missing faults.
//! - A mapped page maps the window's page again, read-write, from the kernel's physical memory
//!   (a memfd, which the process that owns the kernel may share with this one), registered for
//!   write-protect faults, and is write-protected while the window maps it read-only.
//!
//! A fault thread passes the faults to the window, which maps the page (and so maps it here), and
//! wakes the faulting thread. It resolves faults concurrently, as futures it polls as they make
//! progress, so that a fault waiting for Fxfs's pager does not hold up the others. A fault the window cannot resolve is fatal, as it is to a Zircon
//! guest: the faulting thread cannot go on.
//!
//! A fault's access is not always the guest's. x86 KVM hands a fault it cannot resolve without
//! waiting, as it cannot one userfaultfd reports, to a worker that faults the page in for writing
//! (`async_pf_execute`), and lets the vCPU retry its own access once the page is in. So a page a
//! writable mapping maps may be dirtied by a load, and written back needlessly; and a write to a
//! page the window maps read-only is resolved as a read, and the page made read-only here, so that
//! the worker's write fails and the vCPU's own access decides.
//!
//! Changing a page's mapping here replaces its memory, which takes several system calls, and a fault
//! between them finds memory not yet registered. Until it is registered, new memory is inaccessible,
//! or read-only where a store would go untracked, so that such a fault fails with `EFAULT` rather
//! than resolving wrongly: a page of zeros faulted in, or a store not dirtying its page. A VMM must
//! then let the guest retry its access, as fxdax's Firecracker retries a vCPU's `KVM_RUN` that
//! fails with `EFAULT` for a moment.
//!
//! The userfaultfd must handle faults taken in the kernel, such as KVM's. That needs access to
//! `/dev/userfaultfd`, or the `vm.unprivileged_userfaultfd` sysctl.

use super::{Access, Faults, Prefault, Protection, SecondStage};
use crate::error::{Errno, Error, Result};
use crate::kernel::page_size;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;

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
pub(crate) struct UffdSecondStage {
    base: usize,
    size: usize,
    uffd: OwnedFd,
    /// The kernel's physical memory, which the window's pages are mapped from.
    memories: Memories,
    /// The hypervisor, told of the pages mapped here, if it maps them ahead of the guest.
    prefault: Option<Arc<dyn Prefault>>,
}

/// The kernel's physical memory, by arena (TASK-43), which a window's pages are mapped from.
pub(crate) enum Memories {
    /// This process's kernel's, every arena's.
    InProcess,
    /// The arenas' a daemon gave this process: its tenant's, and the default, read-only, or the
    /// default alone.
    Given(Vec<(crate::kernel::ArenaId, OwnedFd)>),
}

impl Memories {
    /// The descriptor of `arena`'s memory, which the window maps pages of, and whether it is
    /// read-only, as a tenant's VM process holds the default arena.
    fn fd(&self, arena: crate::kernel::ArenaId) -> Option<(std::os::fd::RawFd, bool)> {
        let fd = match self {
            Self::InProcess => crate::kernel::arena_memory(arena)?.as_raw_fd(),
            Self::Given(memories) => {
                memories.iter().find(|(given, _)| *given == arena)?.1.as_raw_fd()
            }
        };
        // SAFETY: Plain system call on a descriptor the memories hold.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        Some((fd, flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY))
    }
}

impl UffdSecondStage {
    /// Takes over the `size` bytes of host memory at `base`, replacing them with memory whose
    /// faults userfaultfd reports, as nothing is mapped yet: the window's pages are mapped there
    /// from `phys`, the kernel's physical memory. `prefault`, if any, is told of the pages mapped
    /// there.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping that nothing else in the process uses.
    pub unsafe fn new(
        base: usize,
        size: u64,
        memories: Memories,
        prefault: Option<Arc<dyn Prefault>>,
    ) -> Result<Arc<Self>, Error> {
        let size = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        let page_size = page_size() as usize;
        if !base.is_multiple_of(page_size) || size == 0 || !size.is_multiple_of(page_size) {
            return Err(Error::msg("DAX window memory must be whole pages"));
        }
        let uffd = open_userfaultfd()?;
        let mut api =
            UffdioApi { api: UFFD_API, features: UFFD_FEATURE_WP_HUGETLBFS_SHMEM, ioctls: 0 };
        ioctl(&uffd, UFFDIO_API, &mut api)
            .map_err(|err| Error::new(anyhow::Error::new(err).context("UFFDIO_API")))?;
        let stage = Arc::new(Self { base, size, uffd, memories, prefault });
        stage.unmap(0, size as u64);
        Ok(stage)
    }

    /// As [`new`](Self::new), for a window of this process's kernel.
    ///
    /// # Safety
    ///
    /// As [`new`](Self::new).
    pub unsafe fn in_process(
        base: usize,
        size: u64,
        prefault: Option<Arc<dyn Prefault>>,
    ) -> Result<Arc<Self>, Error> {
        // SAFETY: As the caller promises.
        unsafe { Self::new(base, size, Memories::InProcess, prefault) }
    }

    /// The size of the memory.
    pub fn size(&self) -> u64 {
        self.size as u64
    }

    /// Serves the faults on the memory with `window`, on a thread of its own, until the returned
    /// thread is dropped.
    pub fn serve(self: &Arc<Self>, window: Arc<dyn Faults>) -> Result<FaultThread, Error> {
        assert_eq!(window.size(), self.size as u64, "the window is not the second stage's size");
        let stop = Arc::new(event_fd()?);
        let wake = Arc::new(EventFdWaker(event_fd()?));
        let thread_stop = stop.clone();
        let stage = self.clone();
        let thread = std::thread::Builder::new()
            .name("dax faults".into())
            .spawn(move || stage.serve_faults(&*window, &thread_stop, wake))
            .map_err(Error::new)?;
        Ok(FaultThread { stop, thread: Some(thread) })
    }

    /// Passes the memory's faults to `window` until `stop` is signalled. Faults are resolved
    /// concurrently: each is a future, which this thread polls once `wake`, its waker, signals that
    /// it can make progress, as when Fxfs's pager supplies the page it waits for.
    fn serve_faults(&self, window: &dyn Faults, stop: &File, wake: Arc<EventFdWaker>) {
        let waker = Waker::from(wake.clone());
        let mut cx = Context::from_waker(&waker);
        let mut faults = FuturesUnordered::new();
        let mut fds = [
            libc::pollfd { fd: self.uffd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.0.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            // Make what progress the faults can, and then wait for new faults or for a fault's
            // waker.
            while let Poll::Ready(Some(())) = faults.poll_next_unpin(&mut cx) {}
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
            if fds[2].revents != 0 {
                wake.reset();
            }
            while let Some(msg) = self.read_event() {
                if msg.event == UFFD_EVENT_PAGEFAULT {
                    faults.push(self.resolve(window, msg.address as usize, msg.flags));
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
    async fn resolve(&self, window: &dyn Faults, address: usize, flags: u64) {
        let page_size = page_size() as usize;
        let addr = address - address % page_size;
        let offset = (addr - self.base) as u64;
        let access = if flags & (UFFD_PAGEFAULT_FLAG_WRITE | UFFD_PAGEFAULT_FLAG_WP) != 0 {
            Access::Write
        } else {
            Access::Read
        };
        // A store to a page mapped here read-only is the guest's: KVM faults loads of pages mapped
        // here without userfaultfd. A fault on a page not mapped here may be for writing whatever
        // the guest's access (see the module docs), and resolves that page alone.
        let store = flags & UFFD_PAGEFAULT_FLAG_WP != 0;
        let resolved = if store {
            window.fault(offset, access).await
        } else {
            window.fault_page(offset, access).await
        };
        let resolved = match resolved {
            // Where the window refuses a write, the page is mapped read-only: make it read-only
            // here too, so that the write fails in the kernel rather than faulting here again. A
            // store from the guest then fails as KVM's would.
            Err(Errno::EACCES) if access == Access::Write => {
                self.read_only(addr, page_size);
                Ok(())
            }
            resolved => resolved,
        };
        if let Err(err) = resolved {
            log::error!("unresolved DAX window fault at offset {offset:#x} ({access:?}): {err}");
            std::process::abort();
        }
        let mut range = UffdioRange { start: addr as u64, len: page_size as u64 };
        ioctl(&self.uffd, UFFDIO_WAKE, &mut range).expect("wake a faulting thread");
        // A write to a page not mapped here may start a run a sequential writer stores to: the pages
        // after it are mapped read-only, the next read in if need be, so that stores to them fault
        // as stores, which dirty runs of pages, and loads do not fault at all. Past the file, or
        // the window, there is nothing to map.
        let next = offset + page_size as u64;
        if !store && access == Access::Write && next < window.size() {
            match window.fault_page(next, Access::Read).await {
                Ok(()) | Err(Errno::ENOENT | Errno::EINVAL) => {}
                Err(err) => log::warn!("mapping DAX window offset {next:#x} ahead: {err}"),
            }
        }
    }

    /// Makes the `len` bytes at host address `addr` read-only.
    fn read_only(&self, addr: usize, len: usize) {
        // SAFETY: Changes the protection of this second stage's memory.
        let status = unsafe { libc::mprotect(addr as *mut _, len, libc::PROT_READ) };
        assert_eq!(status, 0, "protect {addr:#x}: {}", std::io::Error::last_os_error());
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

    /// The host address of the `len` bytes at `offset`, as a length in host terms.
    fn addr(&self, offset: u64, len: u64) -> (usize, usize) {
        let (offset, len) = (offset as usize, len as usize);
        assert!(offset <= self.size && len <= self.size - offset, "out of the second stage");
        (self.base + offset, len)
    }
}

impl SecondStage for UffdSecondStage {
    fn map(
        &self,
        offset: u64,
        _host_addr: *mut u8,
        phys: crate::kernel::PhysLocation,
        len: u64,
        protection: Protection,
    ) {
        let (addr, len) = self.addr(offset, len);
        // A page of an arena the window may not map is the daemon's error, which the guest must
        // not get past.
        let (fd, read_only) =
            self.memories.fd(phys.arena).expect("a window's page is of its arenas");
        let prot = match protection {
            Protection::Read => libc::PROT_READ,
            _ => libc::PROT_READ | libc::PROT_WRITE,
        };
        // Memory this process holds read-only, the default arena's, public bases' pages, is mapped
        // private: userfaultfd tracks stores to a mapping that may be made writable alone, and a
        // store to a private one could only copy the page into this process, which the window
        // never lets it make, as it maps such pages read-only. Its loads read the very pages.
        assert!(!read_only || protection == Protection::Read, "a read-only page mapped writably");
        let share = if read_only { libc::MAP_PRIVATE } else { libc::MAP_SHARED };
        // SAFETY: Maps the window's pages, of the kernel's physical memory, over this second
        // stage's memory at `addr`.
        let mapped = unsafe {
            libc::mmap(
                addr as *mut libc::c_void,
                len,
                prot,
                share | libc::MAP_FIXED,
                fd,
                phys.offset as libc::off_t,
            )
        };
        assert_eq!(mapped as usize, addr, "map {addr:#x}: {}", std::io::Error::last_os_error());
        // Stores are tracked by write-protecting the page, rather than by its protection, whose
        // faults userfaultfd does not report. The new mapping has the window's protection until
        // it is write-protected, so that a store meanwhile fails rather than going untracked.
        self.register(addr, len, UFFDIO_REGISTER_MODE_WP);
        assert_ne!(protection, Protection::None, "a guest's pages are always readable");
        if protection == Protection::Read {
            self.write_protect(addr, len, true);
        }
        // SAFETY: As above.
        let status =
            unsafe { libc::mprotect(addr as *mut _, len, libc::PROT_READ | libc::PROT_WRITE) };
        assert_eq!(status, 0, "protect {addr:#x}: {}", std::io::Error::last_os_error());
        // The pages are mapped for the guest ahead of its accesses, as when a file is mapped, an
        // open extent's pages are mapped for the guest to write, or a fault maps the pages after
        // its own. A writable page is mapped writable here first: its first access through the
        // new mapping would map it read-only, as the mapping tracks stores, and the guest's
        // first store would then fault anyway.
        if protection == Protection::ReadWrite {
            // SAFETY: Faults in the pages just mapped, without changing them.
            let status = unsafe { libc::madvise(addr as *mut _, len, libc::MADV_POPULATE_WRITE) };
            assert_eq!(status, 0, "populate {addr:#x}: {}", std::io::Error::last_os_error());
        }
        if let Some(prefault) = &self.prefault {
            prefault.prefault(offset, len as u64);
        }
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        assert_ne!(protection, Protection::None, "a guest's pages are always readable");
        let (addr, len) = self.addr(offset, len);
        self.write_protect(addr, len, protection == Protection::Read);
    }

    fn unmap(&self, offset: u64, len: u64) {
        let (addr, len) = self.addr(offset, len);
        // The new memory is inaccessible until it is registered, so that an access meanwhile fails
        // rather than faulting in a page of zeros.
        // SAFETY: Replaces this second stage's memory at `addr`.
        let mapped = unsafe {
            libc::mmap(
                addr as *mut _,
                len,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_eq!(mapped as usize, addr, "unmap {addr:#x}: {}", std::io::Error::last_os_error());
        self.register(addr, len, UFFDIO_REGISTER_MODE_MISSING);
        // SAFETY: As above.
        let status =
            unsafe { libc::mprotect(addr as *mut _, len, libc::PROT_READ | libc::PROT_WRITE) };
        assert_eq!(status, 0, "protect {addr:#x}: {}", std::io::Error::last_os_error());
    }
}

/// A new event file descriptor, which does not block reads.
fn event_fd() -> Result<File, Error> {
    // SAFETY: Creates an event file descriptor, which is owned below.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(Error::new(std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just created, and is owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Wakes the fault thread by signalling an event file descriptor it polls.
struct EventFdWaker(File);

impl EventFdWaker {
    /// Clears the signal, once the thread has woken.
    fn reset(&self) {
        let mut count = [0u8; 8];
        // A read fails with `EAGAIN` if a racing reset cleared the signal first.
        let _ = (&self.0).read(&mut count);
    }
}

impl Wake for EventFdWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // A write fails with `EAGAIN` only if the counter would overflow, when it is signalled.
        let _ = (&self.0).write(&1u64.to_ne_bytes());
    }
}

/// The thread serving a second stage's faults, which stops when this is dropped.
pub(crate) struct FaultThread {
    stop: Arc<File>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FaultThread {
    fn drop(&mut self) {
        (&*self.stop).write_all(&1u64.to_ne_bytes()).expect("stop the fault thread");
        if let Err(panic) = self.thread.take().unwrap().join() {
            std::panic::resume_unwind(panic);
        }
    }
}

/// Opens a userfaultfd that handles faults taken in the kernel as well as in user mode.
fn open_userfaultfd() -> Result<OwnedFd, Error> {
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
        return Err(Error::new(anyhow::Error::new(std::io::Error::last_os_error()).context(
            "userfaultfd: handling a guest's faults needs access to /dev/userfaultfd, or \
             vm.unprivileged_userfaultfd=1",
        )));
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

#[cfg(test)]
mod tests {
    use super::super::Dax;
    use super::super::tests::TestPager;
    use super::*;

    /// The fault thread resolves faults concurrently: a fault waiting for the pager does not hold
    /// up the next, whose page request reaches the pager first.
    #[test]
    fn faults_are_resolved_concurrently() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        let pager = TestPager::new(2);
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 2 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage).unwrap();
        dax.window.map(0, &pager.vmo, 0, 2 * ps as u64, false).unwrap();

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load =
            move |offset: usize| move || unsafe { ((base + offset) as *const u8).read_volatile() };
        let first = std::thread::spawn(load(0));
        assert_eq!(pager.read_request(zx::MonotonicInstant::INFINITE), 0..ps as u64);
        let second = std::thread::spawn(load(ps));
        assert_eq!(pager.read_request(zx::MonotonicInstant::INFINITE), ps as u64..2 * ps as u64);

        pager.supply(1, 2);
        assert_eq!(second.join().unwrap(), 2);
        assert!(!first.is_finished());
        pager.supply(0, 1);
        assert_eq!(first.join().unwrap(), 1);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 2 * ps) }, 0);
    }
}
