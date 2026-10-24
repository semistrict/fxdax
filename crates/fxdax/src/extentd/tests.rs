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

/// The daemon's log, on standard error, which nextest shows of a test that fails.
struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{} {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Makes this process a host of the multi-node system, as the daemon's must be: its stores'
/// branches in a bucket in its memory, as of placement 1; its log on standard error.
fn host() {
    if log::set_logger(&Stderr).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
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
        let data = extent_remote::read(&bucket, id, &state.extents[&id]).await.unwrap();
        assert_eq!(data, pattern(1, 2 * BLOCK));
    });
    drop(store);
    served.join().unwrap();
}

/// fxdax: the daemon keeps what its bucket did, its requests and bytes, what the stores'
/// followers are, how many and how far behind, what serving the stores cost, and the requests it
/// served, in the file `FXDAX_EXTENTD_STATS` names, soon after each change: the bench reports it
/// (`scripts/bench-report.py`).
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
    let fields = |kept: &str, at: usize, kind: &str| -> Option<Vec<(String, u64)>> {
        let line = kept.lines().nth(at)?.strip_prefix(kind)?.strip_prefix(": ")?;
        let field =
            |field: &str| field.split_once('=').map(|(k, v)| (k.to_owned(), v.parse().unwrap()));
        line.split(' ').map(field).collect()
    };
    let count = |fields: &[(String, u64)], name: &str| {
        fields.iter().find(|(named, _)| named == name).map(|(_, count)| *count)
    };
    let start = std::time::Instant::now();
    let kept = loop {
        let kept = std::fs::read_to_string(&file).unwrap_or_default();
        let behind = fields(&kept, 1, "followers").and_then(|f| count(&f, "max-behind"));
        let commits = fields(&kept, 2, "daemon").and_then(|f| count(&f, "commits"));
        if kept.lines().next() == Some(&bucket.stats().line())
            && behind == Some(2)
            && commits == Some(3)
        {
            break kept;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "kept {kept:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let followers = fields(&kept, 1, "followers").unwrap();
    let names: Vec<_> = followers.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["views", "max-views", "max-behind", "max-stale-ms", "max-kept"]);
    let counts = (followers[0].1, followers[1].1, followers[4].1);
    assert_eq!(counts, (1, 1, 2 * BLOCK as u64), "{kept}");
    let daemon = fields(&kept, 2, "daemon").unwrap();
    let names: Vec<_> = daemon.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, crate::stats::stats().named().map(|(name, _)| name), "{kept}");
    // The three commits, each a COMMIT request: a request's time holds its commit's, which
    // holds its phases'.
    let requests = fields(&kept, 3, "requests").unwrap();
    assert_eq!(count(&requests, "commit"), Some(3), "{kept}");
    let ns = |fields: &[(String, u64)], name: &str| count(fields, name).unwrap();
    assert!(ns(&requests, "commit-ns") >= ns(&daemon, "commit-ns"), "{kept}");
    let phases = ["commit-write-ns", "commit-store-ns", "commit-seal-ns", "commit-wait-ns"];
    assert!(
        ns(&daemon, "commit-ns") >= phases.map(|phase| ns(&daemon, phase)).iter().sum::<u64>(),
        "{kept}"
    );
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
        let asking = tokio::task::spawn_blocking(move || (asked.recv().unwrap(), asked));
        let asked = tokio::select! {
            shrunk = client.resize_store(request) => panic!("{shrunk:?}"),
            asked = asking => asked.unwrap(),
        };
        assert_eq!(asked.0, (1 << 20, true));
        // Dropped, the call is reset, and the daemon drops its wait.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while connection.shrink_awaited() {
            assert!(tokio::time::Instant::now() < deadline, "the shrink still waits");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // TASK-79.41: and tells the guest the capacity the store keeps.
        let told = tokio::task::spawn_blocking(move || asked.1.recv().unwrap()).await.unwrap();
        assert_eq!(told, (2 << 20, false));
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

/// fxdax: a future spawned on replies sends its request on the thread spawning it, and goes on,
/// as the reply comes, on the thread receiving it, which no other thread then waits for (TASK-41);
/// its next request too. The thread receiving replies is held, in the store's capacity callback,
/// until the future waits for its reply: otherwise the reply may come while the spawning thread
/// still runs it, which then goes on with it.
#[test]
fn futures_go_on_where_their_replies_arrive() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let (holding, held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    store.on_capacity(move |_, _| {
        holding.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
    });
    resize(&path, 4 << 20).unwrap();
    held.recv().unwrap();

    let store = std::sync::Arc::new(store);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    let (done, finished) = std::sync::mpsc::channel();
    let serving = store.clone();
    super::spawn_on_replies(async move {
        let data = pattern(7, BLOCK);
        let committed = WriteBarriers { before: false, after: Some(Commit::Ordered) };
        let wrote = serving
            .write_with_barriers_async(extent, 0, &[VolatileSlice::from(&data[..])], committed)
            .await;
        let after_write = std::thread::current();
        let mut read = vec![0; BLOCK];
        let mut into = [VolatileSliceMut::from(&mut read[..])];
        let read_result = serving.read_async(extent, 0, &mut into).await;
        let after_read = std::thread::current();
        done.send((wrote, read_result, read == data, after_write, after_read)).unwrap();
    });
    release.send(()).unwrap();
    let (wrote, read, same, after_write, after_read) = finished.recv().unwrap();
    assert_eq!((wrote, read, same), (Ok(()), Ok(()), true));
    assert_eq!(after_write.name(), Some("extentd client"));
    assert_eq!(after_read.id(), after_write.id());
    drop(store);
    served.join().unwrap();
}

/// fxdax: a store's blocking call on the thread receiving its replies would wait for itself: it
/// panics, as from a callback of the store's there.
#[test]
fn blocking_calls_where_replies_arrive_panic() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let store = std::sync::Arc::new(store);
    let (answer, answered) = std::sync::mpsc::channel();
    let calling = std::sync::Arc::downgrade(&store);
    store.on_capacity(move |_, _| {
        let calling = calling.upgrade().unwrap();
        let info = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| calling.info()));
        let refusal =
            info.err().and_then(|panic| panic.downcast_ref::<&str>().map(|s| s.to_string()));
        answer.send(refusal).unwrap();
    });
    resize(&path, 4 << 20).unwrap();
    let refusal = answered.recv().unwrap();
    assert_eq!(
        refusal.as_deref(),
        Some("a store's request waited for on a thread receiving replies")
    );
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

/// The environment a child process of [`two_processes_map_the_same_pages`] is told what to map in;
/// and, if set, that it runs on once it has, until its input closes.
const CHILD_SOCKET: &str = "FXDAX_EXTENTD_TEST_SOCKET";
const CHILD_STORE: &str = "FXDAX_EXTENTD_TEST_STORE";
const CHILD_EXTENT: &str = "FXDAX_EXTENTD_TEST_EXTENT";
const CHILD_RUNS_ON: &str = "FXDAX_EXTENTD_TEST_RUNS_ON";

/// Maps `extent`'s first page into a new window of the store at `store` the daemon at `socket`
/// serves, checks it holds the pattern written, and returns the store, mapping it still, and the
/// page's address.
fn map_first_page(socket: &Path, store: &Path, extent: ExtentId) -> (RemoteExtentStore, usize) {
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
    (store, memory.as_ptr() as usize)
}

/// A VM process, for [`two_processes_map_the_same_pages`] and
/// [`frozen_vm_processes_hold_nothing_up`]: prints where it maps the extent from; then exits, or,
/// told to run on, runs on until its input closes.
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
    let (_store, page) = map_first_page(Path::new(&socket), Path::new(&store), extent);
    let (file, offset) = mapped_from(page);
    println!("MAPPED {file} {offset}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    if std::env::var_os(CHILD_RUNS_ON).is_some() {
        std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
    }
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

    let (_store, page) = map_first_page(&socket, &parent, extent);
    let ours = mapped_from(page);
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

/// What `work` returns, run on a thread of its own, unless it takes longer than `wait`: what a
/// test that would otherwise hang for good waits for.
fn within<T: Send + 'static>(
    wait: std::time::Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Unheard if the test stopped waiting.
        let _ = done.send(work());
    });
    finished.recv_timeout(wait)
}

