// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-149.17: what a compromised VM process does to the daemon, checked against what must stay
//! true of the tenants beside it.

use super::super::client::RemoteExtentStore;
use super::super::server::{QUEUED_ROOM, Tenant, serve, serve_tenant};
use super::super::wire::*;
use super::{BLOCK, host, read, write};
use crate::ExtentId;
use crate::pages::page_size;
use std::alloc::{GlobalAlloc, Layout, System};
use std::os::fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts what the process holds of its heap (`HELD`), and the most it held (`PEAK`), as they are
/// allocated: what the daemon holds of its VM processes', which the same process serves here.
struct Counted;

static HELD: AtomicUsize = AtomicUsize::new(0);
/// The most it held since [`peak_while`] last looked.
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: Counted = Counted;

impl Counted {
    fn check(size: usize) {
        let held = HELD.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(held, Ordering::Relaxed);
    }
}

// SAFETY: this only counts what `System` does.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::check(layout.size());
        // SAFETY: As the caller promises.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::check(layout.size());
        // SAFETY: As the caller promises.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: As the caller promises.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        Self::check(size);
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: As the caller promises.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// The most the heap held past what it holds now while `work` ran.
fn peak_while(work: impl FnOnce()) -> u64 {
    let before = HELD.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    work();
    PEAK.load(Ordering::Relaxed).saturating_sub(before) as u64
}

/// What the daemon holds at most of what a VM process with a window of `window` bytes keeps in
/// flight, as the protocol bounds it: its requests' stream data, what it queued for it besides,
/// and as many mappings as a reply to `MAPPINGS` holds of its window (TASK-79.16, TASK-79.43).
fn room(window: u64) -> u64 {
    MAX_STREAM_BYTES + QUEUED_ROOM + window / page_size() * MAPPING_BYTES
}

/// A transfer area of `len` bytes, sealed against shrinking, as the daemon takes one.
fn sealed_memfd(len: u64) -> OwnedFd {
    // SAFETY: Makes a descriptor, owned below.
    let fd = unsafe { libc::memfd_create(c"transfer".as_ptr(), libc::MFD_ALLOW_SEALING) };
    assert!(fd >= 0);
    // SAFETY: Just made, and owned here.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: Sizes and seals it.
    unsafe {
        assert_eq!(libc::ftruncate(fd.as_raw_fd(), len as i64), 0);
        assert_eq!(libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK), 0);
    }
    fd
}

