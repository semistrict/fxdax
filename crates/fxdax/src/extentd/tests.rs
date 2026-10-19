// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::client::{Opening, RemoteExtentStore};
use super::server::{serve, serve_connection};
use crate::kernel::page_size;
use crate::{Commit, Errno, ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut, WriteBarriers};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::ptr::NonNull;
use std::thread::JoinHandle;

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

/// Makes this process a host of the multi-node system, as the daemon's must be: its stores'
/// branches in a bucket in its memory, as of placement 1.
fn host() {
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", "memory:///branches");
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
}

/// A daemon serving one connection, on a thread, and its client, of the store at `path`.
fn connect(path: &Path, capacity: u64) -> (RemoteExtentStore, JoinHandle<()>) {
    let (client, server) = UnixStream::pair().unwrap();
    let served = std::thread::spawn(move || serve_connection(server, None));
    (RemoteExtentStore::over(client, path, Opening::Create(capacity)).unwrap(), served)
}

/// fxdax: a daemon serving one connection, on a thread, and its client, a follower of the store
/// at `path` (backlog doc-8).
fn follow(path: &Path) -> Result<(RemoteExtentStore, JoinHandle<()>), Option<Errno>> {
    let (client, server) = UnixStream::pair().unwrap();
    let served = std::thread::spawn(move || serve_connection(server, None));
    match RemoteExtentStore::over(client, path, Opening::ReadOnly) {
        Ok(store) => Ok((store, served)),
        Err(error) => {
            served.join().unwrap();
            Err(error.errno())
        }
    }
}

/// fxdax: the commits a follower hears of ([`RemoteExtentStore::on_committed`]).
fn commits_heard(follower: &RemoteExtentStore) -> std::sync::mpsc::Receiver<u64> {
    let (heard, commits) = std::sync::mpsc::channel();
    follower.on_committed(move |epoch| {
        // Unheard once the test stops listening.
        let _ = heard.send(epoch);
    });
    commits
}

/// `len` bytes of `byte` plus their offset modulo 251.
fn pattern(byte: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| byte ^ (i % 251) as u8).collect()
}

fn write(
    store: &RemoteExtentStore,
    extent: ExtentId,
    offset: u64,
    data: &[u8],
) -> Result<(), Errno> {
    // In two buffers, as a request's descriptors may hold it.
    let (first, second) = data.split_at(data.len() / 2);
    store.write(extent, offset, &[VolatileSlice::from(first), VolatileSlice::from(second)])
}

fn read(
    store: &RemoteExtentStore,
    extent: ExtentId,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, Errno> {
    let mut data = vec![0; len];
    let (first, second) = data.split_at_mut(len / 2);
    store.read(
        extent,
        offset,
        &mut [VolatileSliceMut::from(first), VolatileSliceMut::from(second)],
    )?;
    Ok(data)
}

/// `pages` pages of anonymous memory, for a window.
fn memory(pages: usize) -> NonNull<u8> {
    // SAFETY: Maps anonymous memory, which the test owns.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            pages * page_size() as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(memory, libc::MAP_FAILED);
    NonNull::new(memory.cast()).unwrap()
}

/// The file and offset this process maps the page at `addr` from, as /proc/self/maps has them:
/// the device and inode, and the offset.
fn mapped_from(addr: usize) -> (String, u64) {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    for line in maps.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (start, end) = fields[0].split_once('-').unwrap();
        let (start, end) =
            (usize::from_str_radix(start, 16).unwrap(), usize::from_str_radix(end, 16).unwrap());
        if (start..end).contains(&addr) {
            let offset = u64::from_str_radix(fields[2], 16).unwrap();
            return (format!("{} {}", fields[3], fields[4]), offset + (addr - start) as u64);
        }
    }
    panic!("{addr:#x} isn't mapped");
}

/// How /proc/self/maps names anonymous memory's file: none.
const ANONYMOUS: &str = "00:00 0";

/// fxdax: with `FXDAX_EXTENTD_REMOTE` naming a bucket, the stores the daemon opens keep their
/// branches there: a durable commit is an uploaded one.
#[test]
fn keeps_branches_in_object_storage() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 20);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(1, 2 * BLOCK)).unwrap();
    store.commit(Some(b"root")).unwrap();
    let bucket = super::stores::remote_bucket().unwrap();
    futures::executor::block_on(async {
        let state = extent_remote::recover(&*bucket, store.branch(), None).await.unwrap().state;
        assert_eq!(state.root, Some(b"root".to_vec()));
        let id = storage_device::ExtentId::new(extent.branch, extent.sequence);
        let data = extent_remote::read(&*bucket, id, &state.extents[&id]).await.unwrap();
        assert_eq!(data, pattern(1, 2 * BLOCK));
    });
    drop(store);
    served.join().unwrap();
}

/// fxdax: the daemon keeps what its bucket did, its requests and bytes, and what the stores'
/// followers are, how many and how far behind, in the file `FXDAX_EXTENTD_STATS` names, soon after
/// each change: the bench reports it (`scripts/bench-report.py`).
#[test]
fn keeps_object_storage_and_follower_stats() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stats.txt");
    // SAFETY: as in host.
    unsafe { std::env::set_var("FXDAX_EXTENTD_STATS", &file) };
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let branch = store.branch();
    let extent = move |sequence| ExtentId { branch, sequence };
    write(&store, extent(1), 0, &pattern(1, 2 * BLOCK)).unwrap();
    store.commit(Some(b"root")).unwrap();
    let bucket = super::stores::remote_bucket().unwrap();
    assert!(bucket.stats().uploaded(extent_remote::store::Upload::Segments) > 0);
    // A follower two commits behind, keeping what the writer freed.
    let (follower, followed) = follow(&path).unwrap();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    // Within a few polls of the last change.
    let followers = |kept: &str| -> Option<Vec<(String, u64)>> {
        let line = kept.lines().nth(1)?.strip_prefix("followers: ")?;
        let field =
            |field: &str| field.split_once('=').map(|(k, v)| (k.to_owned(), v.parse().unwrap()));
        line.split(' ').map(field).collect()
    };
    let start = std::time::Instant::now();
    let kept = loop {
        let kept = std::fs::read_to_string(&file).unwrap_or_default();
        let behind = followers(&kept)
            .and_then(|fields| fields.into_iter().find(|(name, _)| name == "max-behind"));
        if kept.lines().next() == Some(&bucket.stats().line())
            && behind == Some(("max-behind".into(), 2))
        {
            break kept;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "kept {kept:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let fields = followers(&kept).unwrap();
    let names: Vec<_> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["views", "max-views", "max-behind", "max-stale-ms", "max-kept"]);
    assert_eq!((fields[0].1, fields[1].1, fields[4].1), (1, 1, 2 * BLOCK as u64), "{kept}");
    for (store, served) in [(follower, followed), (store, served)] {
        drop(store);
        served.join().unwrap();
    }
}

