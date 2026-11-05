// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-149.28: the VM process's side of a copy-on-write DAX window under the seccomp filters it
//! runs under in production, Firecracker's (vendor/firecracker/resources/seccomp): its VMM
//! thread's, `vmm`, which the threads fxdax starts in its process run under (`fxdax::thread`,
//! Firecracker's `confine_extent_threads`), and its vCPUs', `vcpu`. Four system calls copy-on-write
//! DAX makes were missing from `vmm`, `memfd_create`, `fallocate`, `UFFDIO_COPY`'s `ioctl` and
//! `pread64`, each found only as a jailed VM on GCE was killed with "bad syscall": nothing else ran
//! this code under the filters.
//!
//! The filters are compiled from their JSON as Firecracker's build compiles them, with its
//! seccompiler, but for what they refuse: where Firecracker's trap, killing the process, these
//! notify a supervisor (`SECCOMP_RET_USER_NOTIF`), which records the call, its arguments and its
//! thread, and lets it go on, so that a run tells of every call refused, not of the first alone.
//!
//! Firecracker is built for musl without debug assertions, and this test for glibc with them,
//! which make some calls otherwise: [`in_production`] has those this test meets, each refused
//! only if Firecracker's own would be. What musl alone calls, nothing here sees; nor a thread's
//! start, glibc's `clone3` where musl's is `clone`: each thread is confined once started.
//!
//! In a binary of its own, as its spawner confines every thread fxdax starts in the process.

#![cfg(target_os = "linux")]

mod common;

use fxdax::pages::page_size;
use fxdax::{ExtentId, RemoteExtentStore, VolatileSlice};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Firecracker's seccomp filters' JSON, for this architecture and musl, as its build compiles them.
fn filters_json() -> std::path::PathBuf {
    let arch = std::env::consts::ARCH;
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../vendor/firecracker/resources/seccomp/{arch}-unknown-linux-musl.json"))
}

/// A BPF program's return of a constant (`BPF_RET | BPF_K`).
const BPF_RET_K: u16 = 0x06;

/// Firecracker's filter of the threads `kind` names ("vmm", "vcpu"), compiled as its build compiles
/// it, but notifying a supervisor of what it refuses, where Firecracker's traps.
fn filter(kind: &str) -> Arc<[libc::sock_filter]> {
    let json = filters_json();
    let dir = tempfile::tempdir().unwrap();
    // Each kind's program, raw, beside the path given (`split_output`).
    let out = dir.path().join("filters");
    let arch = std::env::consts::ARCH;
    seccompiler::compile_bpf(json.to_str().unwrap(), arch, out.to_str().unwrap(), false, true)
        .unwrap_or_else(|err| panic!("compiling {}: {err}", json.display()));
    let program = std::fs::read(dir.path().join(format!("{kind}.bpf"))).unwrap();
    assert!(program.len().is_multiple_of(8), "{kind}'s program isn't whole instructions");
    let mut traps = 0;
    let program: Arc<[libc::sock_filter]> = (program.chunks_exact(8))
        .map(|insn| {
            let mut insn = libc::sock_filter {
                code: u16::from_ne_bytes([insn[0], insn[1]]),
                jt: insn[2],
                jf: insn[3],
                k: u32::from_ne_bytes([insn[4], insn[5], insn[6], insn[7]]),
            };
            if insn.code == BPF_RET_K
                && insn.k & libc::SECCOMP_RET_ACTION_FULL == libc::SECCOMP_RET_TRAP
            {
                insn.k = libc::SECCOMP_RET_USER_NOTIF;
                traps += 1;
            }
            insn
        })
        .collect();
    assert_ne!(traps, 0, "{kind}'s filter traps nothing: Firecracker's refuses otherwise now");
    program
}

/// A system call a filter refused: the thread's, its number and its arguments.
struct Refused {
    thread: Arc<str>,
    nr: i32,
    args: [u64; 6],
}

/// What the filters refused, recorded by their supervisors, which mustn't allocate (see
/// [`supervise`]): it has room for [`ROOM`] calls, and counts those past it.
static REFUSED: Mutex<Vec<Refused>> = Mutex::new(Vec::new());
static UNRECORDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const ROOM: usize = 1 << 16;