/// A child process, killed and reaped once dropped, as a test that fails leaves it.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Has this process's threads, those it makes and the processes it starts find the system call
/// `call` failing with `errno` where its arguments are as `args` say, each the index of one and
/// the low word it is: as a kernel without what the call asks for, or a daemon without the right
/// to it, answers, for tests of the daemon on such hosts. For as long as the process lives, which
/// nextest gives each test of its own.
fn refuse_system_call(call: libc::c_long, args: &[(usize, u32)], errno: u16) {
    use libc::{BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, sock_filter};
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc000_00b7;
    let load = |offset: usize| sock_filter {
        code: (BPF_LD | BPF_W | BPF_ABS) as u16,
        jt: 0,
        jf: 0,
        k: offset as u32,
    };
    // Each test, unless it holds, jumps to the last instruction, which allows the call.
    let tests = 2 + args.len();
    let mut program = Vec::new();
    let test = |program: &mut Vec<sock_filter>, offset: usize, value: u32| {
        program.push(load(offset));
        let left = 2 * tests - program.len();
        let skip = u8::try_from(left).expect("a short filter");
        program.push(sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 0,
            jf: skip,
            k: value,
        });
    };
    test(&mut program, std::mem::offset_of!(libc::seccomp_data, arch), ARCH);
    test(&mut program, std::mem::offset_of!(libc::seccomp_data, nr), call as u32);
    for (arg, value) in args {
        // The argument's low word, which comes first, little-endian.
        test(&mut program, std::mem::offset_of!(libc::seccomp_data, args) + 8 * arg, *value);
    }
    let ret = |k: u32| sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k };
    program.push(ret(libc::SECCOMP_RET_ERRNO | u32::from(errno)));
    program.push(ret(libc::SECCOMP_RET_ALLOW));
    let filter = libc::sock_fprog { len: program.len() as u16, filter: program.as_mut_ptr() };
    // SAFETY: Plain system calls: the filter, which they copy, outlives them.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        let set = libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_TSYNC,
            &filter,
        );
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
    }
}

/// The page of the daemon's kernel's physical memory at `offset` in the memory of `arena`, as a VM
/// process maps it from its tenant's arena.
fn page_at(arena: crate::kernel::ArenaId, offset: u64) -> zircon::vm::page::VmPagePtr {
    use zircon::kernel::types::PAddr;
    use zircon::vm::physmap::{CHUNK_SIZE, PHYS_BASE};
    let memory = zircon::vm::pmm::node().phys_memory();
    let chunk_at = |chunk: usize| PAddr(PHYS_BASE.0 + chunk * CHUNK_SIZE);
    let within = offset % CHUNK_SIZE as u64;
    let chunk = (0..memory.chunk_count())
        .find(|chunk| {
            memory.chunk_arena(*chunk) == Some(arena)
                && memory.location(chunk_at(*chunk)).offset == offset - within
        })
        .unwrap_or_else(|| panic!("no chunk of {arena:?} holds {offset:#x}"));
    let paddr = PAddr(chunk_at(chunk).0 + within as usize);
    zircon::vm::pmm::paddr_to_vm_page(paddr).expect("a page of physical memory")
}

/// TASK-79.9: a VM process that stops answering, frozen as SIGSTOP or a cgroup's freezer freezes
/// it, holds nothing up: the daemon takes its access to the pages it maps away without waiting for
/// it, as it evicts them, as the kernel's relief does, while another VM process's requests and
/// faults on them are served; and once it has acknowledged nothing it was sent for a while, the
/// daemon kills it, and lets its store go. The page it maps, evicted, is no other's until then: not
/// free, but the quarantine's, until it has exited; free once it has.
#[test]
fn frozen_vm_processes_hold_nothing_up() {
    frozen_vm_processes_are_killed();
}

/// TASK-79.9: as [`frozen_vm_processes_hold_nothing_up`], on a kernel without `SO_PEERPIDFD`
/// (Linux 6.1 to 6.4), as the system call is refused here: the daemon kills the frozen VM process,
/// which it knows by the process that connected, and keeps the page it maps until it has exited.
#[test]
fn frozen_vm_processes_are_killed_without_peer_pidfds() {
    let (level, option) = (libc::SOL_SOCKET as u32, libc::SO_PEERPIDFD as u32);
    refuse_system_call(libc::SYS_getsockopt, &[(1, level), (2, option)], libc::ENOPROTOOPT as u16);
    frozen_vm_processes_are_killed();
}

/// What [`frozen_vm_processes_hold_nothing_up`] shows.
fn frozen_vm_processes_are_killed() {
    use std::io::BufRead as _;
    use std::os::unix::process::ExitStatusExt as _;
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    // A tenant's VM processes, whose pages are of its arena alone.
    let tenant = super::server::Tenant::new("frozen", dir.path()).unwrap();
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
    let serving = tenant.clone();
    std::thread::spawn(move || super::server::serve_tenant(listener, serving));
    let (ours, page) = map_first_page(&socket, &parent, extent);

    let mut theirs = Reaped(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "extentd::tests::child_maps_an_extent", "--ignored", "--nocapture"])
            .env(CHILD_SOCKET, &socket)
            .env(CHILD_STORE, &child)
            .env(CHILD_EXTENT, format!("{}:{}", extent.branch, extent.sequence))
            .env(CHILD_RUNS_ON, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = std::io::BufReader::new(theirs.0.stdout.take().unwrap());
    let mapped = stdout.lines().map(Result::unwrap).find(|line| line.starts_with("MAPPED "));
    let (file, offset) = mapped_from(page);
    assert_eq!(mapped, Some(format!("MAPPED {file} {offset}")));
    // SAFETY: Signals the child this test made.
    assert_eq!(unsafe { libc::kill(theirs.0.id() as libc::pid_t, libc::SIGSTOP) }, 0);

    // The extent's page evicted: its mappings taken away, ours and the frozen child's.
    let mapped = page_at(tenant.arena(), offset);
    let wait = std::time::Duration::from_secs(10);
    let evicted = within(wait, || crate::extents::evict_least_used(None, |_| false));
    assert_eq!(evicted, Ok(ps as u64), "the eviction waited for the frozen VM process");
    assert!(!mapped.is_free(), "the page the frozen VM process maps is free");
    let ours = std::sync::Arc::new(ours);
    let reading = ours.clone();
    let served = within(wait, move || read(&reading, extent, 0, BLOCK));
    assert_eq!(served, Ok(Ok(pattern(9, BLOCK))), "our read waited for the frozen VM process");
    // SAFETY: The window's memory, whose faults our store serves.
    let loaded =
        within(wait, move || unsafe { std::slice::from_raw_parts(page as *const u8, ps) }.to_vec());
    assert_eq!(loaded, Ok(pattern(9, ps)), "our fault waited for the frozen VM process");

    // Killed, once it has acknowledged nothing for a while.
    let killed = within(std::time::Duration::from_secs(30), move || theirs.0.wait().unwrap());
    let status = killed.expect("the frozen VM process lives on");
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status}");
    // Its store goes with it, for another VM process to open once the daemon lets it go, which
    // the opening waits for (`CLOSING_WAIT`); and its page, free by then.
    let reopened = RemoteExtentStore::connect(&socket, &child, 1 << 24).map(|store| store.branch());
    assert_eq!(reopened.map_err(|error| error.errno()), Ok(extent.branch + 1));
    assert!(mapped.is_free(), "the page the frozen VM process mapped isn't free once it's gone");
}

/// A VM process, for [`vm_processes_the_daemon_may_not_kill_are_refused`]: opens the store
/// [`CHILD_STORE`] names through the daemon at [`CHILD_SOCKET`], and says whether it did.
#[test]
#[ignore = "run by vm_processes_the_daemon_may_not_kill_are_refused"]
fn child_opens_a_store() {
    let (Ok(socket), Ok(store)) = (std::env::var(CHILD_SOCKET), std::env::var(CHILD_STORE)) else {
        return;
    };
    match RemoteExtentStore::connect(socket, store, 1 << 24) {
        Ok(_) => println!("OPENED"),
        Err(error) => println!("REFUSED {:?}", error.errno()),
    }
}