/// Runs `call`, a store operation of the control API's, on a runtime of its own, as the API does.
fn control<T>(call: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(call)
}

/// Resizes the store at `path`, as the control API does (`ResizeStore`).
fn resize(path: &Path, capacity: u64) -> Result<(), Errno> {
    control(super::stores::resize(path.to_owned(), capacity))
}

/// Forks the store at `from` to a new branch at `to`, as the control API does (`ForkStore`), and
/// returns the branch.
fn fork_store(from: &Path, to: &Path) -> Result<u64, crate::Error> {
    let branch = rand::random::<u64>().max(1);
    control(super::stores::fork(from.to_owned(), to.to_owned(), branch))?;
    Ok(branch)
}

/// Deletes the store at `path`, as the control API does (`DeleteStore`).
fn delete_store(path: &Path) -> Result<(), crate::Error> {
    Ok(control(super::stores::delete(path.to_owned()))?)
}

/// Waits for the store at `path` to be uploaded, as the control API does (`SyncStore`).
fn wait_uploaded(path: &Path) -> Result<(), crate::Error> {
    Ok(control(super::stores::sync(path.to_owned()))?)
}

/// A store a VM has open grows at once, and its VM process is told the new capacity; it shrinks
/// once its guest, asked, agrees, never below what it holds, and keeps its size if the guest
/// refuses. Durably: the store reopened has the capacity it was given.
#[test]
fn resizes_open_stores() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let (changes, changed) = std::sync::mpsc::channel();
    store.on_capacity(move |capacity, ask| changes.send((capacity, ask)).unwrap());
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(1, 64 * BLOCK)).unwrap();
    store.commit(None).unwrap();

    resize(&path, 4 << 20).unwrap();
    assert_eq!(changed.recv().unwrap(), (4 << 20, false));
    assert_eq!(store.info().unwrap().capacity, 4 << 20);

    // Asked, the guest agrees, then refuses.
    for (capacity, took) in [(2 << 20, true), (1 << 20, false)] {
        let resizing = {
            let path = path.clone();
            std::thread::spawn(move || resize(&path, capacity))
        };
        assert_eq!(changed.recv().unwrap(), (capacity, true));
        store.resized(capacity, took).unwrap();
        let expected = if took { Ok(()) } else { Err(Errno::ENOSPC) };
        assert_eq!(resizing.join().unwrap(), expected);
    }
    assert_eq!(store.info().unwrap().capacity, 2 << 20);
    // Below what the store holds, the guest isn't asked.
    assert_eq!(resize(&path, 32 * BLOCK as u64), Err(Errno::ENOSPC));
    assert!(changed.try_recv().is_err());

    drop(store);
    served.join().unwrap();
    let reopened = ExtentStore::open(&path).unwrap();
    assert_eq!(reopened.info().unwrap().capacity, 2 << 20);
}

/// A shrink whose caller drops its `ResizeStore` stops waiting for the guest (TASK-43.6): the
/// guest's answer, coming after, shrinks nothing.
#[test]
fn shrinks_dropped_by_their_callers_stop_waiting() {
    use super::control::proto::ResizeStoreRequest;
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 2 << 20);
    let (asks, asked) = std::sync::mpsc::channel();
    store.on_capacity(move |capacity, ask| asks.send((capacity, ask)).unwrap());
    let socket = dir.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let tenants = dir.path().join("tenants");
    std::fs::create_dir(&tenants).unwrap();
    std::thread::spawn(move || super::control::serve_control(listener, tenants));
    let connection = super::stores::open_paths().holder(&super::stores::store_key(&path)).unwrap();

    control(async {
        let mut client = super::control::connect(&socket).await.unwrap();
        let request =
            ResizeStoreRequest { path: path.to_str().unwrap().to_owned(), capacity: 1 << 20 };
        let asked = tokio::task::spawn_blocking(move || asked.recv().unwrap());
        tokio::select! {
            shrunk = client.resize_store(request) => panic!("{shrunk:?}"),
            asked = asked => assert_eq!(asked.unwrap(), (1 << 20, true)),
        }
        // Dropped, the call is reset, and the daemon drops its wait.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while connection.shrink_awaited() {
            assert!(tokio::time::Instant::now() < deadline, "the shrink still waits");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    store.resized(1 << 20, true).unwrap();
    assert_eq!(store.info().unwrap().capacity, 2 << 20);

    drop((store, connection));
    served.join().unwrap();
}

/// A store no VM has open grows, through the daemon, which no VM opens it from meanwhile; it does
/// not shrink, as its guest is not there to agree.
#[test]
fn resizes_closed_stores() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    drop(ExtentStore::create(&path, 1 << 20).unwrap());
    resize(&path, 3 << 20).unwrap();
    assert_eq!(resize(&path, 2 << 20), Err(Errno::EBUSY));
    assert_eq!(resize(&dir.path().join("none"), 2 << 20), Err(Errno::ENOENT));
    let (store, served) = connect(&path, 1 << 20);
    assert_eq!(store.info().unwrap().capacity, 3 << 20);
    drop(store);
    served.join().unwrap();
}

/// A store served by the daemon is the store: what it writes it reads, commits, and keeps once
/// the VM process is gone; it fails as the store does; and no other VM process opens it meanwhile.
#[test]
fn serves_a_store() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(1, 2 * BLOCK)).unwrap();
    assert_eq!(read(&store, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
    store.commit(Some(b"root")).unwrap();
    assert_eq!(store.root(), Ok(b"root".to_vec()));
    let info = store.info().unwrap();
    assert_eq!((info.capacity, info.used, info.next_sequence), (1 << 20, 2 * BLOCK as u64, 2));
    assert_eq!(write(&store, extent, 2 * BLOCK as u64, &[0; BLOCK]), Err(Errno::EPERM));
    let (other, other_server) = UnixStream::pair().unwrap();
    let other_served = std::thread::spawn(move || serve_connection(other_server, None));
    let error = RemoteExtentStore::over(other, &path, Opening::Create(1 << 20))
        .err()
        .expect("opened it twice");
    assert!(format!("{error:#}").contains(&Errno::EBUSY.to_string()), "{error:#}");
    other_served.join().unwrap();

    drop(store);
    served.join().unwrap();
    let mut data = vec![0; 2 * BLOCK];
    let local = ExtentStore::open(&path).unwrap();
    local.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    assert_eq!(data, pattern(1, 2 * BLOCK));
}

/// Reads and writes move their data through the connection's transfer area, not the socket
/// (TASK-26.18.6): those that fit a slot, while one is free; others go through the socket.
#[test]
fn reads_and_writes_go_through_the_transfer_area() {
    use super::client::TRANSFER_SLOT;
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 24);
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    write(&store, extent(1), 0, &pattern(1, 2 * BLOCK)).unwrap();
    assert_eq!(
        read(&store, extent(1), BLOCK as u64, BLOCK),
        Ok(pattern(1, 2 * BLOCK)[BLOCK..].to_vec())
    );
    assert_eq!(store.transfers(), 2);

    // A slot's worth, and more than a slot's.
    write(&store, extent(2), 0, &pattern(2, TRANSFER_SLOT)).unwrap();
    write(&store, extent(3), 0, &pattern(3, 2 * TRANSFER_SLOT)).unwrap();
    assert_eq!(store.transfers(), 3);
    assert_eq!(read(&store, extent(3), 0, 2 * TRANSFER_SLOT), Ok(pattern(3, 2 * TRANSFER_SLOT)));
    assert_eq!(read(&store, extent(2), 0, TRANSFER_SLOT), Ok(pattern(2, TRANSFER_SLOT)));
    assert_eq!(store.transfers(), 4);

    // No slot free: through the socket.
    let taken = store.take_transfer_slots();
    let used = store.transfers();
    write(&store, extent(4), 0, &pattern(4, BLOCK)).unwrap();
    assert_eq!(read(&store, extent(4), 0, BLOCK), Ok(pattern(4, BLOCK)));
    assert_eq!(store.transfers(), used);
    drop(taken);
    assert_eq!(read(&store, extent(4), 0, BLOCK), Ok(pattern(4, BLOCK)));
    assert_eq!(store.transfers(), used + 1);
    drop(store);
    served.join().unwrap();
}