/// TASK-149.17: a VM process that shrinks the memory it is given as its store opens crashes
/// nothing: not its tenant's arena, which it may write, whose pages past a new end would fault the
/// daemon's own accesses (`SIGBUS`), and every tenant's VMs with it; nor the public one, which it
/// may only read. It may grow its arena, as the daemon goes on growing it. The memory it reads
/// alone is its owner's alone to open anew: a VM process, of another user, can't reopen it to
/// write through `/proc/self/fd`.
#[test]
fn vm_processes_shrinking_their_memory_crash_nothing() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let vms = dir.path().join("vms");
    std::fs::create_dir(&vms).unwrap();
    let socket = dir.path().join("vms.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let tenant = Tenant::new("vms", &vms).unwrap();
    std::thread::spawn(move || serve_tenant(listener, tenant));
    let store = RemoteExtentStore::connect(&socket, vms.join("store"), 1 << 26).unwrap();
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    write(&store, extent(1), 0, &super::pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // In the extent cache, the arena's pages.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));

    let (own, public) = (store.physical_memory().as_raw_fd(), store.public_memory().as_raw_fd());
    let truncated = |fd, len: i64| {
        // SAFETY: Sizes an object the test holds a descriptor of.
        match unsafe { libc::ftruncate(fd, len) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error().raw_os_error()),
        }
    };
    let shrunk = (truncated(own, 0), truncated(public, 0));
    // What the daemon reads of the arena's pages, which it would fault on, had the VM process
    // shrunk it.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));
    assert_eq!(shrunk, (Err(Some(libc::EPERM)), Err(Some(libc::EINVAL))));
    // Grown, by a tebibyte, as the daemon grows it further, as its extents take more chunks.
    // SAFETY: A stat buffer the call fills, of a descriptor the test holds.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstat(own, &mut stat) }, 0);
    assert_eq!(truncated(own, stat.st_size + (1 << 40)), Ok(()));
    let extents = 2 * physmem::CHUNK_SIZE / (8 * BLOCK);
    for sequence in 2..2 + extents as u64 {
        write(&store, extent(sequence), 0, &super::pattern(sequence as u8, 8 * BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    for sequence in 1..2 + extents as u64 {
        let len = if sequence == 1 { BLOCK } else { 8 * BLOCK };
        assert_eq!(read(&store, extent(sequence), 0, len), Ok(super::pattern(sequence as u8, len)));
    }
    // SAFETY: As above.
    assert_eq!(unsafe { libc::fstat(public, &mut stat) }, 0);
    assert_eq!(stat.st_mode & 0o777, 0o600);
    drop(store);
}
/// TASK-149.17: what the daemon holds of a VM process's requests is within the connection's room
/// ([`room`]), whatever they ask: whole extents read and written, through the stream, as many at
/// once as the room holds, and through its transfer area, more at once than the connection serves,
/// their replies read as they come. Each running held a copy of its whole length, which the room
/// doesn't count, as a transfer's data isn't: 481 MiB held of reads, 1,024 of writes from the area.
#[test]
fn what_requests_hold_is_within_the_room() {
    host();
    let dir = tempfile::tempdir().unwrap();
    // A bucket of files, as one in memory would hold what it is sent in the heap counted here.
    let bucket = format!("file://{}/", dir.path().canonicalize().unwrap().join("bucket").display());
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe { std::env::set_var("FXDAX_EXTENTD_REMOTE", bucket) };
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (sender, mut receiver, _stream, branch) =
        super::raw_connection(&socket, &dir.path().join("store"), 1 << 33);
    let len = MAX_PAYLOAD;
    let area = sealed_memfd(len);
    let transfer = Header::new(TRANSFER, 1).with_args(&[len]);
    sender.send(transfer, &[], &[area.as_fd()]).unwrap();
    let transferred = receiver.receive(|| 0).unwrap().unwrap().header;
    assert_eq!((transferred.id, transferred.status), (1, 0));
    let extent = |sequence| [branch, sequence];
    let written = Header::new(WRITE, 2).with_args(&extent(1));
    assert_eq!(super::call(&sender, &mut receiver, written, &vec![7; len as usize]).header.status, 0);
    let committed = Header::new(COMMIT, 3);
    assert_eq!(super::call(&sender, &mut receiver, committed, &[]).header.status, 0);
    // In the extent cache, so that what bringing it in holds isn't counted here.
    let warm = Header::new(READ, 4).with_args(&[branch, 1, 0, len]);
    assert_eq!(super::call(&sender, &mut receiver, warm, &[]).header.status, 0);
    // So many of each kind, `at_once` in flight, as many as the protocol allows: those of the
    // stream whose data the room holds, and those of the area all of them. Their replies are read
    // as they come.
    let mut burst = |kind: &str, headers: Vec<Header>, payload: &[u8], at_once: usize| {
        let count = headers.len();
        let held = peak_while(|| {
            let mut headers = headers.into_iter();
            for header in headers.by_ref().take(at_once) {
                sender.send(header, payload, &[]).unwrap();
            }
            for _ in 0..count {
                let reply = receiver.receive(|| 0).unwrap().unwrap();
                assert_eq!((reply.header.kind, reply.header.status), (REPLY, 0), "{kind}");
                if let Some(header) = headers.next() {
                    sender.send(header, payload, &[]).unwrap();
                }
            }
        });
        // The daemon's, and the reply this reads, which its receiver holds as it does.
        (kind.to_owned(), held <= room(0) + MAX_PAYLOAD, held >> 20)
    };
    let stream = (MAX_STREAM_BYTES / len) as usize;
    let many = 16;
    let reads = (0..many).map(|id| Header::new(READ, 10 + id).with_args(&[branch, 1, 0, len]));
    let into_area = (0..many)
        .map(|id| Header::new(READ, 100 + id).with_args(&[branch, 1, 0, len, 0]).with_flags(READ_TRANSFER));
    let from_area = (0..many).map(|id| {
        Header::new(WRITE, 200 + id).with_args(&[branch, 2 + id, 0, 0, len]).with_flags(WRITE_TRANSFER)
    });
    let writes = (0..many).map(|id| Header::new(WRITE, 300 + id).with_args(&[branch, 20 + id, 0]));
    let held = [
        burst("reads", reads.collect(), &[], stream),
        burst("reads into the area", into_area.collect(), &[], many as usize),
        burst("writes from the area", from_area.collect(), &[], many as usize),
        burst("writes", writes.collect(), &vec![9; len as usize], stream),
    ];
    let within: Vec<_> = held.iter().map(|(kind, within, _)| (kind.as_str(), *within)).collect();
    assert_eq!(
        within,
        [("reads", true), ("reads into the area", true), ("writes from the area", true), ("writes", true)],
        "MiB held past the room's {} MiB and a reply's: {held:?}",
        room(0) >> 20,
    );
}