/// TASK-79.9: a VM process the daemon may not kill, as it may not signal it (`EPERM`, as a daemon
/// of another user without `CAP_KILL` may not, which the system call refused here stands for), it
/// refuses as it connects: one that stopped answering it could not let go of, and the pages it may
/// reach, freed, would wait for it for good.
#[test]
fn vm_processes_the_daemon_may_not_kill_are_refused() {
    refuse_system_call(libc::SYS_pidfd_send_signal, &[], libc::EPERM as u16);
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "extentd::tests::child_opens_a_store", "--ignored", "--nocapture"])
        .env(CHILD_SOCKET, &socket)
        .env(CHILD_STORE, dir.path().join("store"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let said =
        stdout.lines().find(|line| line.starts_with("OPENED") || line.starts_with("REFUSED"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(said, Some("REFUSED Some(Errno(EIO))"), "{stdout}{stderr}");
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
    // A fork to a path that is there already fails, as making a store there does (TASK-79.83).
    assert_eq!(store.fork(&child, branch + 1), Err(Errno::EEXIST));
    drop(store);
    let (ours, page) = map_first_page(&socket, &parent, extent);
    let ours_from = mapped_from(page);
    drop(ours);

    let (theirs, page) = map_first_page(&socket, &child, extent);
    assert_eq!(theirs.branch(), branch);
    assert_eq!(mapped_from(page), ours_from);
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
        let data = extent_remote::read(&bucket, id, &state.state.extents[&id]).await.unwrap();
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

/// The extents the cleaning tests write: 16 of them a store's capacity.
const EXTENT: usize = 64 * BLOCK;

/// Writes `store`'s extents `sequences`, of `EXTENT` bytes each, their own data, which does not
/// compress: what of it no extent is in takes as much of the host's disk as it is, which cleaning
/// counts (TASK-79.2).
fn write_extents(store: &RemoteExtentStore, sequences: std::ops::RangeInclusive<u64>) {
    for sequence in sequences {
        let extent = ExtentId { branch: store.branch(), sequence };
        write(store, extent, 0, &crate::test_data::noise(sequence, EXTENT)).unwrap();
    }
}

/// Frees `store`'s extents `sequences`.
fn free_extents(store: &RemoteExtentStore, sequences: std::ops::RangeInclusive<u64>) {
    for sequence in sequences {
        store.free(ExtentId { branch: store.branch(), sequence }).unwrap();
    }
}

/// Waits for the uploader of the store at `path`, which a connection has open, to have uploaded
/// its latest commit as its interval ran out, and let go of what the commits up to it freed: as
/// the daemon's control API says (`StatStore`), which makes nothing durable, as waiting for the
/// uploads would (`SyncStore`).
fn wait_for_uploader(path: &Path) {
    control(async {
        loop {
            let state = super::stores::stat(path.to_owned()).await.unwrap();
            if state.upload.unwrap().uploaded == state.committed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
}

/// Waits for `bytes` to be in the files of the open segments of `branch` of the store at `path`
/// (`open-segments/<branch>/<number>`, doc-7), as cleaning's copies write them, and returns what
/// those hold then. It waits 10 s at most, ample for a pass, which begins within a turn of what
/// made it due: so a daemon whose passes waited for the guest's next commit, which the tests make
/// none of first, fails them rather than hangs them.
fn wait_for_open_segments(path: &Path, branch: u64, bytes: u64) -> u64 {
    let open = path.join("open-segments").join(format!("{branch:016x}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let entries = std::fs::read_dir(&open).unwrap();
        let held = entries.map(|entry| entry.unwrap().metadata().unwrap().len()).sum();
        if held >= bytes {
            return held;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{held} bytes in the open segments, not {bytes}, 10 s on, no commit since"
        );
        std::thread::yield_now();
    }
}

/// The dead space of the store at `path`, which a connection has open
/// ([`ExtentStore::dead_bytes`]): the bytes of its segments' data no extent is in, which cleaning
/// takes back, but those another branch may hold extents in, as a fork's.
fn dead_bytes(path: &Path) -> u64 {
    let connection = super::stores::open_paths().holder(&super::stores::store_key(path)).unwrap();
    connection.store().unwrap().dead_bytes().unwrap()
}

/// TASK-79.2: the daemon cleans a store behind its guest's commits. What the guest frees of what
/// it committed, the store keeps for its uploader, until the uploader has uploaded the commit
/// that freed it: let go of then, between the guest's commits, it is dead space, the host's to
/// take back, which takes none of the guest's room, as the guest can neither make room for it nor
/// know of it. Once it is past the capacity's worth, a pass copies what is live of the segments it
/// is in, with no commit of the guest's, and the guest's next commit lets those segments go.
#[test]
fn stores_are_cleaned_behind_their_guests_commits() {
    let extent_bytes = EXTENT as u64;
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 16 * extent_bytes);
    let branch = store.branch();
    let info = || store.info().unwrap();
    // A segment of the capacity's worth, freed but its first extent; another of all but one of
    // what that left room for, freed but its first too, the last frees ordered.
    write_extents(&store, 1..=16);
    store.commit(None).unwrap();
    free_extents(&store, 2..=16);
    write_extents(&store, 17..=31);
    store.commit(None).unwrap();
    free_extents(&store, 18..=31);
    store.commit_ordered(None).unwrap();
    // 29 extents' worth no extent holds, 13 of them past the capacity's: the guest's room is what
    // its two extents leave.
    wait_for_uploader(&path);
    assert_eq!(dead_bytes(&path), 29 * extent_bytes);
    assert_eq!(info().available, 14 * extent_bytes);
    // The two extents live copied, no commit having come since: the dead space stays until one
    // comes, and the guest's writes have their room meanwhile.
    assert_eq!(wait_for_open_segments(&path, branch, 2 * extent_bytes), 2 * extent_bytes);
    assert_eq!(dead_bytes(&path), 29 * extent_bytes);
    assert_eq!(info().available, 14 * extent_bytes);
    write(&store, ExtentId { branch, sequence: 32 }, 0, &pattern(32, 2 * EXTENT)).unwrap();
    // The commit makes the copies their extents' places: the segments no extent is in any more
    // go, and their dead space with them.
    store.commit(None).unwrap();
    assert_eq!(dead_bytes(&path), 0);
    assert_eq!(info().available, 12 * extent_bytes);
    for sequence in [1, 17] {
        let extent = ExtentId { branch, sequence };
        assert_eq!(read(&store, extent, 0, EXTENT), Ok(crate::test_data::noise(sequence, EXTENT)));
    }
    drop(store);
    served.join().unwrap();
}

/// TASK-79.2: a fork deleted leaves the store it was forked from what that store freed since in
/// the segments they shared, which the fork held, as dead space of its own, which takes none of
/// its guest's room either: once the store's dead space in all is past the capacity's worth, a
/// pass copies what is live of the segments it is in, with no commit of the store's guest, and the
/// guest's next commit lets them go.
#[test]
fn stores_are_cleaned_once_the_forks_sharing_their_segments_go() {
    let extent_bytes = EXTENT as u64;
    host();
    let dir = tempfile::tempdir().unwrap();
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let (store, served) = connect(&path, 16 * extent_bytes);
    let branch = store.branch();
    let info = || store.info().unwrap();
    // As above, the fork holding the first segment.
    write_extents(&store, 1..=16);
    store.commit(None).unwrap();
    store.fork(&fork, branch + 1).unwrap();
    free_extents(&store, 2..=16);
    write_extents(&store, 17..=31);
    store.commit(None).unwrap();
    free_extents(&store, 18..=31);
    store.commit_ordered(None).unwrap();
    // 14 extents' worth no extent holds, within the capacity's worth: what the store freed of
    // those the fork holds is none of its own.
    wait_for_uploader(&path);
    assert_eq!(dead_bytes(&path), 14 * extent_bytes);
    assert_eq!(info().available, 14 * extent_bytes);
    delete_store(&fork).unwrap();
    // The two extents live copied, no commit having come since: the fork gone, 29 extents' worth
    // no extent holds, 13 past the capacity's, which stay until one comes.
    assert_eq!(wait_for_open_segments(&path, branch, 2 * extent_bytes), 2 * extent_bytes);
    assert_eq!(dead_bytes(&path), 29 * extent_bytes);
    assert_eq!(info().available, 14 * extent_bytes);
    // The commit makes the copies their extents' places: the segments no extent is in any more
    // go, and their dead space with them.
    store.commit(None).unwrap();
    assert_eq!(dead_bytes(&path), 0);
    assert_eq!(info().available, 14 * extent_bytes);
    for sequence in [1, 17] {
        let extent = ExtentId { branch, sequence };
        assert_eq!(read(&store, extent, 0, EXTENT), Ok(crate::test_data::noise(sequence, EXTENT)));
    }
    drop(store);
    served.join().unwrap();
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

/// fxdax: a connection opens no store open in an arena not its own (TASK-79.36), whose pages its
/// VM process's window could not map, or not as it maps them: neither a follower of no tenant's a
/// store a tenant's writer has open, nor a tenant's writer one that followers of no tenant's have
/// open (EBUSY), until the other is gone.
#[test]
fn stores_open_in_another_arena_are_busy() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let tenant = super::Tenant::new("acme", dir.path()).unwrap();
    let open_as_tenant = || {
        let (client, server) = UnixStream::pair().unwrap();
        let tenant = tenant.clone();
        let served = std::thread::spawn(move || serve_connection(server, Some(tenant)));
        (RemoteExtentStore::over(client, &path, Opening::Create(1 << 20)), served)
    };
    let (store, served) = open_as_tenant();
    let store = store.unwrap();
    write(&store, ExtentId { branch: store.branch(), sequence: 1 }, 0, &pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(follow(&path).err(), Some(Some(Errno::EBUSY)));
    drop(store);
    served.join().unwrap();

    let (follower, followed) = follow(&path).unwrap();
    let (opened, busy) = open_as_tenant();
    assert_eq!(opened.err().and_then(|error| error.errno()), Some(Errno::EBUSY));
    busy.join().unwrap();
    drop(follower);
    followed.join().unwrap();
    let (store, served) = open_as_tenant();
    drop(store.unwrap());
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
        let threads = thread_comms().filter(|comm| comm != "extentd client").count();
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

/// The names of this process's threads alive as they are read: one that ends between the listing of
/// `/proc/self/task` and the read of its name, as an idle blocking thread of the daemon's runtimes
/// does, is gone, and holds nothing: its name not found, or, as it exits, its process no more
/// (`ESRCH`), as for the threads a store's flush syncs its files on (`extent_store`'s `AT_ONCE`).
fn thread_comms() -> impl Iterator<Item = String> {
    std::fs::read_dir("/proc/self/task").unwrap().filter_map(|task| {
        match std::fs::read_to_string(task.unwrap().path().join("comm")) {
            Ok(comm) => Some(comm.trim().to_owned()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) if err.raw_os_error() == Some(libc::ESRCH) => None,
            Err(err) => panic!("reading a thread's name: {err}"),
        }
    })
}

/// This process's threads, by name, and how many of each.
fn thread_names() -> std::collections::BTreeMap<String, usize> {
    let mut names = std::collections::BTreeMap::new();
    for comm in thread_comms() {
        *names.entry(comm).or_default() += 1;
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
    // Each fork's: the connection's two ends here, and the client's, with its kernel's memory, its
    // arena and the default one, read-only (TASK-79.17); its store's index and open segment. And
    // those the forks share: the files of the base's segments they read, the most
    // `Options::open_files` (64) of them.
    let opened = after.files - before.files;
    assert!(opened <= 9 * FORKS + 64, "{opened} files for {FORKS} forks");
    assert!(resident < 1 << 20, "{resident} bytes a fork");
    drop(stores);
}

/// TASK-92.17: what the daemon says of followers, which its stats poll looks at every 200 ms and
/// each scrape of its metrics counts, costs nothing of what a fork's base holds: here, of 100
/// forks open of a base of 32768 extents, none followed, the followers' views are counted within
/// a few milliseconds, as of forks of an empty base. Counting them walked each fork's whole
/// extent map, its base's with it, under its state lock: 3.3 million extents a count.
#[test]
fn counting_followers_walks_no_extents() {
    const FORKS: u64 = 100;
    const EXTENTS: u64 = 32768;
    host();
    super::raise_open_files_limit().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let base = RemoteExtentStore::connect(&socket, dir.path().join("base"), 1 << 30).unwrap();
    let branch = base.branch();
    for sequence in 1..=EXTENTS {
        write(&base, ExtentId { branch, sequence }, 0, &pattern(sequence as u8, BLOCK)).unwrap();
    }
    base.commit(None).unwrap();
    let forks: Vec<_> = (1..=FORKS)
        .map(|n| {
            let fork = dir.path().join(format!("fork-{n}"));
            base.fork(&fork, branch + n).unwrap();
            fork
        })
        .collect();
    drop(base);
    let stores: Vec<_> = (forks.iter())
        .map(|fork| RemoteExtentStore::connect(&socket, fork, 1 << 30).unwrap())
        .collect();
    // The quickest of a few counts: what counting itself costs.
    let took = (0..5)
        .map(|_| {
            let start = std::time::Instant::now();
            assert_eq!(super::stores::follower_views(), 0);
            start.elapsed()
        })
        .min()
        .unwrap();
    println!("{FORKS} forks of a base of {EXTENTS} extents: their followers counted in {took:?}");
    assert!(took < std::time::Duration::from_millis(10), "counting them took {took:?}");
    drop(stores);
}

/// TASK-92.49: the daemon makes the ordered commits of 300 stores' guests durable on threads they
/// share, a bounded number, rather than a blocking thread for each store whose flush is due at
/// its turn, which hundreds of guests committing together kept as many of as their syncs' waits
/// took, each idle 10 s after: here each guest commits, ordered, again and again for a second,
/// and, once their commits are durable, the daemon's runtime and the threads its stores' flushes
/// and passes of cleaning run on keep no more threads than it serves 300 forks' reads on
/// (`serves_hundreds_of_forks_of_a_base`). The most seen at once is told too: those a flush syncs
/// its files on beside its own, as many as 15 (`extent_store`'s `AT_ONCE`, TASK-92.42), which go
/// with it, count as its. One guest's commits are durable within a few turns of 50 ms each, and
/// the flushes' time: the most one waited, its lag, is measured. The stores are on a disk, in
/// `/var/tmp`, as a host's are: `/tmp` may be memory (tmpfs), whose syncs cost nothing.
#[test]
fn hundreds_of_writers_flush_on_threads_they_share() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    const STORES: usize = 300;
    const WRITERS: usize = 8;
    // The runtime's, and those flushing and cleaning.
    const NAMES: [&str; 3] = ["extentd io", "extentd flush", "extentd clean"];
    host();
    super::raise_open_files_limit().unwrap();
    let dir = tempfile::tempdir_in("/var/tmp").unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let paths: Vec<_> = (0..STORES).map(|n| dir.path().join(format!("store-{n}"))).collect();
    let stores: Vec<_> = (paths.iter())
        .map(|path| Arc::new(RemoteExtentStore::connect(&socket, path, 1 << 30).unwrap()))
        .collect();
    // The daemon's side of each store, which says whether its commits wait to be made durable;
    // and the guest whose commits' lag is measured.
    let theirs: Vec<_> = (paths.iter())
        .map(|path| {
            let key = super::stores::store_key(path);
            super::stores::open_paths().holder(&key).unwrap().store().unwrap().clone()
        })
        .collect();
    let ((probe, others), probed) = (stores.split_first().unwrap(), theirs[0].clone());
    // The most of them seen at once, and how many of each kind then.
    let sampling = Arc::new(AtomicBool::new(true));
    let sampler = {
        let sampling = sampling.clone();
        std::thread::spawn(move || {
            let mut most = [0; NAMES.len()];
            while sampling.load(Ordering::Relaxed) {
                let names = thread_names();
                let threads = NAMES.map(|name| names.get(name).copied().unwrap_or(0));
                if threads.iter().sum::<usize>() > most.iter().sum() {
                    most = threads;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            most
        })
    };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let writers: Vec<_> = (others.chunks(others.len().div_ceil(WRITERS)))
        .map(|stores| {
            let stores = stores.to_vec();
            std::thread::spawn(move || {
                let mut sequence = 0;
                while std::time::Instant::now() < until {
                    sequence += 1;
                    for store in &stores {
                        let extent = ExtentId { branch: store.branch(), sequence };
                        write(store, extent, 0, &pattern(sequence as u8, BLOCK)).unwrap();
                        store.commit_ordered(None).unwrap();
                    }
                }
            })
        })
        .collect();
    let lagging = {
        let probe = probe.clone();
        std::thread::spawn(move || {
            let (mut lags, mut sequence) = (Vec::new(), 0);
            while std::time::Instant::now() < until {
                sequence += 1;
                let extent = ExtentId { branch: probe.branch(), sequence };
                write(&probe, extent, 0, &pattern(sequence as u8, BLOCK)).unwrap();
                probe.commit_ordered(None).unwrap();
                let committed = std::time::Instant::now();
                while probed.flush_pending() {
                    std::thread::yield_now();
                }
                lags.push(committed.elapsed());
            }
            lags.sort();
            lags
        })
    };
    for writer in writers {
        writer.join().unwrap();
    }
    let lags = lagging.join().unwrap();
    // Every guest's commits durable, what the flushes took and kept.
    while theirs.iter().any(|store| store.flush_pending()) {
        std::thread::yield_now();
    }
    let names = thread_names();
    sampling.store(false, Ordering::Relaxed);
    let most = sampler.join().unwrap();
    let kept = NAMES.map(|name| names.get(name).copied().unwrap_or(0));
    let threads: usize = kept.iter().sum();
    let (median, lag) = (lags[lags.len() / 2], lags[lags.len() - 1]);
    println!(
        "{STORES} stores' guests committing: {threads} threads kept, {kept:?} named {NAMES:?}, \
         at most {} at once, {most:?}; a guest's {} ordered commits durable within {median:?}, \
         the median, and {lag:?}",
        most.iter().sum::<usize>(),
        lags.len()
    );
    assert!(threads < 64, "{threads} threads kept: {kept:?} named {NAMES:?}, of {names:?}");
    // Four turns.
    assert!(lag < std::time::Duration::from_millis(200), "ordered commits durable {lag:?} after");
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
    // TASK-91.3: its uploader's lag, nothing once a durable commit is uploaded.
    store.commit(None).unwrap();
    let metrics = get("/metrics");
    let upload =
        |name: &str| format!("fxdax_extentd_upload_{name}{{branch=\"{:x}\"}}", store.branch());
    assert_eq!(
        [
            metric(&metrics, &upload("pending_bytes")),
            metric(&metrics, &upload("oldest_pending_seconds")),
            metric(&metrics, &upload("stalls_total")),
        ],
        [0, 0, 0]
    );
    assert!(get("/other").starts_with("HTTP/1.1 404 Not Found\r\n"));
    drop(store);
    served.join().unwrap();
}

/// TASK-79.27: the daemon's metrics server answers a request line longer than it reads with `431`,
/// having read no more of it, and closes the connection.
#[test]
fn metrics_refuse_requests_too_long() {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || super::serve_metrics(listener));
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    let mut writing = stream.try_clone().unwrap();
    // A line of a mebibyte, which a server reading lines whole holds whole.
    std::thread::spawn(move || {
        writing.write_all(&vec![b'G'; 1 << 20]).unwrap();
        writing.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let response = within(std::time::Duration::from_secs(10), move || {
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });
    let response = response.unwrap();
    assert!(response.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"), "{response}");
}

/// TASK-79.27: a scrape is answered while another client, slow, has sent but part of its request:
/// the daemon's metrics server waits for neither on the other.
#[test]
fn metrics_are_served_beside_slow_clients() {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || super::serve_metrics(listener));
    let mut slow = std::net::TcpStream::connect(address).unwrap();
    write!(slow, "GET /metrics HTTP/1.1\r\n").unwrap();
    let scraped = within(std::time::Duration::from_secs(2), move || {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        write!(stream, "GET /metrics HTTP/1.1\r\nHost: extentd\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });
    let scraped = scraped.expect("the scrape waited for the slow client");
    assert!(scraped.starts_with("HTTP/1.1 200 OK\r\n"), "{scraped}");
    drop(slow);
}

/// TASK-79.35: the daemon goes on accepting VM processes' connections through a failure to accept
/// one, as with as many files open as it may (EMFILE): it accepts them once files close.
#[test]
fn the_daemon_accepts_on_after_failing_to() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: Reads the process's limit into `limit`.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
    // No file opens: the daemon's accept fails as it begins.
    let none = libc::rlimit { rlim_cur: 0, ..limit };
    // SAFETY: Lowers the process's own limit, which this test sets back.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &none) }, 0);
    let (serving, stopped) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // Dropped as serving stops.
        let _serving = serving;
        serve(listener)
    });
    let served = stopped.recv_timeout(std::time::Duration::from_secs(1));
    // SAFETY: As above.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    assert_eq!(served, Err(std::sync::mpsc::RecvTimeoutError::Timeout), "serving stopped");
    let path = dir.path().join("store");
    let opened = within(std::time::Duration::from_secs(10), move || {
        RemoteExtentStore::connect(&socket, path, 1 << 20).map(|store| store.branch())
    });
    assert!(matches!(opened, Ok(Ok(_))), "the daemon accepts no more: {opened:?}");
}

/// TASK-79.83: a fork of a store, as the control API makes it (`ForkStore`), to a path a store is
/// at already, fails with `EEXIST`, as making a store there does (`CreateStore`).
#[test]
fn forks_to_stores_there_already_fail_with_eexist() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = ExtentStore::create(&path, 1 << 20).unwrap();
    store.commit(None).unwrap();
    drop(store);
    fork_store(&path, &fork).unwrap();
    let again = fork_store(&path, &fork);
    assert_eq!(again.err().and_then(|error| error.errno()), Some(Errno::EEXIST));
}

/// TASK-79.17: a store the daemon opens to work on, as no VM process has it open, boots no guest:
/// the boot profile of the base it is a fork of is not read into the extent cache, as its pages
/// would be the default arena's, which every VM process holds, a tenant's base's too. Here the
/// fork is synced.
#[test]
fn stores_worked_on_here_read_no_boot_profile() {
    host();
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (base, fork) = (dir.path().join("base"), dir.path().join("fork"));
    let store = ExtentStore::create(&base, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&pattern(7, ps)[..])]).unwrap();
    store.commit(None).unwrap();
    store.fork(&fork, extent.branch + 1).unwrap();
    drop(store);
    // The base's profile, as its first fork to boot recorded it.
    let profile = crate::profile::path(&base.canonicalize().unwrap(), extent.branch);
    let recorder = crate::profile::Recorder::new(profile, extent.branch + 1);
    recorder.record(extent, 0, ps as u64);
    recorder.write().unwrap();

    let before = crate::stats::stats();
    wait_uploaded(&fork).unwrap();
    // A profile's replay is done once its threads are.
    while thread_comms().any(|comm| comm == "fxdax replay") {
        std::thread::yield_now();
    }
    let counted = crate::stats::stats().since(&before);
    assert_eq!((counted.prefetches, counted.page_ins), (0, 0));
}

/// TASK-79.34: a store the daemon synced, opened here as no connection had it open, is closed
/// before those waiting to open it may: deleted at once after, it is open no more, each time.
#[test]
fn stores_synced_here_are_closed_before_they_are_let_go() {
    host();
    let dir = tempfile::tempdir().unwrap();
    for round in 0..20 {
        let path = dir.path().join(format!("store-{round}"));
        let store = ExtentStore::create(&path, 1 << 20).unwrap();
        store.commit(None).unwrap();
        drop(store);
        wait_uploaded(&path).unwrap();
        let deleted = delete_store(&path).map_err(|error| error.errno());
        assert_eq!(deleted, Ok(()), "round {round}");
    }
}

/// A memory object of `len` bytes, as a daemon hands its VM processes its arenas.
fn memfd(len: u64) -> std::fs::File {
    // SAFETY: The name is a C string.
    let fd = unsafe { libc::memfd_create(c"memory".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    // SAFETY: Just created, and owned here.
    let file = std::fs::File::from(unsafe {
        <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd)
    });
    file.set_len(len).unwrap();
    file
}

/// TASK-79.43: a window's mappings, a snapshot's, more of them than one message's most bytes of
/// payload hold, arrive whole: a daemon's reply holds as many as a window of its size can have,
/// which its VM process takes. Here a daemon of the test's answers the store's opening, its
/// window's, and its mappings', a page each, past `MAX_PAYLOAD`.
#[test]
fn window_mappings_past_a_messages_payload_arrive() {
    use super::wire::*;
    use std::os::fd::AsFd;
    let ps = page_size();
    // More than a message's payload, and its header's room past it, hold.
    let count = (MAX_PAYLOAD + size_of::<Header>() as u64) / 48 + 1;
    let (client, daemon) = UnixStream::pair().unwrap();
    let answering = std::thread::spawn(move || {
        let (sender, mut receiver) =
            (Sender::new(daemon.try_clone().unwrap()), Receiver::new(daemon));
        let (own, public) = (memfd(ps), memfd(ps));
        while let Ok(Some(request)) = receiver.receive(|| 0) {
            let mut reply = Header::new(REPLY, request.header.id);
            let (mut payload, mut fds) = (Vec::new(), Vec::new());
            match request.header.kind {
                OPEN => {
                    reply.args[0] = 7;
                    reply.args[1] = 1;
                    fds = vec![own.as_fd(), public.as_fd()];
                }
                MAPPINGS => {
                    payload = (0..count)
                        .flat_map(|page| [page * ps, ps, 0, 0, 0, MAPPING_ZEROS])
                        .flat_map(u64::to_le_bytes)
                        .collect();
                }
                WINDOW => {}
                _ => reply.status = -Errno::EINVAL.raw(),
            }
            sender.send(reply, &payload, &fds).unwrap();
        }
    });
    let store = RemoteExtentStore::over(client, Path::new("store"), Opening::Create(1 << 20));
    let store = store.unwrap();
    let size = count * ps;
    // SAFETY: Reserves the window's memory, which the test leaks: the window maps it while the
    // process lives.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    assert_ne!(memory, libc::MAP_FAILED);
    let memory = NonNull::new(memory.cast()).unwrap();
    // SAFETY: The memory is the test's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, size, None) }.unwrap();
    let mappings = store.window_mappings().unwrap();
    assert_eq!(mappings.len() as u64, count);
    let last = crate::WindowMapping {
        window_offset: (count - 1) * ps,
        len: ps,
        contents: crate::MappingContents::Zeros,
    };
    assert_eq!(mappings.last(), Some(&last));
    drop(store);
    answering.join().unwrap();
}

/// A VM process's connection to the daemon at `socket`, made by hand, as one that keeps to no
/// protocol makes it: its store at `path` opened, created with `capacity` bytes, and its branch.
fn raw_connection(
    socket: &Path,
    path: &Path,
    capacity: u64,
) -> (super::wire::Sender, super::wire::Receiver, UnixStream, u64) {
    use super::wire::*;
    let stream = UnixStream::connect(socket).unwrap();
    let (sender, mut receiver) =
        (Sender::new(stream.try_clone().unwrap()), Receiver::new(stream.try_clone().unwrap()));
    let open = Header::new(OPEN, 1).with_args(&[capacity]).with_flags(OPEN_CREATE);
    sender.send(open, std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()), &[]).unwrap();
    let opened = receiver.receive(|| 0).unwrap().unwrap();
    assert_eq!(opened.header.status, 0);
    (sender, receiver, stream, opened.header.args[0])
}

/// How many bytes `stream` has received that are not read yet.
fn unread(stream: &UnixStream) -> usize {
    use std::os::fd::AsRawFd;
    let mut unread: libc::c_int = 0;
    // SAFETY: Reads the count into `unread`.
    assert_eq!(unsafe { libc::ioctl(stream.as_raw_fd(), libc::FIONREAD, &mut unread) }, 0);
    unread as usize
}

/// TASK-79.16: a VM process that stops reading what the daemon sends it holds up no other: as the
/// daemon's reply to it waits for room in its socket, another VM process opening its store is
/// refused at once (`EBUSY`), and another's store opens.
#[test]
fn vm_processes_that_stop_reading_hold_up_no_other() {
    use super::wire::*;
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let path = dir.path().join("store");
    let (sender, mut receiver, stream, branch) = raw_connection(&socket, &path, 1 << 24);
    // An extent, read back whole in one reply, many times what the socket holds, which it never
    // reads.
    let len = 4 << 20;
    let written = Header::new(WRITE, 2).with_args(&[branch, 1, 0]);
    sender.send(written, &pattern(1, len), &[]).unwrap();
    assert_eq!(receiver.receive(|| 0).unwrap().unwrap().header.status, 0);
    let read = Header::new(READ, 3).with_args(&[branch, 1, 0, len as u64]);
    sender.send(read, &[], &[]).unwrap();
    while unread(&stream) == 0 {
        std::thread::yield_now();
    }

    let wait = std::time::Duration::from_secs(10);
    let opening = path.clone();
    let (busy_socket, other_socket) = (socket.clone(), socket.clone());
    let busy = within(wait, move || {
        RemoteExtentStore::connect(&busy_socket, &opening, 1 << 24).err().and_then(|e| e.errno())
    });
    assert_eq!(busy, Ok(Some(Errno::EBUSY)), "opening the store waited for its VM process");
    let other = dir.path().join("other");
    let opened = within(wait, move || {
        RemoteExtentStore::connect(&other_socket, &other, 1 << 24).map(|store| store.branch())
    });
    assert!(matches!(opened, Ok(Ok(_))), "another store's opening waited: {opened:?}");
    drop((sender, receiver, stream));
}

/// TASK-79.16: a VM process that leaves what the daemon sends it unread, past what a connection
/// may hold, is let go, rather than held without end in the daemon's memory, or its threads: here
/// replies to its reads, half a gibibyte of them.
#[test]
fn vm_processes_that_leave_replies_unread_are_let_go() {
    use super::wire::*;
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (sender, mut receiver, stream, branch) =
        raw_connection(&socket, &dir.path().join("store"), 1 << 24);
    let len = 8 << 20;
    let written = Header::new(WRITE, 2).with_args(&[branch, 1, 0]);
    sender.send(written, &pattern(1, len), &[]).unwrap();
    assert_eq!(receiver.receive(|| 0).unwrap().unwrap().header.status, 0);
    for id in 3..67 {
        let read = Header::new(READ, id).with_args(&[branch, 1, 0, len as u64]);
        sender.send(read, &[], &[]).unwrap();
    }
    use std::os::fd::AsRawFd;
    let mut hung_up = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
    // SAFETY: Polls the stream, which the test keeps open, for 10 s at most.
    let ready = unsafe { libc::poll(&mut hung_up, 1, 10_000) };
    assert_eq!(ready, 1, "the daemon holds what the VM process leaves unread");
    assert_ne!(hung_up.revents & libc::POLLRDHUP, 0, "{:#x}", hung_up.revents);
    drop((sender, receiver, stream));
}

/// TASK-79.16: a tenant's VM processes, the default tenant's too, have so many connections at
/// most: one past them is closed as it comes; once one goes, another is served.
#[test]
fn tenants_connections_are_bounded() {
    use std::io::Read as _;
    host();
    super::raise_open_files_limit().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let mut connected: Vec<UnixStream> = (0..super::server::MAX_CONNECTIONS)
        .map(|_| UnixStream::connect(&socket).unwrap())
        .collect();
    let past = UnixStream::connect(&socket).unwrap();
    let closed = within(std::time::Duration::from_secs(10), move || {
        (&past).read(&mut [0]).map_err(|err| err.kind())
    });
    assert_eq!(closed, Ok(Ok(0)), "a connection past the tenant's was served");

    drop(connected.pop());
    let path = dir.path().join("store");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let store = loop {
        match RemoteExtentStore::connect(&socket, &path, 1 << 20) {
            Ok(store) => break store,
            Err(error) => {
                assert!(std::time::Instant::now() < deadline, "no connection served: {error:#}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    };
    drop((store, connected));
}

/// TASK-79.84: a VM restored from a snapshot to where a fork of its point failed before, whose
/// deletion stopped partway, leaving the fork's handle, marked failed, gets a fork made anew, of a
/// branch of its own, holding what the point holds: not the failed fork, which goes.
#[test]
fn restores_fork_anew_over_forks_that_failed() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(8, BLOCK)).unwrap();
    store.commit(Some(b"root")).unwrap();
    let point = dir.path().join("point");
    store.fork_quiescent(&point, store.branch() + 1).unwrap();
    drop(store);

    // A fork of the point there, as one that failed leaves it.
    let restored = dir.path().join("restored");
    let failed = fork_store(&point, &restored).unwrap();
    extent_store::fail_fork(&restored).unwrap();
    let ours = RemoteExtentStore::connect_forked(&socket, &restored, &point, 0, 0).unwrap();
    assert_ne!(ours.branch(), failed, "the restored VM opened the failed fork");
    assert_eq!(ours.root(), Ok(b"root".to_vec()));
    assert_eq!(read(&ours, extent, 0, BLOCK), Ok(pattern(8, BLOCK)));
    assert!(!ExtentStore::fork_failed(&restored), "the new fork's handle reads as failed");
}

/// How many requests of `kind` the daemon has served, as its metrics count them.
fn served(kind: &str) -> u64 {
    let metrics = super::metrics::render();
    let name = format!("fxdax_extentd_requests_total{{kind=\"{kind}\"}} ");
    let line = metrics.lines().find_map(|line| line.strip_prefix(&name));
    line.unwrap_or_else(|| panic!("no {kind} requests counted in {metrics}")).parse().unwrap()
}

/// TASK-92.13: a flush's dead extents, as many as a guest's `FREE` names with `FREE_MANY` (257),
/// freed together, are freed in one request of the daemon's, not one each, each a round trip.
#[test]
fn extents_freed_together_are_freed_in_one_request() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let (store, serving) = connect(&dir.path().join("store"), 1 << 24);
    let branch = store.branch();
    let extents: Vec<_> = (1..=257).map(|sequence| ExtentId { branch, sequence }).collect();
    for extent in &extents {
        write(&store, *extent, 0, &pattern(extent.sequence as u8, BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    let before = served("free");
    store.free_many(&extents).unwrap();
    assert_eq!(served("free") - before, 1);
    for extent in [extents[0], extents[128], extents[256]] {
        assert_eq!(read(&store, extent, 0, BLOCK), Err(Errno::ENOENT), "{extent:?}");
    }
    drop(store);
    serving.join().unwrap();
}

/// An object store in memory whose reads wait while they are held ([`GatedReads::hold`]): what
/// makes a page-in's fetch from object storage wait as long as a test needs.
#[derive(Debug)]
struct GatedReads {
    store: object_store::memory::InMemory,
    held: tokio::sync::watch::Sender<bool>,
    /// The reads waiting.
    waiting: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Display for GatedReads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gated reads")
    }
}

impl GatedReads {
    fn new() -> Self {
        Self {
            store: object_store::memory::InMemory::new(),
            held: tokio::sync::watch::channel(false).0,
            waiting: Default::default(),
        }
    }

    /// Holds the reads from now on, if `held`, or lets them go.
    fn hold(&self, held: bool) {
        self.held.send_replace(held);
    }

    fn waiting(&self) -> usize {
        self.waiting.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl object_store::ObjectStore for GatedReads {
    async fn put_opts(
        &self,
        location: &object_store::path::Path,
        payload: object_store::PutPayload,
        opts: object_store::PutOptions,
    ) -> object_store::Result<object_store::PutResult> {
        self.store.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &object_store::path::Path,
        opts: object_store::PutMultipartOptions,
    ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
        self.store.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &object_store::path::Path,
        options: object_store::GetOptions,
    ) -> object_store::Result<object_store::GetResult> {
        use std::sync::atomic::Ordering;
        if *self.held.borrow() {
            let mut held = self.held.subscribe();
            self.waiting.fetch_add(1, Ordering::Relaxed);
            let _ = held.wait_for(|held| !*held).await;
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        }
        self.store.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: futures::stream::BoxStream<
            'static,
            object_store::Result<object_store::path::Path>,
        >,
    ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::path::Path>> {
        self.store.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&object_store::path::Path>,
    ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
        self.store.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&object_store::path::Path>,
    ) -> object_store::Result<object_store::ListResult> {
        self.store.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &object_store::path::Path,
        to: &object_store::path::Path,
        options: object_store::CopyOptions,
    ) -> object_store::Result<()> {
        self.store.copy_opts(from, to, options).await
    }
}

/// TASK-92.20: a VM's faults that wait for the pager, which fetches what they fault on from
/// object storage, hold up none of its requests: here 12 of them, more than the 8 requests the
/// daemon serves of a connection at once, wait while the bucket answers no reads, and the VM's
/// `INFO` is answered meanwhile. Each held a thread of the daemon's and one of those 8 while it
/// waited, as for a part from GCS, 30-100 ms, and the requests after them waited their turn.
#[test]
fn faults_waiting_for_object_storage_hold_up_no_request() {
    use std::sync::Arc;
    const FAULTS: usize = 12;
    host();
    let ps = page_size() as usize;
    let gated = Arc::new(GatedReads::new());
    super::stores::use_bucket(extent_remote::store::Store::new(gated.clone(), "branches"));
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    // A store of an extent a page for each fault, written here, whose pages are of an arena no
    // VM process's connection has, made a snapshot's point through the daemon, which keeps its
    // branch in the bucket.
    let (path, point) = (dir.path().join("store"), dir.path().join("point"));
    let branch = {
        let store = ExtentStore::create(&path, 1 << 24).unwrap();
        let branch = store.branch();
        for sequence in 1..=FAULTS as u64 {
            let data = pattern(sequence as u8, ps);
            store
                .write(ExtentId { branch, sequence }, 0, &[VolatileSlice::from(&data[..])])
                .unwrap();
        }
        store.commit(None).unwrap();
        branch
    };
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    store.fork_quiescent(&point, branch + 1).unwrap();
    drop(store);
    // Its VM restored on another host, which has the point's extents in the bucket alone.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let restored = elsewhere.join("store");
    let ours = RemoteExtentStore::connect_forked(
        &socket,
        &restored,
        elsewhere.join("point"),
        branch + 1,
        1 << 24,
    )
    .unwrap();
    let memory = memory(FAULTS);
    // SAFETY: The memory is the test's, and outlives the store.
    unsafe { ours.create_dax_window_in_host_memory(memory, (FAULTS * ps) as u64, None) }.unwrap();
    for n in 0..FAULTS {
        let extent = ExtentId { branch, sequence: n as u64 + 1 };
        ours.map(extent, 0, ps as u64, (n * ps) as u64, false).unwrap();
    }
    gated.hold(true);
    let base = memory.as_ptr() as usize;
    let faulting: Vec<_> = (0..FAULTS)
        .map(|n| {
            // SAFETY: The window's memory, whose faults the store serves.
            std::thread::spawn(move || unsafe {
                std::slice::from_raw_parts((base + n * ps) as *const u8, ps).to_vec()
            })
        })
        .collect();
    // Every fault received, each waiting for a fetch.
    let connection =
        super::stores::open_paths().holder(&super::stores::store_key(&restored)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while connection.unanswered() < FAULTS || gated.waiting() == 0 {
        assert!(std::time::Instant::now() < deadline, "{} faults", connection.unanswered());
        std::thread::yield_now();
    }
    let ours = Arc::new(ours);
    let asking = ours.clone();
    let info = within(std::time::Duration::from_secs(10), move || asking.info());
    let answered = connection.unanswered();
    gated.hold(false);
    assert!(matches!(info, Ok(Ok(_))), "the VM's INFO waited for its faults: {info:?}");
    assert_eq!(answered, FAULTS, "the faults were answered as the INFO was");
    for (n, faulting) in faulting.into_iter().enumerate() {
        let page = faulting.join().unwrap();
        assert!(page == pattern(n as u8 + 1, ps), "page {n} holds other data");
    }
}

/// TASK-92.5: the restores of a zygote's snapshot, all at once, here with object storage as far
/// away as another region's (50 ms), fork its point as it is, opened once for them and kept open:
/// none fails, and the point's branch is taken once, by the writer of the point kept open, not
/// once by each. Each restore opened the point, attached it (taking its branch), forked and closed
/// it, in turn, the k-th waiting for all those before it, a second or more each, and those that
/// waited past `CLOSING_WAIT` failed (`EBUSY`). Their forks are made in object storage one at a
/// time still, each waiting for a checkpoint of the point's branch that reads the record of every
/// fork before it (TASK-92.4): what each restore took is told.
#[test]
fn a_zygotes_restores_open_its_point_once() {
    const RESTORES: usize = 24;
    host();
    // SAFETY: As in host.
    unsafe { std::env::set_var("FXDAX_REMOTE_LATENCY_MS", "50") };
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(6, BLOCK)).unwrap();
    store.commit(Some(b"zygote")).unwrap();
    let (point, point_branch) = (dir.path().join("point"), store.branch() + 1);
    store.fork_quiescent(&point, point_branch).unwrap();
    drop(store);
    // How many writers took the point's branch: its lease's epoch.
    let bucket = super::stores::remote_bucket().unwrap();
    let lease = extent_remote::layout::lease(point_branch);
    let takings = || control(extent_remote::branch::lease_epoch(&*bucket, &lease)).unwrap();
    let taken = takings();

    let restoring: Vec<_> = (0..RESTORES)
        .map(|n| {
            let (socket, point) = (socket.clone(), point.clone());
            let to = dir.path().join(format!("restore-{n}"));
            std::thread::spawn(move || {
                let start = std::time::Instant::now();
                let restored = RemoteExtentStore::connect_forked(&socket, to, point, 0, 0);
                let root = restored.map_err(|error| error.errno()).and_then(|store| {
                    assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
                    store.root().map_err(Some)
                });
                (root, start.elapsed())
            })
        })
        .collect();
    let restored: Vec<_> = restoring.into_iter().map(|thread| thread.join().unwrap()).collect();
    let took: Vec<_> = restored.iter().map(|(_, took)| *took).collect();
    let taken = takings() - taken;
    println!("{RESTORES} restores at once took {took:?}; the point's branch taken {taken} times");
    for (n, (root, _)) in restored.iter().enumerate() {
        assert_eq!(root, &Ok(b"zygote".to_vec()), "restore {n}");
    }
    assert_eq!(taken, 1, "the point's branch was taken by more writers than one");
}

/// TASK-79.9: a follower's VM process that stops reading, frozen as SIGSTOP freezes it, holds as
/// much of the daemon's memory as a message telling it of its writer's commits, whatever the
/// writer commits meanwhile: told of the latest once it reads again, rather than of each, which the
/// daemon kept for it without end, as it sends without waiting (TASK-79.9).
#[test]
fn followers_that_stop_reading_are_told_of_the_latest_commit() {
    use super::wire::*;
    const ROUNDS: u8 = 200;
    host();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let path = dir.path().join("store");
    let writer = RemoteExtentStore::connect(&socket, &path, 1 << 24).unwrap();
    // The follower's VM process, by hand, which reads its opening's reply, and nothing more until
    // its writer is done.
    let stream = UnixStream::connect(&socket).unwrap();
    let (sender, mut receiver) = (Sender::new(stream.try_clone().unwrap()), Receiver::new(stream));
    let open = Header::new(OPEN, 1).with_flags(OPEN_READ_ONLY);
    sender.send(open, std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()), &[]).unwrap();
    let opened = receiver.receive(|| 0).unwrap().unwrap();
    assert_eq!(opened.header.status, 0);
    let mut epoch = opened.header.args[2];

    // Commits, many times what its socket holds, each told of as the writer makes it, each of an
    // extent of its own.
    let mut sequence = 0;
    for round in 0..ROUNDS {
        for _ in 0..16 {
            sequence += 1;
            let extent = ExtentId { branch: writer.branch(), sequence };
            write(&writer, extent, 0, &pattern(round, BLOCK)).unwrap();
            writer.commit_ordered(None).unwrap();
        }
        let queued = super::server::most_queued();
        assert!(queued <= size_of::<Header>() as u64, "{queued} bytes queued for VM processes");
    }
    let key = super::stores::store_key(&path);
    let holder = super::stores::open_paths().holder(&key).unwrap();
    let latest = holder.store().unwrap().committed().unwrap();
    // Each commit it is told of after the last, the latest last.
    let told = within(std::time::Duration::from_secs(10), move || {
        while epoch < latest {
            let told = receiver.receive(|| 0).unwrap().unwrap();
            assert_eq!(told.header.kind, COMMITTED);
            assert!(told.header.args[0] > epoch, "told of {} after {epoch}", told.header.args[0]);
            epoch = told.header.args[0];
        }
        epoch
    });
    assert_eq!(told, Ok(latest), "the follower wasn't told of its writer's latest commit");
    drop((sender, writer));
}

/// Sends `header` and `payload`, a request, through `sender`, and returns its reply, which
/// `receiver` receives, the stage operations before it acknowledged, as a VM process makes them.
fn call(
    sender: &super::wire::Sender,
    receiver: &mut super::wire::Receiver,
    header: super::wire::Header,
    payload: &[u8],
) -> super::wire::Message {
    use super::wire::*;
    sender.send(header, payload, &[]).unwrap();
    loop {
        let message = receiver.receive(|| 0).unwrap().expect("the daemon let the VM process go");
        match message.header.kind {
            REPLY => {
                assert_eq!(message.header.id, header.id, "a reply out of order");
                return message;
            }
            _ if message.header.flags & STAGE_ACK != 0 => {
                sender.send(Header::new(ACK, message.header.id), &[], &[]).unwrap();
            }
            _ => {}
        }
    }
}

/// TASK-79.9: a shrink of a tenant's memory evicts what it must, and returns what the tenant holds,
/// once the pages it evicted are free: a window's, which wait, freed, for its VM process to
/// acknowledge their unmapping, or for the daemon to let go of it, as here of one that acknowledges
/// nothing. Of two extents of a chunk each, read and mapped, it evicts the one asked for longest
/// ago, which leaves the tenant the other's chunk alone.
#[test]
fn shrinks_return_once_the_pages_they_evict_are_free() {
    use super::wire::*;
    const CHUNK: u64 = zircon::vm::physmap::CHUNK_SIZE as u64;
    host();
    let dir = tempfile::tempdir().unwrap();
    let tenant = super::server::Tenant::new("shrunk", dir.path()).unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || super::server::serve_tenant(listener, tenant));
    let (sender, mut receiver, _stream, branch) =
        raw_connection(&socket, &dir.path().join("store"), 1 << 26);
    let mut served = |header: Header, payload: &[u8]| {
        let reply = call(&sender, &mut receiver, header, payload);
        assert_eq!(reply.header.status, 0, "request {}", header.kind);
        reply
    };
    for sequence in 1..=2 {
        let written = Header::new(WRITE, sequence).with_args(&[branch, sequence, 0]);
        served(written, &pattern(sequence as u8, CHUNK as usize));
    }
    served(Header::new(COMMIT, 3).with_flags(COMMIT_DURABLE), &[]);
    for sequence in 1..=2 {
        let read = Header::new(READ, 3 + sequence).with_args(&[branch, sequence, 0, CHUNK]);
        assert_eq!(served(read, &[]).payload, pattern(sequence as u8, CHUNK as usize));
    }
    served(Header::new(WINDOW, 6).with_args(&[2 * CHUNK]), &[]);
    for sequence in 1..=2 {
        let at = (sequence - 1) * CHUNK;
        served(Header::new(MAP, 6 + sequence).with_args(&[branch, sequence, 0, CHUNK, at]), &[]);
    }
    assert_eq!(super::memory::shrink(Some("shrunk"), u64::MAX), Ok(2 * CHUNK));

    let shrinking = std::thread::spawn(|| super::memory::shrink(Some("shrunk"), CHUNK));
    // The unmaps it is sent, which it never acknowledges, until the daemon lets it go.
    let mut unmapped = Vec::new();
    while let Ok(Some(message)) = receiver.receive(|| 0) {
        assert_eq!(message.header.kind, STAGE_UNMAP, "{:?}", message.header);
        unmapped.push(message.header.args[..2].to_vec());
    }
    assert_eq!((shrinking.join().unwrap(), unmapped), (Ok(CHUNK), vec![vec![0, CHUNK]]));
}

/// TASK-79.34: a store whose VM process goes while the daemon's work through its connection holds
/// the connection, here a shrink waiting for its guest's answer, is let go once that work is done,
/// which its VM process's going ends, not before: deleted once its connection is done, it is open
/// no more.
#[test]
fn stores_held_through_their_connections_are_let_go_once_they_are() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 2 << 20);
    let (asks, asked) = std::sync::mpsc::channel();
    store.on_capacity(move |capacity, ask| {
        // Unheard once the test has its answer.
        let _ = asks.send((capacity, ask));
    });
    let resizing = {
        let path = path.clone();
        std::thread::spawn(move || resize(&path, 1 << 20))
    };
    assert_eq!(asked.recv().unwrap(), (1 << 20, true));
    // Its VM process goes, answering nothing.
    drop(store);
    served.join().unwrap();
    assert_eq!(delete_store(&path).map_err(|error| error.errno()), Ok(()));
    assert_eq!(resizing.join().unwrap(), Err(Errno::ETIMEDOUT));
}

/// TASK-79.16: a VM process that keeps as much stream data in flight as the protocol allows,
/// whole extents' writes of eight threads, four in flight at once, the others waiting for room
/// until one is answered, is served on: what a request's payload holds is counted in flight until
/// its reply is queued, and the reply's bytes from then, not both (`server::Answer`), which, for a
/// while, would have a VM process within the bounds let go.
#[test]
fn vm_processes_within_the_bounds_are_served_on() {
    const WRITERS: u64 = 8;
    const ROUNDS: u64 = 8;
    host();
    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 30);
    let store = std::sync::Arc::new(store);
    let extent = pattern(3, super::wire::MAX_PAYLOAD as usize);
    let extent = std::sync::Arc::new(extent);
    let writers: Vec<_> = (1..=WRITERS)
        .map(|sequence| {
            let (store, extent) = (store.clone(), extent.clone());
            std::thread::spawn(move || {
                let id = ExtentId { branch: store.branch(), sequence };
                (0..ROUNDS).map(|_| write(&store, id, 0, &extent)).collect::<Vec<_>>()
            })
        })
        .collect();
    let written: Vec<_> = writers.into_iter().map(|writer| writer.join().unwrap()).collect();
    assert_eq!(written, vec![vec![Ok(()); ROUNDS as usize]; WRITERS as usize]);
    drop(store);
    served.join().unwrap();
}