/// The daemon refuses a transfer area it could fault on, one not sealed against shrinking, and
/// ranges past the end of the one it has.
#[test]
fn transfer_areas_are_checked() {
    use super::wire::{Header, READ, READ_TRANSFER, TRANSFER, WRITE, WRITE_TRANSFER};
    use std::os::fd::AsFd;
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 24);
    // SAFETY: The name is a C string.
    let fd = unsafe { libc::memfd_create(c"unsealed".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    // SAFETY: Just created, and owned here.
    let file = std::fs::File::from(unsafe {
        <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd)
    });
    file.set_len(1 << 20).unwrap();
    let unsealed = Header::new(TRANSFER, 0).with_args(&[1 << 20]);
    assert_eq!(store.call(unsealed, &[], &[file.as_fd()]).err(), Some(Errno::EINVAL));

    // The store's own area, given at its opening: once.
    // SAFETY: As above.
    let fd = unsafe {
        libc::memfd_create(c"sealed".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
    };
    assert!(fd >= 0);
    // SAFETY: As above.
    let sealed = std::fs::File::from(unsafe {
        <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd)
    });
    sealed.set_len(1 << 20).unwrap();
    // SAFETY: Seals memory this test made.
    let seal = unsafe {
        libc::fcntl(
            std::os::fd::AsRawFd::as_raw_fd(&sealed),
            libc::F_ADD_SEALS,
            libc::F_SEAL_SHRINK,
        )
    };
    assert_eq!(seal, 0);
    let again = Header::new(TRANSFER, 0).with_args(&[1 << 20]);
    assert_eq!(store.call(again, &[], &[sealed.as_fd()]).err(), Some(Errno::EEXIST));
    let extent = [store.branch(), 1];
    let area = 8 * super::client::TRANSFER_SLOT as u64;
    let past = Header::new(READ, 0)
        .with_args(&[extent[0], extent[1], 0, BLOCK as u64, area - BLOCK as u64 + 1])
        .with_flags(READ_TRANSFER);
    assert_eq!(store.call(past, &[], &[]).err(), Some(Errno::EINVAL));
    let wrapping = Header::new(WRITE, 0)
        .with_args(&[extent[0], extent[1], 0, u64::MAX, 2])
        .with_flags(WRITE_TRANSFER);
    assert_eq!(store.call(wrapping, &[], &[]).err(), Some(Errno::EINVAL));
    drop(store);
    served.join().unwrap();
}

/// A write a commit follows is sealed by it, and a write a barrier precedes seals what came
/// before: each one round trip, as the store has them (`WriteBarriers`).
#[test]
fn writes_with_barriers() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let (first, second) = (
        ExtentId { branch: store.branch(), sequence: 1 },
        ExtentId { branch: store.branch(), sequence: 2 },
    );
    let data = pattern(1, BLOCK);
    let committed = WriteBarriers { before: false, after: Some(Commit::Durable) };
    store.write_with_barriers(first, 0, &[VolatileSlice::from(&data[..])], committed).unwrap();
    assert_eq!(write(&store, first, BLOCK as u64, &data), Err(Errno::EPERM), "first is open");
    write(&store, second, 0, &data).unwrap();
    let barrier = WriteBarriers { before: true, after: None };
    let third = ExtentId { branch: store.branch(), sequence: 3 };
    store.write_with_barriers(third, 0, &[VolatileSlice::from(&data[..])], barrier).unwrap();
    assert_eq!(write(&store, second, BLOCK as u64, &data), Err(Errno::EPERM), "second is open");
    write(&store, third, BLOCK as u64, &data).unwrap();
    assert_eq!(read(&store, first, 0, BLOCK), Ok(data));
    drop(store);
    served.join().unwrap();
}

/// The window is the daemon's, in this process's memory: a store to a writable mapping of a new
/// extent faults, which the daemon resolves, and the data is the extent's; sealed, the mapping is
/// read-only, of the sealed extent's pages, which another mapping of it maps too.
#[test]
fn maps_a_window_in_the_vm_process() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 24);
    let memory = memory(4);
    // SAFETY: The memory is the test's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, 4 * ps as u64, None) }.unwrap();
    let base = memory.as_ptr() as usize;
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.map(extent, 0, ps as u64, 0, true).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    unsafe { std::ptr::copy_nonoverlapping(pattern(3, ps).as_ptr(), memory.as_ptr(), ps) };
    assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(3, BLOCK)));

    store.commit(None).unwrap();
    store.map(extent, 0, ps as u64, 2 * ps as u64, false).unwrap();
    // SAFETY: As above.
    let window = unsafe { std::slice::from_raw_parts(memory.as_ptr(), 3 * ps) };
    assert!(window[..ps] == pattern(3, ps)[..], "the sealed extent holds other data");
    assert!(window[2 * ps..] == pattern(3, ps)[..], "its other mapping maps other data");
    assert_eq!(mapped_from(base + 2 * ps), mapped_from(base));
    assert_ne!(mapped_from(base).0, ANONYMOUS, "the window maps no file");
    // Unmapped, the page is gone from this process's memory once the daemon replies.
    store.unmap(2 * ps as u64, ps as u64).unwrap();
    assert_eq!(mapped_from(base + 2 * ps).0, ANONYMOUS);

    drop(store);
    served.join().unwrap();
    // SAFETY: Unmaps the memory, which nothing uses any more.
    assert_eq!(unsafe { libc::munmap(memory.as_ptr().cast(), 4 * ps) }, 0);
}