/// The supervisors of the threads confined, each running until its thread is gone.
static SUPERVISORS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Confines this thread, `name`, with `program`: what it refuses, a supervisor of the thread's
/// records, letting the call go on.
fn confine(name: &str, program: &[libc::sock_filter]) {
    let (give, listener) = std::sync::mpsc::sync_channel::<OwnedFd>(1);
    let thread: Arc<str> = name.into();
    // Started first: confined, this thread's start of one would be refused, waiting for it.
    let supervisor = std::thread::Builder::new()
        .name(format!("seccomp {name}"))
        .spawn(move || supervise(thread, listener.recv().unwrap()))
        .unwrap();
    SUPERVISORS.lock().unwrap().push(supervisor);
    let len = u16::try_from(program.len()).expect("a seccomp program of 64 Ki instructions");
    let program = libc::sock_fprog { len, filter: program.as_ptr().cast_mut() };
    // SAFETY: Plain system calls: this thread can gain no privileges from now on, as a filter of
    // an unprivileged thread must, and is filtered by `program`, which the kernel copies.
    let listener = unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        let flags = libc::SECCOMP_FILTER_FLAG_NEW_LISTENER;
        let mode = libc::SECCOMP_SET_MODE_FILTER;
        let listener = libc::syscall(libc::SYS_seccomp, mode, flags, &program);
        let error = std::io::Error::last_os_error();
        assert!(listener >= 0, "confining {name}: {error}");
        OwnedFd::from_raw_fd(listener as libc::c_int)
    };
    give.send(listener).unwrap();
}

/// Records each call `listener`'s filter refuses `thread`, and lets it go on, until the thread is
/// gone. It allocates nothing as it does: a thread refused a call in an allocation, holding its
/// allocator's lock, would wait for the supervisor, the supervisor for the lock.
fn supervise(thread: Arc<str>, listener: OwnedFd) {
    let fd = listener.as_raw_fd();
    loop {
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: Plain system call on one `pollfd`.
        if unsafe { libc::poll(&mut poll, 1, -1) } < 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::Interrupted,
                "polling {thread}'s: {error}"
            );
            continue;
        }
        // Hung up alone: the thread is gone.
        if poll.revents & libc::POLLIN == 0 {
            return;
        }
        // SAFETY: Zeros are a notification, which the kernel overwrites.
        let mut notification: libc::seccomp_notif = unsafe { std::mem::zeroed() };
        // SAFETY: Receives a notification into `notification`, its size.
        if unsafe { libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_RECV, &mut notification) } < 0 {
            // The call interrupted before it was received: signalled, it goes on without it.
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::ENOENT), "receiving {thread}'s: {error}");
            continue;
        }
        let refused = Refused {
            thread: thread.clone(),
            nr: notification.data.nr,
            args: notification.data.args,
        };
        let mut recorded = REFUSED.lock().unwrap();
        if recorded.len() < recorded.capacity() {
            recorded.push(refused);
        } else {
            UNRECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        drop(recorded);
        let flags = libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32;
        let mut go_on = libc::seccomp_notif_resp { id: notification.id, val: 0, error: 0, flags };
        // SAFETY: Sends the response, its size.
        if unsafe { libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_SEND, &mut go_on) } < 0 {
            // Interrupted since: as above.
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::ENOENT), "answering {thread}'s: {error}");
        }
    }
}

/// A thread to start: its name, what it runs, and where its start is told of.
type Start = (String, Box<dyn FnOnce() + Send>, std::sync::mpsc::SyncSender<std::io::Result<()>>);

/// fxdax's thread spawner, as Firecracker's starts every thread fxdax starts in its process under
/// its VMM thread's filter (`confine_extent_threads`): from a thread of its own, as a thread
/// confined would start them, whose filter they would inherit, and its supervisor's listener, which
/// a thread's filters have one of; each confined with the program as it starts.
struct Confining(Mutex<std::sync::mpsc::Sender<Start>>);

impl Confining {
    fn new(program: Arc<[libc::sock_filter]>) -> Self {
        let (asks, asked) = std::sync::mpsc::channel::<Start>();
        let spawning = move || {
            while let Ok((name, run, started)) = asked.recv() {
                let (program, thread) = (program.clone(), name.clone());
                let confined = move || {
                    confine(&thread, &program);
                    run()
                };
                let _ =
                    started.send(std::thread::Builder::new().name(name).spawn(confined).map(drop));
            }
        };
        std::thread::Builder::new().name("seccomp spawner".into()).spawn(spawning).unwrap();
        Self(Mutex::new(asks))
    }
}

impl fxdax::thread::Spawn for Confining {
    fn spawn(&self, name: String, run: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        let (started, start) = std::sync::mpsc::sync_channel(1);
        let gone = || std::io::Error::other("the spawner is gone");
        self.0.lock().unwrap().send((name, run, started)).map_err(|_| gone())?;
        start.recv().map_err(|_| gone())?
    }
}