/// The environment a child process of [`two_processes_map_the_same_pages`] is told what to map in.
const CHILD_SOCKET: &str = "FXDAX_EXTENTD_TEST_SOCKET";
const CHILD_STORE: &str = "FXDAX_EXTENTD_TEST_STORE";
const CHILD_EXTENT: &str = "FXDAX_EXTENTD_TEST_EXTENT";

/// Maps `extent`'s first page into a new window of the store at `store` the daemon at `socket`
/// serves, checks it holds the pattern written, and returns the store, mapping it still, and where
/// the page is mapped from.
fn map_first_page(
    socket: &Path,
    store: &Path,
    extent: ExtentId,
) -> (RemoteExtentStore, (String, u64)) {
    let ps = page_size() as usize;
    // Once the daemon has seen a connection that had the store open go.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let store = loop {
        match RemoteExtentStore::connect(socket, store, 1 << 24) {
            Ok(store) => break store,
            Err(error)
                if error.errno() == Some(Errno::EBUSY) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("opening {}: {error:#}", store.display()),
        }
    };
    let memory = memory(1);
    // SAFETY: The memory is this function's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, ps as u64, None) }.unwrap();
    store.map(extent, 0, ps as u64, 0, false).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    let page = unsafe { std::slice::from_raw_parts(memory.as_ptr(), ps) };
    assert!(*page == pattern(9, ps)[..], "the window maps other data");
    // The memory is leaked: the store's window maps it until the process exits.
    (store, mapped_from(memory.as_ptr() as usize))
}

/// A VM process, for [`two_processes_map_the_same_pages`]: prints where it maps the extent from.
#[test]
#[ignore = "run by two_processes_map_the_same_pages"]
fn child_maps_an_extent() {
    let (Ok(socket), Ok(store), Ok(extent)) =
        (std::env::var(CHILD_SOCKET), std::env::var(CHILD_STORE), std::env::var(CHILD_EXTENT))
    else {
        return;
    };
    let (branch, sequence) = extent.split_once(':').unwrap();
    let extent = ExtentId { branch: branch.parse().unwrap(), sequence: sequence.parse().unwrap() };
    let (_store, (file, offset)) = map_first_page(Path::new(&socket), Path::new(&store), extent);
    println!("MAPPED {file} {offset}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    // As a VMM that dies does: with its store and window still open.
    std::process::exit(0);
}

/// Two VM processes, each of a store forked from one, mapping one extent of it at once map the
/// same pages: the daemon's, which cache it once.
#[test]
fn two_processes_map_the_same_pages() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (parent, child) = (dir.path().join("parent"), dir.path().join("child"));
    let extent = {
        let store = ExtentStore::create(&parent, 1 << 24).unwrap();
        let extent = ExtentId { branch: store.branch(), sequence: 1 };
        store.write(extent, 0, &[VolatileSlice::from(&pattern(9, ps)[..])]).unwrap();
        store.commit(None).unwrap();
        store.fork(&child, extent.branch + 1).unwrap();
        extent
    };
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let (_store, ours) = map_first_page(&socket, &parent, extent);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "extentd::tests::child_maps_an_extent", "--ignored", "--nocapture"])
        .env(CHILD_SOCKET, &socket)
        .env(CHILD_STORE, &child)
        .env(CHILD_EXTENT, format!("{}:{}", extent.branch, extent.sequence))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    let theirs = stdout
        .lines()
        .find_map(|line| line.strip_prefix("MAPPED "))
        .unwrap_or_else(|| panic!("the child mapped nothing: {stdout}"));
    let (file, offset) = theirs.rsplit_once(' ').unwrap();
    assert_eq!((file.to_string(), offset.parse::<u64>().unwrap()), ours);

    // The child's store goes with it, for another VM process to open, once the daemon has seen it
    // go.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match RemoteExtentStore::connect(&socket, &child, 1 << 24) {
            Ok(_) => break,
            Err(error) if std::time::Instant::now() < deadline => {
                assert!(format!("{error:#}").contains(&Errno::EBUSY.to_string()), "{error:#}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("the child's store is still open: {error:#}"),
        }
    }
}

/// A store the daemon serves forks to a new branch the daemon serves too, whose window maps the
/// pages the parent's maps: the daemon's counters count the maps, the faults and the reads. The
/// extent is paged in once, for both windows and the fork's READ, which reads nothing from the
/// store (TASK-32).
#[test]
fn the_daemon_forks_stores_it_serves() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (parent, child) = (dir.path().join("parent"), dir.path().join("child"));
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let before = crate::stats::stats();
    let store = RemoteExtentStore::connect(&socket, &parent, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(9, ps)).unwrap();
    store.commit(None).unwrap();
    let branch = extent.branch + 1;
    store.fork(&child, branch).unwrap();
    // A fork to a path that is there already fails.
    assert_eq!(store.fork(&child, branch + 1), Err(Errno::EIO));
    drop(store);
    let (ours, ours_from) = map_first_page(&socket, &parent, extent);
    drop(ours);

    let (theirs, theirs_from) = map_first_page(&socket, &child, extent);
    assert_eq!(theirs.branch(), branch);
    assert_eq!(theirs_from, ours_from);
    assert_eq!(read(&theirs, extent, 0, ps).unwrap(), pattern(9, ps));
    let counted = crate::stats::stats().since(&before);
    assert_eq!((counted.maps, counted.reads, counted.read_bytes), (2, 1, ps as u64));
    assert_eq!((counted.read_stored_bytes, counted.page_ins), (0, 1));
}

/// TASK-39.1: a VM's store forks, for a snapshot, only once nothing is open; the VMs restored from
/// the snapshot each open a fork of their own of that point, holding what it held, whose windows
/// map what the snapshot's recorded, mapped again; and a point a connection has open is busy.
#[test]
fn snapshots_restore_into_forks_of_their_point() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24).unwrap();
    let ours = memory(4);
    // SAFETY: The memory is the test's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(ours, 4 * ps as u64, None) }.unwrap();
    let (sealed, open) = (
        ExtentId { branch: store.branch(), sequence: 1 },
        ExtentId { branch: store.branch(), sequence: 2 },
    );
    write(&store, sealed, 0, &pattern(1, ps)).unwrap();
    store.commit(Some(b"root")).unwrap();
    store.map(sealed, 0, ps as u64, 0, false).unwrap();
    let zeros = crate::WindowMapping {
        window_offset: 2 * ps as u64,
        len: ps as u64,
        contents: crate::MappingContents::Zeros,
    };
    store.map_again(&zeros).unwrap();
    store.map(open, 0, ps as u64, 3 * ps as u64, true).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    unsafe {
        std::ptr::copy_nonoverlapping(pattern(2, ps).as_ptr(), ours.as_ptr().add(3 * ps), ps)
    };
    let point = dir.path().join("point");
    assert_eq!(store.fork_quiescent(&point, store.branch() + 1), Err(Errno::EBUSY));
    store.commit(None).unwrap();
    let point_branch = store.branch() + 1;
    store.fork_quiescent(&point, point_branch).unwrap();
    let mappings = store.window_mappings().unwrap();
    let extent =
        |extent, offset| crate::MappingContents::Extent { extent, offset, writable: false };
    assert_eq!(
        mappings,
        [
            crate::WindowMapping { window_offset: 0, len: ps as u64, contents: extent(sealed, 0) },
            zeros,
            crate::WindowMapping {
                window_offset: 3 * ps as u64,
                len: ps as u64,
                contents: extent(open, 0),
            },
        ]
    );
    // SAFETY: As above.
    let window =
        |memory: NonNull<u8>| unsafe { std::slice::from_raw_parts(memory.as_ptr(), 4 * ps) };
    let snapshotted = [0, 2, 3].map(|page| window(ours)[page * ps..][..ps].to_vec());

    let restored: Vec<RemoteExtentStore> = ["a", "b"]
        .into_iter()
        .map(|name| {
            let restored =
                RemoteExtentStore::connect_forked(&socket, dir.path().join(name), &point, 0, 0)
                    .unwrap();
            assert_eq!(restored.root().unwrap(), b"root");
            let memory = memory(4);
            // SAFETY: As above.
            unsafe { restored.create_dax_window_in_host_memory(memory, 4 * ps as u64, None) }
                .unwrap();
            for mapping in &mappings {
                restored.map_again(mapping).unwrap();
            }
            assert_eq!(restored.window_mappings().unwrap(), mappings);
            for (page, bytes) in [0, 2, 3].into_iter().zip(&snapshotted) {
                assert!(window(memory)[page * ps..][..ps] == bytes[..], "page {page} differs");
            }
            restored
        })
        .collect();
    let branches: Vec<u64> = restored.iter().map(RemoteExtentStore::branch).collect();
    assert!(!branches.contains(&store.branch()) && branches[0] != branches[1], "{branches:x?}");
    // Each writes on its own.
    let theirs = ExtentId { branch: branches[0], sequence: 1 };
    write(&restored[0], theirs, 0, &pattern(3, BLOCK)).unwrap();
    restored[0].commit(None).unwrap();
    assert!(read(&restored[1], theirs, 0, BLOCK).is_err());
    assert_eq!(read(&restored[1], open, 0, BLOCK), Ok(pattern(2, BLOCK)));

    // A point a connection has open is busy.
    let opened = RemoteExtentStore::connect(&socket, &point, 1 << 24).unwrap();
    let busy = RemoteExtentStore::connect_forked(&socket, dir.path().join("c"), &point, 0, 0);
    assert_eq!(busy.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    drop(opened);
}

/// TASK-39.1: a VM restored on another host than its snapshot's, whose point it doesn't have, takes
/// the point's branch from the daemon's bucket, and forks its store from that.
#[test]
fn snapshot_points_are_taken_from_the_bucket() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(4, BLOCK)).unwrap();
    store.commit(Some(b"root")).unwrap();
    let point_branch = store.branch() + 1;
    store.fork_quiescent(&dir.path().join("point"), point_branch).unwrap();
    drop(store);

    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let (moved, point) = (elsewhere.join("store"), elsewhere.join("point"));
    let restored =
        RemoteExtentStore::connect_forked(&socket, &moved, &point, point_branch, 1 << 24).unwrap();
    assert_eq!(restored.root().unwrap(), b"root");
    assert_eq!(read(&restored, extent, 0, BLOCK), Ok(pattern(4, BLOCK)));
    assert_ne!(restored.branch(), point_branch);
    assert!(point.exists(), "the point taken is a store here");
}

/// TASK-39.1: a store whose VM process just went opens at once for another, as a VM restored from
/// a snapshot of it does: its connection lets it go once its commits are durable.
#[test]
fn stores_open_once_their_vm_process_goes() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let path = dir.path().join("store");
    for round in 0..20 {
        let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
        let extent = ExtentId { branch: store.branch(), sequence: round + 1 };
        write(&store, extent, 0, &pattern(5, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        drop(store);
    }
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    assert_eq!(store.info().unwrap().next_sequence, 21);
}

/// TASK-39.2: a daemon with roots refuses to open, create or fork stores outside them.
#[test]
fn daemons_with_roots_serve_no_stores_outside_them() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let (ours, theirs) = (dir.path().join("ours"), dir.path().join("theirs"));
    std::fs::create_dir(&ours).unwrap();
    std::fs::create_dir(&theirs).unwrap();
    // SAFETY: as in host.
    unsafe { std::env::set_var("FXDAX_EXTENTD_ROOTS", &ours) };
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let refused = RemoteExtentStore::connect(&socket, theirs.join("store"), 1 << 24);
    assert_eq!(refused.err().and_then(|error| error.errno()), Some(Errno::EACCES));
    let store = RemoteExtentStore::connect(&socket, ours.join("store"), 1 << 24).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.fork(&theirs.join("fork"), store.branch() + 1), Err(Errno::EACCES));
    store.fork(&ours.join("fork"), store.branch() + 1).unwrap();
}

/// TASK-39.3: a store a VM process has open forks live through the daemon, from a connection of
/// its own: refused while an extent is open, as unless its guest's filesystem is frozen; once
/// committed, the fork holds the store as of then, and the store goes on as it was.
#[test]
fn stores_fork_live_once_nothing_is_open() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    write(&store, extent(1), 0, &pattern(1, BLOCK)).unwrap();
    store.commit(Some(b"before")).unwrap();
    write(&store, extent(2), 0, &pattern(2, BLOCK)).unwrap();

    let busy = fork_store(&path, &fork);
    assert_eq!(busy.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    store.commit(Some(b"at the fork")).unwrap();
    let branch = fork_store(&path, &fork).unwrap();
    // The store goes on, its writes its own.
    write(&store, extent(3), 0, &pattern(3, BLOCK)).unwrap();
    store.commit(Some(b"after")).unwrap();
    assert_eq!(read(&store, extent(3), 0, BLOCK), Ok(pattern(3, BLOCK)));

    let forked = RemoteExtentStore::connect(&socket, &fork, 1 << 24).unwrap();
    assert_eq!(forked.branch(), branch);
    assert_eq!(forked.root().unwrap(), b"at the fork");
    assert_eq!(read(&forked, extent(2), 0, BLOCK), Ok(pattern(2, BLOCK)));
    assert!(read(&forked, extent(3), 0, BLOCK).is_err());
}