/// What Firecracker calls, built for musl without debug assertions, where this test, built for
/// glibc with them, called `name` with `args`.
enum Production {
    /// The same call.
    Same,
    /// Another call, which its filters allow whatever its arguments, if they do.
    Instead(&'static str),
    /// No call.
    Nothing,
}

/// What Firecracker calls where this test called `name` with `args`: the calls this test meets
/// that glibc or a build with debug assertions makes otherwise.
fn in_production(name: &str, args: &[u64; 6]) -> Production {
    let int = |arg: usize| args[arg] as u32 as i32;
    match name {
        // Rust's check, as an `OwnedFd` is dropped, that it is open (`debug_assert_fd_is_open`),
        // made with debug assertions alone (`core::ub_checks`).
        "fcntl" if cfg!(debug_assertions) && int(1) == libc::F_GETFD => Production::Nothing,
        // Rust's `File::metadata`: glibc's tries `statx` first; musl's calls `fstat`.
        "statx" if int(2) & libc::AT_EMPTY_PATH != 0 => Production::Instead("fstat"),
        // open(2), which x86-64's kernel has (aarch64's has `openat` alone): musl's is `open`.
        "openat" if cfg!(target_arch = "x86_64") => Production::Instead("open"),
        _ => Production::Same,
    }
}

#[link(name = "seccomp")]
unsafe extern "C" {
    /// libseccomp's name of system call `num` on `arch` (0: this architecture), allocated, or null.
    fn seccomp_syscall_resolve_num_arch(arch: u32, num: libc::c_int) -> *mut libc::c_char;
}

/// System call `nr`'s name, on this architecture.
fn syscall_name(nr: i32) -> String {
    // SAFETY: libseccomp's lookup, whose result, if any, is a C string, freed below.
    unsafe {
        let name = seccomp_syscall_resolve_num_arch(0, nr);
        if name.is_null() {
            return format!("system call {nr}");
        }
        let owned = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
        libc::free(name.cast());
        owned
    }
}

/// Whether the filter of `kind` in Firecracker's JSON allows `syscall` with any arguments.
fn allows_any(kind: &str, syscall: &str) -> bool {
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(filters_json()).unwrap()).unwrap();
    let rules = json[kind]["filter"].as_array().expect("a filter's rules");
    (rules.iter()).any(|rule| rule["syscall"] == syscall && rule.get("args").is_none())
}

/// What was refused that Firecracker's filters would refuse Firecracker ([`in_production`]), by thread, call and arguments, each with how many
/// times: refused of the thread kind its name says (`fc_vcpu` for the vCPUs', `vmm` for the rest).
fn refusals() -> Vec<String> {
    let refused = REFUSED.lock().unwrap();
    let mut told = std::collections::BTreeMap::<String, usize>::new();
    for call in refused.iter() {
        let name = syscall_name(call.nr);
        let kind = if call.thread.starts_with("fc_vcpu") { "vcpu" } else { "vmm" };
        match in_production(&name, &call.args) {
            Production::Same => {}
            Production::Instead(musls) if !allows_any(kind, musls) => {}
            Production::Instead(_) | Production::Nothing => continue,
        }
        let args = call.args.map(|arg| format!("{arg:#x}")).join(", ");
        let thread = &call.thread;
        *told
            .entry(format!("{kind}'s refused {thread} {name} ({}) of {args}", call.nr))
            .or_default() += 1;
    }
    let unrecorded = UNRECORDED.load(std::sync::atomic::Ordering::Relaxed);
    let mut told: Vec<String> =
        told.into_iter().map(|(call, times)| format!("{call}, {times} times")).collect();
    if unrecorded > 0 {
        told.push(format!("{unrecorded} calls more, unrecorded"));
    }
    told
}

/// A byte of a pipe's read into the page at `addr`, as the kernel writes it, faulting for writing
/// as KVM does for a vCPU: `Err` with the errno it failed with.
fn kernel_store(addr: usize) -> Result<(), i32> {
    let mut pipe = [0; 2];
    // SAFETY: Makes a pipe, writes a byte to it, and reads one into `addr`, closing it after.
    unsafe {
        assert_eq!(libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC), 0);
        assert_eq!(libc::write(pipe[1], [0x77u8].as_ptr().cast(), 1), 1);
        let read = libc::read(pipe[0], addr as *mut libc::c_void, 1);
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap();
        libc::close(pipe[0]);
        libc::close(pipe[1]);
        if read == 1 { Ok(()) } else { Err(errno) }
    }
}

/// Runs `work` on a thread fxdax starts, under the VMM thread's filter, as Firecracker's device and
/// its workers make their requests of the store.
fn on_vmm<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send) -> T {
    fxdax::thread::run(name, work).unwrap()
}