/// TASK-39.4: a store is deleted through the daemon while a fork of it is open: refused while its
/// own VM process has it open; then its directory stays, holding what the fork shares, and no
/// longer opens, while the fork reads it and writes as before, until the fork is deleted too.
#[test]
fn stores_are_deleted_while_their_forks_are_open() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    let base = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, base, 0, &pattern(1, BLOCK)).unwrap();
    store.commit(Some(b"base")).unwrap();
    fork_store(&path, &fork).unwrap();
    let forked = RemoteExtentStore::connect(&socket, &fork, 1 << 24).unwrap();

    let busy = delete_store(&path);
    assert_eq!(busy.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    drop(store);
    delete_store(&path).unwrap();
    assert!(path.exists(), "the store's directory went while its fork lives");
    let deleted = RemoteExtentStore::connect(&socket, &path, 1 << 24);
    assert_eq!(deleted.err().map(|error| error.errno()), Some(Some(Errno::EIO)));
    assert_eq!(read(&forked, base, 0, BLOCK), Ok(pattern(1, BLOCK)));
    let own = ExtentId { branch: forked.branch(), sequence: 1 };
    write(&forked, own, 0, &pattern(2, BLOCK)).unwrap();
    forked.commit(Some(b"fork")).unwrap();

    let busy = delete_store(&fork);
    assert_eq!(busy.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    drop(forked);
    let forked = RemoteExtentStore::connect(&socket, &fork, 1 << 24).unwrap();
    assert_eq!(forked.root().unwrap(), b"fork");
    assert_eq!(read(&forked, base, 0, BLOCK), Ok(pattern(1, BLOCK)));
    drop(forked);
    delete_store(&fork).unwrap();
    assert!(!fork.exists() && !path.exists(), "the last branch gone, its directory goes");
    let missing = delete_store(&fork);
    assert_eq!(missing.err().and_then(|error| error.errno()), Some(Errno::ENOENT));
}

/// TASK-39.4: with a bucket, deleting a store through the daemon deletes its branch there too,
/// and the collector keeps what its fork needs of it.
#[test]
fn deleted_stores_branches_keep_what_their_forks_need() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    let base_branch = store.branch();
    let base = ExtentId { branch: base_branch, sequence: 1 };
    write(&store, base, 0, &pattern(3, 2 * BLOCK)).unwrap();
    store.commit(Some(b"base")).unwrap();
    let fork_branch = fork_store(&path, &fork).unwrap();
    drop(store);
    delete_store(&path).unwrap();

    let bucket = super::stores::remote_bucket().unwrap();
    futures::executor::block_on(async {
        use extent_remote::branch;
        let deleted = branch::record(&*bucket, base_branch).await.unwrap().unwrap();
        assert!(deleted.deleted);
        let record = branch::record(&*bucket, fork_branch).await.unwrap().unwrap();
        assert!(!record.deleted);
        let options = extent_remote::collector::Options { compact_below: 0.5, grace: 0 };
        extent_remote::collector::collect(&*bucket, 1, &options).await.unwrap();
        let state = extent_remote::recover(&*bucket, fork_branch, record.origin).await.unwrap();
        let id = storage_device::ExtentId::new(base.branch, base.sequence);
        let data = extent_remote::read(&*bucket, id, &state.state.extents[&id]).await.unwrap();
        assert_eq!(data, pattern(3, 2 * BLOCK));
    });
    let forked = RemoteExtentStore::connect(&socket, &fork, 1 << 24).unwrap();
    assert_eq!(forked.root().unwrap(), b"base");
    assert_eq!(read(&forked, base, 0, 2 * BLOCK), Ok(pattern(3, 2 * BLOCK)));
}

/// TASK-39.7: a store's copy placed at an earlier placement than the one that last took its branch
/// does not open (`ESTALE`): a placement cut off takes no branch back from the one after it.
#[test]
fn earlier_placements_open_no_stores() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (path, earlier) = (dir.path().join("store"), dir.path().join("earlier"));
    let branch = ExtentStore::create(&path, 1 << 24).unwrap().branch();
    ExtentStore::place(&path, 5).unwrap();
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    write(&store, ExtentId { branch, sequence: 1 }, 0, &pattern(1, BLOCK)).unwrap();
    store.commit(Some(b"placed at 5")).unwrap();
    drop(store);

    ExtentStore::create_for_branch(&earlier, 1 << 24, branch).unwrap();
    ExtentStore::place(&earlier, 3).unwrap();
    let refused = RemoteExtentStore::connect(&socket, &earlier, 1 << 24);
    assert_eq!(refused.err().and_then(|error| error.errno()), Some(Errno::ESTALE));
    ExtentStore::place(&earlier, 6).unwrap();
    let later = RemoteExtentStore::connect(&socket, &earlier, 1 << 24).unwrap();
    assert_eq!(later.root().unwrap(), b"placed at 5");
}

/// TASK-39.7: the daemon returns once a store's commits so far are in its bucket: a store a VM
/// process has open, and one none has.
#[test]
fn the_daemon_waits_for_uploads() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let path = dir.path().join("store");
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(2, BLOCK)).unwrap();
    store.commit_ordered(Some(b"ordered")).unwrap();
    wait_uploaded(&path).unwrap();
    let bucket = super::stores::remote_bucket().unwrap();
    let uploaded = || {
        futures::executor::block_on(extent_remote::recover(&*bucket, extent.branch, None))
            .unwrap()
            .state
            .root
    };
    assert_eq!(uploaded(), Some(b"ordered".to_vec()));
    store.commit_ordered(Some(b"closed")).unwrap();
    drop(store);
    wait_uploaded(&path).unwrap();
    assert_eq!(uploaded(), Some(b"closed".to_vec()));
}

/// TASK-45: the daemon keeps every store's branch in its bucket, and opens none without one.
#[test]
fn stores_open_only_with_a_bucket() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let refused = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24);
    assert_eq!(refused.err().and_then(|error| error.errno()), Some(Errno::EIO));
}

/// fxdax: a tenant's arena, and the extents cached in it, go with the tenant, once its grant and
/// its VM processes have (TASK-43.4).
#[test]
fn tenants_arenas_go_with_them() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let tenant = super::Tenant::new("acme", dir.path()).unwrap();
    let arena = tenant.arena();
    let (client, server) = UnixStream::pair().unwrap();
    let served = std::thread::spawn(move || serve_connection(server, Some(tenant)));
    let store =
        RemoteExtentStore::over(client, &dir.path().join("store"), Opening::Create(1 << 24))
            .unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&pattern(4, ps)[..])]).unwrap();
    store.commit(None).unwrap();
    let memory = memory(1);
    // SAFETY: The memory is this test's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, ps as u64, None) }.unwrap();
    store.map(extent, 0, ps as u64, 0, false).unwrap();
    // SAFETY: The window's memory, whose faults the store serves: the extent cached, in the arena.
    let page = unsafe { std::slice::from_raw_parts(memory.as_ptr(), ps) };
    assert!(*page == pattern(4, ps)[..], "the window maps other data");
    assert!(crate::kernel::arena_memory(arena).is_some());

    drop(store);
    served.join().unwrap();
    // Gone with the tenant, once the page-in that served the fault, which holds the extent a while
    // after the fault returns, lets it go: the arena retires until then.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while crate::kernel::arena_memory(arena).is_some() {
        assert!(std::time::Instant::now() < deadline, "the tenant's arena stayed");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// fxdax: followers read a store as of their view's commit beside its writer's connection, which
/// never waits for them: they hear of its commits, advance to them, hold what the commits between
/// held until they let go of them, and pin what their guest holds open; what the writer wrote
/// since they read as ENOENT, what they let go of as EIO, and what would change the store they
/// refuse with EROFS (backlog doc-8).
#[test]
fn followers_follow_their_writer_side_by_side() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let branch = store.branch();
    let extent = move |sequence| ExtentId { branch, sequence };
    write(&store, extent(1), 0, &pattern(1, BLOCK)).unwrap();
    store.commit(Some(b"one")).unwrap();
    let (follower, followed) = follow(&path).unwrap();
    let (other, other_followed) = follow(&path).unwrap();
    assert!(follower.is_read_only() && !store.is_read_only());
    assert_eq!((follower.branch(), follower.root()), (store.branch(), Ok(b"one".to_vec())));
    let commits = commits_heard(&follower);
    assert_eq!(read(&follower, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));

    // The writer goes on, the followers where they are.
    write(&store, extent(2), 0, &pattern(2, BLOCK)).unwrap();
    store.free(extent(1)).unwrap();
    store.commit(Some(b"two")).unwrap();
    let epoch = commits.recv().unwrap();
    assert_eq!(follower.committed(), epoch);
    assert_eq!(read(&follower, extent(2), 0, BLOCK), Err(Errno::ENOENT));
    assert_eq!(read(&other, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));
    assert_eq!(follower.advance(), Ok(epoch));
    assert_eq!(follower.root(), Ok(b"two".to_vec()));
    assert_eq!(read(&follower, extent(2), 0, BLOCK), Ok(pattern(2, BLOCK)));
    assert_eq!(read(&follower, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));
    follower.let_go(epoch).unwrap();
    assert_eq!(read(&follower, extent(1), 0, BLOCK), Err(Errno::EIO));
    follower.pin(extent(2)).unwrap();

    // The writer commits on, freeing what the followers hold, without waiting for them.
    store.free(extent(2)).unwrap();
    for sequence in 3..20 {
        write(&store, extent(sequence), 0, &pattern(sequence as u8, BLOCK)).unwrap();
        store.commit(None).unwrap();
    }
    assert_eq!(read(&other, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));
    let latest = follower.advance().unwrap();
    follower.let_go(latest).unwrap();
    assert_eq!(read(&follower, extent(2), 0, BLOCK), Ok(pattern(2, BLOCK)));
    assert_eq!(read(&follower, extent(19), 0, BLOCK), Ok(pattern(19, BLOCK)));
    follower.unpin(extent(2)).unwrap();
    assert_eq!(read(&follower, extent(2), 0, BLOCK), Err(Errno::EIO));
    assert_eq!(follower.committed(), latest);

    assert_eq!(write(&follower, extent(20), 0, &pattern(20, BLOCK)), Err(Errno::EROFS));
    assert_eq!(follower.free(extent(3)), Err(Errno::EROFS));
    assert_eq!(follower.commit(None), Err(Errno::EROFS));
    assert_eq!(follower.fork(&dir.path().join("fork"), 7), Err(Errno::EROFS));
    assert_eq!(store.advance(), Err(Errno::EINVAL));
    for (store, served) in [(follower, followed), (other, other_followed), (store, served)] {
        drop(store);
        served.join().unwrap();
    }
}