/// The VM process's side of a copy-on-write window, every thread of it under Firecracker's
/// filters: its client's, receiving the daemon's changes of the window, and its fault thread, as
/// fxdax starts them; the device's, making the window and its requests; a vCPU's, its calls into
/// fxdax. The window is made, sealed extents and zeros mapped, and faulted in; the guest's stores
/// make pages private, of a sealed extent's page, its bytes read from the daemon's memory, and of
/// zeros; a worker's write of a page is resolved as a load and the page made read-only, until a
/// vCPU's access fails; a `PRIVATE` makes a page private at once, an `UNMAP` frees one, and the
/// window lets go of its memory as the store goes. The filters refuse none of their calls.
#[test]
fn copy_on_write_windows_run_under_firecrackers_seccomp_filters() {
    common::host();
    REFUSED.lock().unwrap().reserve_exact(ROOM);
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    // The daemon, a process of its own in production, unconfined.
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || fxdax::extentd::serve(listener));
    fxdax::thread::set_spawner(Confining::new(filter("vmm")));
    // As Firecracker makes its device, before its VMM thread is confined, the client's thread
    // confined as it starts.
    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24).unwrap();
    let memory = common::memory(4);
    let base = memory.as_ptr() as usize;
    on_vmm("fc_extent_activate", || {
        let memory = std::ptr::NonNull::new(base as *mut u8).unwrap();
        // SAFETY: The memory is the test's, and outlives the store.
        unsafe { store.create_dax_window_in_host_memory(memory, 4 * ps as u64, None, 3) }
    })
    .unwrap();
    assert!(store.copies_on_write());
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    let bytes = common::pattern(1);
    on_vmm("fc_extent_q0", || {
        store.write(extent, 0, &[VolatileSlice::from(&bytes[..ps])]).unwrap();
        store.commit(None).unwrap();
        for page in [0, 1, 3] {
            store.map(extent, 0, ps as u64, (page * ps) as u64).unwrap();
        }
        futures::executor::block_on(store.map_zeros_async(2 * ps as u64, ps as u64)).unwrap();
    });
    // The window's bytes from `at` to `end`, as the guest reads them.
    let window = |at: usize, end: usize| -> Vec<u8> {
        // SAFETY: The window's memory, whose faults the store serves.
        (at..end).map(|at| unsafe { ((base + at) as *const u8).read_volatile() }).collect()
    };

    // This thread is the guest's, a vCPU's.
    fxdax::guest_thread_started();
    assert_eq!(window(0, 2 * ps), [&bytes[..ps], &bytes[..ps]].concat());
    // A store to a sealed extent's page, which the window maps, copies it; to zeros, copies none.
    // SAFETY: The window's memory, as above, a store of the guest's.
    unsafe { (base as *mut u8).write_volatile(0xee) };
    // SAFETY: As above.
    unsafe { ((base + 2 * ps) as *mut u8).write_volatile(0xdd) };
    assert_eq!(window(0, ps), [&[0xee], &bytes[1..ps]].concat());
    assert_eq!(window(2 * ps, 3 * ps), [&[0xdd][..], &vec![0; ps - 1]].concat());
    // A worker's write of a page not mapped here, KVM's, is resolved as a load, and fails.
    let worker = std::thread::spawn(move || kernel_store(base + 3 * ps));
    assert_eq!(worker.join().unwrap(), Err(libc::EFAULT));
    // The vCPU's access fails: it says so, under its own filter, which lifts the worker's
    // restriction, and tells of the windows.
    let vcpu = filter("vcpu");
    let vcpu = std::thread::Builder::new().name("fc_vcpu 0".into()).spawn(move || {
        confine("fc_vcpu 0", &vcpu);
        fxdax::guest_access_failed();
        fxdax::describe_windows(Some(base))
    });
    assert!(!vcpu.unwrap().join().unwrap().is_empty());
    // Retried, as Firecracker retries the vCPU, until the restriction is lifted.
    while let Err(errno) = kernel_store(base + 3 * ps) {
        assert_eq!(errno, libc::EFAULT);
        std::thread::yield_now();
    }
    assert_eq!(window(3 * ps, 3 * ps + 2), [0x77, bytes[1]]);

    on_vmm("fc_extent_q0", || {
        store.unmap(0, ps as u64).unwrap();
        store.make_private(ps as u64, ps as u64).unwrap();
    });
    assert_eq!(window(ps, 2 * ps), bytes[..ps]);
    on_vmm("fc_extent_q0", || store.unmap(0, 4 * ps as u64).unwrap());
    // The device goes, and the window lets go of its memory.
    on_vmm("fc_extent_reset", move || drop(store));

    // Once every thread confined is gone, so that each call it made is recorded.
    let supervisors = std::mem::take(&mut *SUPERVISORS.lock().unwrap());
    supervisors.into_iter().for_each(|supervisor| supervisor.join().unwrap());
    assert_eq!(
        refusals(),
        Vec::<String>::new(),
        "Firecracker's seccomp filters ({}) refuse what fxdax calls",
        filters_json().display()
    );
    // SAFETY: Unmaps the memory, which nothing uses any more.
    assert_eq!(unsafe { libc::munmap(memory.as_ptr().cast(), 4 * ps) }, 0);
}