/// fxdax: followers of a store no writer has open follow it as of its last commit; a writer
/// opening it then takes it over, and they hear of its commits.
#[test]
fn writers_take_over_stores_their_followers_opened() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    assert_eq!(follow(&path).err(), Some(Some(Errno::ENOENT)));
    let (store, served) = connect(&path, 1 << 20);
    let branch = store.branch();
    let extent = move |sequence| ExtentId { branch, sequence };
    write(&store, extent(1), 0, &pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    drop(store);
    served.join().unwrap();

    let (follower, followed) = follow(&path).unwrap();
    let commits = commits_heard(&follower);
    assert_eq!(read(&follower, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));
    let (store, served) = connect(&path, 1 << 20);
    write(&store, extent(2), 0, &pattern(2, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(follower.advance(), Ok(commits.recv().unwrap()));
    assert_eq!(read(&follower, extent(2), 0, BLOCK), Ok(pattern(2, BLOCK)));

    // Once its writer goes, nothing open, a writer takes it over again.
    drop(store);
    served.join().unwrap();
    let (store, served) = connect(&path, 1 << 20);
    write(&store, extent(3), 0, &pattern(3, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(follower.advance(), Ok(commits.recv().unwrap()));
    assert_eq!(read(&follower, extent(3), 0, BLOCK), Ok(pattern(3, BLOCK)));
    assert_eq!(store.info().unwrap().next_sequence, 4);
    for (store, served) in [(follower, followed), (store, served)] {
        drop(store);
        served.join().unwrap();
    }
}

/// fxdax: a writer that goes with writes uncommitted cuts its followers off (ESTALE), and no
/// writer or follower opens its store until they are gone: then it opens as of its last commit.
#[test]
fn writers_that_go_with_writes_uncommitted_cut_their_followers_off() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let branch = store.branch();
    let extent = move |sequence| ExtentId { branch, sequence };
    write(&store, extent(1), 0, &pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let (follower, followed) = follow(&path).unwrap();
    write(&store, extent(2), 0, &pattern(2, BLOCK)).unwrap();
    drop(store);
    served.join().unwrap();

    assert_eq!(read(&follower, extent(1), 0, BLOCK), Err(Errno::ESTALE));
    assert_eq!(follower.advance(), Err(Errno::ESTALE));
    assert_eq!(follow(&path).err(), Some(Some(Errno::EBUSY)));
    let (client, server) = UnixStream::pair().unwrap();
    let busy = std::thread::spawn(move || serve_connection(server, None));
    let opened = RemoteExtentStore::over(client, &path, Opening::Create(1 << 20));
    assert_eq!(opened.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    busy.join().unwrap();

    drop(follower);
    followed.join().unwrap();
    let (store, served) = connect(&path, 1 << 20);
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(pattern(1, BLOCK)));
    assert_eq!(read(&store, extent(2), 0, BLOCK), Err(Errno::ENOENT));
    drop(store);
    served.join().unwrap();
}

/// fxdax: the daemon's work on a store followers have open: synced, grown and forked as it is;
/// deleted, placed or made public, not (EBUSY).
#[test]
fn stores_followed_are_worked_on_as_they_are() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    drop(store);
    served.join().unwrap();
    let (follower, followed) = follow(&path).unwrap();

    wait_uploaded(&path).unwrap();
    resize(&path, 2 << 20).unwrap();
    assert_eq!(resize(&path, 1 << 20), Err(Errno::EBUSY));
    assert_eq!(follower.info().unwrap().capacity, 2 << 20);
    let fork = dir.path().join("fork");
    fork_store(&path, &fork).unwrap();
    assert_eq!(delete_store(&path).err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    assert_eq!(control(super::stores::place(path.clone(), 2)), Err(Errno::EBUSY));
    assert_eq!(control(super::stores::make_public(path.clone())), Err(Errno::EBUSY));
    drop(follower);
    followed.join().unwrap();
    let forked = ExtentStore::open(&fork).unwrap();
    let mut data = vec![0; BLOCK];
    forked.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    assert_eq!(data, pattern(1, BLOCK));
    delete_store(&path).unwrap();
}

/// What this process holds, as Linux counts it: the daemon's threads (but for its clients'), its
/// open files, and its resident memory.
#[derive(Debug, Clone, Copy)]
struct Held {
    threads: usize,
    files: usize,
    resident: u64,
}

impl Held {
    fn now() -> Self {
        let threads = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter(|task| {
                let comm = task.as_ref().unwrap().path().join("comm");
                std::fs::read_to_string(comm).unwrap().trim() != "extentd client"
            })
            .count();
        let files = std::fs::read_dir("/proc/self/fd").unwrap().count();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|kib| kib.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .unwrap();
        Self { threads, files, resident: kib << 10 }
    }
}

/// This process's threads, by name, and how many of each.
fn thread_names() -> std::collections::BTreeMap<String, usize> {
    let mut names = std::collections::BTreeMap::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        let comm = std::fs::read_to_string(task.unwrap().path().join("comm")).unwrap();
        *names.entry(comm.trim().to_owned()).or_default() += 1;
    }
    names
}

/// TASK-34: the daemon serves 300 stores, forks of one base, open at once, each reading its base,
/// on threads they share: the daemon's threads do not grow with them, and each holds a few files
/// open and a little memory, the base's state and its files' handles shared.
#[test]
fn serves_hundreds_of_forks_of_a_base() {
    const FORKS: usize = 300;
    const EXTENTS: u64 = 64;
    host();
    // As the daemon does.
    super::raise_open_files_limit().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    let (store, served) = connect(&base, 1 << 30);
    let branch = store.branch();
    for sequence in 0..EXTENTS {
        let extent = ExtentId { branch, sequence };
        write(&store, extent, 0, &pattern(sequence as u8, 16 * BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    drop(store);
    served.join().unwrap();
    let forks: Vec<_> = (0..FORKS)
        .map(|n| {
            let fork = dir.path().join(format!("fork-{n}"));
            fork_store(&base, &fork).unwrap();
            fork
        })
        .collect();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let before = Held::now();
    let stores: Vec<_> = (forks.iter())
        .map(|fork| RemoteExtentStore::connect(&socket, fork, 1 << 30).unwrap())
        .collect();
    for (n, store) in stores.iter().enumerate() {
        let sequence = n as u64 % EXTENTS;
        let extent = ExtentId { branch, sequence };
        assert_eq!(read(store, extent, 0, BLOCK).unwrap(), pattern(sequence as u8, BLOCK));
    }
    let after = Held::now();
    let files = (after.files - before.files) as f64 / FORKS as f64;
    let resident = after.resident.saturating_sub(before.resident) / FORKS as u64;
    println!("{FORKS} forks open: before {before:?}, after {after:?}");
    println!("per fork: {files:.1} files, {} KiB resident", resident >> 10);
    // The daemon's threads: its runtimes', and the requests' running, not a set per connection.
    assert!(after.threads < 64, "{} threads: {:?}", after.threads, thread_names());
    // Each fork's: the connection's two ends here, and the client's, with its kernel's memory;
    // its store's index and open segment. And those the forks share: the files of the base's
    // segments they read, the most `Options::open_files` (64) of them.
    let opened = after.files - before.files;
    assert!(opened <= 8 * FORKS + 64, "{opened} files for {FORKS} forks");
    assert!(resident < 1 << 20, "{resident} bytes a fork");
    drop(stores);
}

/// TASK-47: the daemon's metrics, scraped over HTTP as Prometheus scrapes them, count each kind of
/// request as it is served, and say how many VM processes are connected and stores open.
#[test]
fn metrics_count_requests_as_they_are_served() {
    use std::io::{Read as _, Write as _};
    host();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || super::serve_metrics(listener));
    let get = |path: &str| {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: extentd\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let metric = |metrics: &str, name: &str| -> u64 {
        let line = (metrics.lines())
            .find(|line| line.split_once(' ').is_some_and(|(found, _)| found == name))
            .unwrap_or_else(|| panic!("no {name} in {metrics}"));
        line.rsplit_once(' ').unwrap().1.parse().unwrap()
    };
    let before = get("/metrics");
    assert!(before.starts_with("HTTP/1.1 200 OK\r\n"), "{before}");
    assert_eq!(metric(&before, "fxdax_extentd_requests_total{kind=\"write\"}"), 0);
    assert_eq!(metric(&before, "fxdax_extentd_connections"), 0);

    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 20);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    for offset in 0..3 {
        write(&store, extent, offset * BLOCK as u64, &pattern(1, BLOCK)).unwrap();
    }
    assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(1, BLOCK)));
    let metrics = get("/metrics");
    let requests =
        |kind: &str| metric(&metrics, &format!("fxdax_extentd_requests_total{{kind=\"{kind}\"}}"));
    assert_eq!(
        [requests("open"), requests("transfer"), requests("write"), requests("read")],
        [1, 1, 3, 1]
    );
    let histogram = "fxdax_extentd_request_duration_seconds";
    assert_eq!(metric(&metrics, &format!("{histogram}_count{{kind=\"write\"}}")), 3);
    assert_eq!(metric(&metrics, &format!("{histogram}_bucket{{kind=\"write\",le=\"+Inf\"}}")), 3);
    assert_eq!(metric(&metrics, "fxdax_extentd_request_errors_total{kind=\"write\"}"), 0);
    assert_eq!(metric(&metrics, "fxdax_extentd_connections"), 1);
    assert_eq!(metric(&metrics, "fxdax_extentd_stores_open"), 1);
    assert!(get("/other").starts_with("HTTP/1.1 404 Not Found\r\n"));
    drop(store);
    served.join().unwrap();
}
