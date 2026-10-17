// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's side of a connection: a VM process's store and window, served from this process's
//! stores, extent cache and kernel.

use super::wire::*;
use crate::{Access, DaxWindow, Deferred, Errno, ExtentId, ExtentStore, Protection, SecondStage};
use crate::{Commit, MappingContents, VolatileSlice, VolatileSliceMut, WriteBarriers};
use extent_remote::cache::Cache;
use extent_remote::store::Stats;
use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::sync::{Weak, mpsc};

/// How many of a connection's requests the daemon serves at once: a VM's device queues', and
/// faults waiting for the pager.
const WORKERS: usize = 8;

/// fxdax: how long a commit only ordered (`COMMIT` without `COMMIT_DURABLE`) waits at most to be
/// made durable: the host's lag, which doc-2's requirement 11 bounds.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// Serves the VM processes that connect to `listener`, each on threads of its own, until accepting
/// a connection fails.
pub fn serve(listener: UnixListener) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        std::thread::Builder::new()
            .name("extentd connection".into())
            .spawn(move || serve_connection(stream))?;
    }
    Ok(())
}

/// Serves the VM process at the other end of `stream` until it closes the connection, or exits:
/// then its store and window go.
pub(crate) fn serve_connection(stream: UnixStream) {
    let sender = match stream.try_clone() {
        Ok(clone) => Arc::new(Sender::new(clone)),
        Err(err) => {
            log::error!("extentd: cloning a connection: {err}");
            return;
        }
    };
    let connection = Arc::new_cyclic(|this| Connection {
        this: this.clone(),
        sender: sender.clone(),
        acks: Arc::new(Acks::default()),
        store: OnceLock::new(),
        path: OnceLock::new(),
        window: Mutex::new(None),
        flusher: Mutex::new(None),
        shrinking: Mutex::new(None),
    });
    let (requests, queue) = mpsc::channel::<Message>();
    let queue = Arc::new(Mutex::new(queue));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (connection, queue) = (connection.clone(), queue.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok(message) = queue.lock().unwrap().recv() else { return };
                    connection.serve(message);
                }
            })
        })
        .collect();
    let mut receiver = Receiver::new(stream);
    loop {
        match receiver.receive() {
            Ok(Some(message)) if message.header.kind == ACK => {
                connection.acks.acked(message.header.id)
            }
            Ok(Some(message)) => {
                if requests.send(message).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                log::error!("extentd: receiving from a VM process: {err}");
                break;
            }
        }
    }
    // The VM process is gone, and its mappings of the window with it: nothing waits for it.
    connection.acks.close();
    drop(requests);
    for worker in workers {
        let _ = worker.join();
    }
    // What it committed is durable before another connection may open its store.
    if let Some((stop, flusher)) = connection.flusher.lock().unwrap().take() {
        drop(stop);
        let _ = flusher.join();
    }
    if let Some(store) = connection.store.get()
        && let Err(err) = store.flush()
    {
        log::error!("extentd: making a store's last commits durable: {err:?}");
    }
    sender.shutdown();
    // Its store goes, with its window and open extents, before another connection may open it.
    let path = connection.path.get().cloned();
    drop(connection);
    if let Some(path) = path {
        open_paths().lock().unwrap().remove(&path);
    }
}

/// The paths of the stores connections have open, which no other may open, with the connections:
/// what a [`RESIZE`] of one finds. A store resized while no connection has it is held by none.
fn open_paths() -> &'static Mutex<HashMap<PathBuf, Weak<Connection>>> {
    static PATHS: OnceLock<Mutex<HashMap<PathBuf, Weak<Connection>>>> = OnceLock::new();
    PATHS.get_or_init(Default::default)
}

/// The store at `path`, created with `capacity` bytes if given and it doesn't exist, keeping its
/// branch in the daemon's bucket, if it has one.
fn open_store(path: &Path, capacity: Option<u64>) -> Result<ExtentStore, Errno> {
    let bucket = remote_bucket()?;
    let opened = match capacity {
        Some(capacity) if !path.exists() => ExtentStore::create(path, capacity),
        _ => ExtentStore::open(path),
    };
    let store = opened.map_err(|err| {
        log::warn!("extentd: opening the store at {}: {err:#}", path.display());
        Errno::EIO
    })?;
    if let Some(bucket) = bucket {
        let options = crate::remote::Options { cache: remote_cache()?, ..Default::default() };
        if let Err(err) = store.attach_remote(bucket, rand::random(), options) {
            log::warn!("extentd: keeping branch {:x} in object storage: {err}", store.branch());
            return Err(Errno::EIO);
        }
    }
    Ok(store)
}

/// Forks the store at `from`, the point a snapshot of a VM was taken at, to a new store at `path`,
/// a branch of its own, which the VM restored from the snapshot opens (TASK-39.1). Restores fork
/// one at a time, so that two from one snapshot, a zygote's, do not find its store busy; a
/// connection having it open makes it so (`EBUSY`).
fn fork_from(from: &Path, path: &Path) -> Result<(), Errno> {
    static FORKING: Mutex<()> = Mutex::new(());
    let _forking = FORKING.lock().unwrap();
    let key = store_key(from);
    {
        let mut paths = open_paths().lock().unwrap();
        if paths.contains_key(&key) {
            return Err(Errno::EBUSY);
        }
        paths.insert(key.clone(), Weak::new());
    }
    let forked = open_store(from, None).and_then(|store| {
        store.fork(path, rand::random::<u64>().max(1)).map_err(|err| {
            log::warn!("extentd: forking {} to {}: {err:#}", from.display(), path.display());
            Errno::EIO
        })
    });
    open_paths().lock().unwrap().remove(&key);
    forked
}

/// The path that names the store at `path` in [`open_paths`]: whichever way a request names it.
fn store_key(path: &std::path::Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    // A store not yet made: where it will be.
    match (path.parent().map(std::fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_owned(),
    }
}

/// How long a shrink waits for the guest's answer.
const SHRINK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A connection's store and window.
struct Connection {
    this: Weak<Connection>,
    sender: Arc<Sender>,
    acks: Arc<Acks>,
    store: OnceLock<Arc<ExtentStore>>,
    path: OnceLock<PathBuf>,
    window: Mutex<Option<DaxWindow>>,
    /// The thread making the store's commits durable behind them, and what stops it.
    flusher: Mutex<Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>>,
    /// The shrink the guest was asked to take, and where its answer goes ([`RESIZED`]).
    shrinking: Mutex<Option<(u64, mpsc::Sender<bool>)>>,
}

impl Connection {
    /// Serves `message`, a request, and replies.
    fn serve(&self, message: Message) {
        let header = message.header;
        let mut reply = Header::new(REPLY, header.id);
        let mut payload = Vec::new();
        let mut fd = None;
        let result = self.handle(&message, &mut reply, &mut payload, &mut fd);
        if let Err(errno) = result {
            // A full store: which request found it so, and how full.
            if errno == Errno::ENOSPC
                && let Some(store) = self.store.get()
            {
                let used = store.info().map(|info| (info.used, info.capacity));
                log::warn!(
                    "extentd: request {} (flags {:#x}, args {:#x?}) found the store full: {used:?}",
                    header.kind,
                    header.flags,
                    header.args
                );
            }
            // A request the store refuses as malformed is the guest's mistake: which it was.
            if errno == Errno::EINVAL {
                log::warn!(
                    "extentd: request {} refused with EINVAL: flags {:#x}, args {:#x?}",
                    header.kind,
                    header.flags,
                    header.args
                );
            }
            reply.status = -errno.raw();
            payload.clear();
            fd = None;
        }
        if let Err(err) = self.sender.send(reply, &payload, fd) {
            log::warn!("extentd: replying to a VM process: {err}");
        }
    }

    fn handle(
        &self,
        message: &Message,
        reply: &mut Header,
        payload: &mut Vec<u8>,
        fd: &mut Option<std::os::fd::BorrowedFd<'static>>,
    ) -> Result<(), Errno> {
        let Header { kind, flags, args, .. } = message.header;
        let extent = ExtentId { branch: args[0], sequence: args[1] };
        if kind == OPEN {
            let (path, from) = if flags & OPEN_FORK_FROM != 0 {
                let payload = &message.payload;
                let nul = payload.iter().position(|byte| *byte == 0).ok_or(Errno::EINVAL)?;
                (&payload[..nul], Some(&payload[nul + 1..]))
            } else {
                (&message.payload[..], None)
            };
            let branch = self.open(path, args[0], flags & OPEN_CREATE != 0, from)?;
            reply.args[0] = branch;
            *fd = Some(crate::kernel::physical_memory());
            return Ok(());
        }
        if kind == RESIZE {
            return resize(&message.payload, args[0]);
        }
        if kind == STATS {
            *payload =
                crate::stats::stats().words().iter().flat_map(|word| word.to_le_bytes()).collect();
            return Ok(());
        }
        let store = self.store.get().ok_or(Errno::EBADF)?;
        match kind {
            READ => {
                if args[3] > MAX_PAYLOAD {
                    return Err(Errno::EINVAL);
                }
                payload.resize(args[3] as usize, 0);
                store.read(extent, args[2], &mut [VolatileSliceMut::from(&mut payload[..])])
            }
            WRITE => {
                let barriers = WriteBarriers {
                    before: flags & WRITE_BARRIER != 0,
                    after: (flags & WRITE_COMMIT != 0).then_some(if flags & WRITE_DURABLE != 0 {
                        Commit::Durable
                    } else {
                        Commit::Ordered
                    }),
                };
                let data = [VolatileSlice::from(&message.payload[..])];
                store.write_with_barriers(extent, args[2], &data, barriers)
            }
            FREE => store.free(extent),
            MAP if flags & MAP_ZEROS != 0 => store.map_zeros_at(args[4], args[3]),
            MAP => store.map(extent, args[2], args[3], args[4], flags & MAP_WRITABLE != 0),
            MAPPINGS => {
                *payload = store
                    .window_mappings()
                    .iter()
                    .flat_map(|mapping| {
                        let words = match mapping.contents {
                            MappingContents::Extent { extent, offset, writable } => [
                                extent.branch,
                                extent.sequence,
                                offset,
                                if writable { MAPPING_WRITABLE } else { 0 },
                            ],
                            MappingContents::Zeros => [0, 0, 0, MAPPING_ZEROS],
                        };
                        [mapping.window_offset, mapping.len].into_iter().chain(words)
                    })
                    .flat_map(u64::to_le_bytes)
                    .collect();
                Ok(())
            }
            UNMAP => store.unmap(args[0], args[1]),
            COMMIT => {
                let root = (flags & COMMIT_ROOT != 0).then_some(&message.payload[..]);
                if flags & COMMIT_DURABLE != 0 {
                    store.commit(root)
                } else {
                    store.commit_ordered(root)
                }
            }
            ROOT => store.root().map(|root| *payload = root),
            FORK => {
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(&message.payload));
                if flags & FORK_QUIESCENT != 0 && !store.is_quiescent()? {
                    return Err(Errno::EBUSY);
                }
                store.fork(&path, args[0]).map_err(|err| {
                    log::warn!("extentd: forking to {}: {err:#}", path.display());
                    Errno::EIO
                })
            }
            INFO => store.info().map(|info| {
                reply.args = [
                    info.capacity,
                    info.used,
                    info.available,
                    info.next_sequence,
                    info.sequence_floor,
                ];
            }),
            WINDOW => {
                let stage =
                    Arc::new(RemoteStage { sender: self.sender.clone(), acks: self.acks.clone() });
                let window = store.create_dax_window(args[0], Some(stage)).map_err(|err| {
                    log::warn!("extentd: creating a window: {err:#}");
                    Errno::EINVAL
                })?;
                *self.window.lock().unwrap() = Some(window);
                Ok(())
            }
            RESIZED => {
                let shrinking = self.shrinking.lock().unwrap().take();
                match shrinking {
                    Some((asked, answer)) if asked == args[0] => {
                        let _ = answer.send(args[1] == 1);
                        Ok(())
                    }
                    // An answer to a shrink no longer waited for.
                    other => {
                        *self.shrinking.lock().unwrap() = other;
                        Ok(())
                    }
                }
            }
            FAULT => {
                let window = self.window.lock().unwrap().clone().ok_or(Errno::EBADF)?;
                let access = if flags & FAULT_WRITE != 0 { Access::Write } else { Access::Read };
                futures::executor::block_on(async {
                    if flags & FAULT_PAGE != 0 {
                        window.fault_page(args[0], access).await
                    } else {
                        window.fault(args[0], access).await
                    }
                })
            }
            _ => Err(Errno::EINVAL),
        }
    }

    /// Opens the store at the path `path` holds, creating it with `capacity` bytes if `create` and
    /// it doesn't exist, or, if `from` names a store, forking it from that one if it doesn't exist
    /// ([`fork_from`]); unless another connection has it open; and returns its branch.
    fn open(
        &self,
        path: &[u8],
        capacity: u64,
        create: bool,
        from: Option<&[u8]>,
    ) -> Result<u64, Errno> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        if self.store.get().is_some() {
            return Err(Errno::EBUSY);
        }
        let key = store_key(&path);
        {
            let mut paths = open_paths().lock().unwrap();
            if paths.contains_key(&key) {
                return Err(Errno::EBUSY);
            }
            paths.insert(key.clone(), self.this.clone());
        }
        let opened = match from {
            Some(from) if !path.exists() => {
                fork_from(std::ffi::OsStr::from_bytes(from).as_ref(), &path)
                    .and_then(|()| open_store(&path, None))
            }
            _ => open_store(&path, create.then_some(capacity)),
        };
        match opened {
            Ok(store) => {
                let branch = store.branch();
                let store = Arc::new(store);
                let (stop, stopped) = mpsc::channel::<()>();
                let flushed = store.clone();
                let flusher = std::thread::Builder::new()
                    .name("extentd flush".into())
                    .spawn(move || flush_behind(&flushed, &stopped))
                    .map_err(|err| {
                        log::error!("extentd: starting a store's flusher: {err}");
                        Errno::EIO
                    })?;
                *self.flusher.lock().unwrap() = Some((stop, flusher));
                let _ = self.store.set(store);
                let _ = self.path.set(key);
                Ok(branch)
            }
            Err(errno) => {
                open_paths().lock().unwrap().remove(&key);
                Err(errno)
            }
        }
    }

    /// Sets its store's capacity to `capacity` bytes ([`RESIZE`]): grows it, and tells the guest;
    /// or asks the guest to shrink, and shrinks it if the guest does.
    fn resize(&self, capacity: u64) -> Result<(), Errno> {
        let store = self.store.get().ok_or(Errno::EBADF)?;
        let info = store.info()?;
        if capacity >= info.capacity {
            store.set_capacity(capacity)?;
            return self.tell(capacity, false);
        }
        // What the store holds, which no shrink goes below, whatever the guest says.
        if capacity < info.capacity - info.available {
            return Err(Errno::ENOSPC);
        }
        let (answer, answered) = mpsc::channel();
        *self.shrinking.lock().unwrap() = Some((capacity, answer));
        self.tell(capacity, true)?;
        let took = answered.recv_timeout(SHRINK_WAIT);
        self.shrinking.lock().unwrap().take();
        match took {
            Ok(true) => store.set_capacity(capacity),
            Ok(false) => Err(Errno::ENOSPC),
            Err(_) => {
                log::warn!("extentd: no answer from the guest to a shrink to {capacity} bytes");
                Err(Errno::ETIMEDOUT)
            }
        }
    }

    /// Tells the VM process the store's capacity is `capacity` bytes, or, if `ask`, asks its guest
    /// to shrink to it ([`CAPACITY`]).
    fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
        let flags = if ask { CAPACITY_ASK } else { 0 };
        let header = Header::new(CAPACITY, 0).with_args(&[capacity]).with_flags(flags);
        self.sender.send(header, &[], None).map_err(|err| {
            log::warn!("extentd: telling a VM process its capacity: {err}");
            Errno::EIO
        })
    }
}

/// Sets the capacity of the store at the path `path` holds to `capacity` bytes ([`RESIZE`]):
/// through the connection that has it open, if one does; or, if none does, here, growing it.
fn resize(path: &[u8], capacity: u64) -> Result<(), Errno> {
    let key = store_key(&PathBuf::from(std::ffi::OsStr::from_bytes(path)));
    let held = {
        let mut paths = open_paths().lock().unwrap();
        match paths.get(&key) {
            Some(holder) => Some(holder.upgrade().ok_or(Errno::EBUSY)?),
            None => {
                // No connection opens it meanwhile.
                paths.insert(key.clone(), Weak::new());
                None
            }
        }
    };
    if let Some(connection) = held {
        return connection.resize(capacity);
    }
    let resized = (|| {
        let store = ExtentStore::open(&key).map_err(|err| {
            log::warn!("extentd: opening the store at {} to resize it: {err:#}", key.display());
            Errno::ENOENT
        })?;
        // Its guest, which a shrink needs to agree, is not here.
        if capacity < store.info()?.capacity {
            return Err(Errno::EBUSY);
        }
        store.set_capacity(capacity)
    })();
    open_paths().lock().unwrap().remove(&key);
    resized
}

/// fxdax: the bucket `FXDAX_EXTENTD_REMOTE` names, if it names one (empty, it names none), which
/// the stores opened keep their branches in.
pub(crate) fn remote_bucket() -> Result<Option<Arc<extent_remote::store::Store>>, Errno> {
    static BUCKET: OnceLock<Option<Arc<extent_remote::store::Store>>> = OnceLock::new();
    if let Some(bucket) = BUCKET.get() {
        return Ok(bucket.clone());
    }
    let bucket = match std::env::var("FXDAX_EXTENTD_REMOTE") {
        Ok(url) if !url.is_empty() => match extent_remote::store::Store::from_url(&url) {
            Ok(bucket) => {
                let bucket = Arc::new(bucket);
                watch_stats(Arc::downgrade(&bucket));
                Some(bucket)
            }
            Err(err) => {
                log::error!("extentd: FXDAX_EXTENTD_REMOTE: {err}");
                return Err(Errno::EIO);
            }
        },
        _ => None,
    };
    Ok(BUCKET.get_or_init(|| bucket).clone())
}

/// fxdax: how often the bucket's requests and bytes are looked at, and how often, at most, they
/// are logged while they change.
const STATS_POLL: std::time::Duration = std::time::Duration::from_millis(200);
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// fxdax: keeps what `bucket` did, its requests by kind and its bytes up and down
/// (`extent_remote::store::Stats::line`), while it lives: in the file `FXDAX_EXTENTD_STATS` names,
/// if it names one, rewritten whole within [`STATS_POLL`] of each change, which the bench reports
/// (`scripts/bench-report.py`); and in the log every [`STATS_EVERY`] that it changed.
fn watch_stats(bucket: std::sync::Weak<extent_remote::store::Store>) {
    let file = std::env::var_os("FXDAX_EXTENTD_STATS").filter(|file| !file.is_empty());
    let file = file.map(PathBuf::from);
    let watching = std::thread::Builder::new().name("extentd stats".into()).spawn(move || {
        let (mut seen, mut logged) = <(Stats, Stats)>::default();
        let mut at = std::time::Instant::now();
        loop {
            std::thread::sleep(STATS_POLL);
            let Some(bucket) = bucket.upgrade() else { return };
            let stats = bucket.stats();
            if stats != seen {
                if let Some(file) = &file
                    && let Err(err) = write_stats(file, &stats)
                {
                    log::warn!("extentd: writing {}: {err}", file.display());
                }
                seen = stats.clone();
            }
            if stats != logged && at.elapsed() >= STATS_EVERY {
                log::info!("extentd: {}", stats.line());
                (logged, at) = (stats, std::time::Instant::now());
            }
        }
    });
    if let Err(err) = watching {
        log::warn!("extentd: watching object storage's stats: {err}");
    }
}

/// fxdax: replaces `file` with `stats`' line, whole: a reader finds the old line or the new.
fn write_stats(file: &std::path::Path, stats: &Stats) -> std::io::Result<()> {
    let new = file.with_extension("new");
    std::fs::write(&new, stats.line() + "\n")?;
    std::fs::rename(&new, file)
}

/// fxdax: how much of its filesystem the cache leaves free.
const CACHE_FLOOR: u64 = 4 << 30;

/// fxdax: the cache in the directory `FXDAX_EXTENTD_CACHE` names, if it names one (empty, it names
/// none), where what the stores opened read from their bucket is kept.
fn remote_cache() -> Result<Option<Arc<Cache>>, Errno> {
    static CACHE: OnceLock<Option<Arc<Cache>>> = OnceLock::new();
    if let Some(cache) = CACHE.get() {
        return Ok(cache.clone());
    }
    let cache = match std::env::var_os("FXDAX_EXTENTD_CACHE") {
        Some(dir) if !dir.is_empty() => match Cache::new(dir.as_ref(), u64::MAX, CACHE_FLOOR) {
            Ok(cache) => Some(Arc::new(cache)),
            Err(err) => {
                log::error!("extentd: FXDAX_EXTENTD_CACHE: {err}");
                return Err(Errno::EIO);
            }
        },
        _ => None,
    };
    Ok(CACHE.get_or_init(|| cache).clone())
}

/// fxdax: makes `store`'s commits durable within [`FLUSH_EVERY`] of being ordered, until
/// `stopped` says to stop.
fn flush_behind(store: &ExtentStore, stopped: &mpsc::Receiver<()>) {
    while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(FLUSH_EVERY) {
        if store.flush_pending()
            && let Err(err) = store.flush()
        {
            log::error!("extentd: making a store's commits durable: {err:?}");
            return;
        }
    }
}

/// The stage operations a VM process has acknowledged: as it makes them in order, all up to the
/// last.
#[derive(Default)]
struct Acks {
    state: Mutex<AckState>,
    acked: Condvar,
}

#[derive(Default)]
struct AckState {
    /// The last stage operation sent.
    sent: u64,
    /// The last acknowledged.
    acked: u64,
    /// The VM process is gone, and nothing it acknowledges is waited for.
    closed: bool,
    /// fxdax: what is kept until stage operations are acknowledged (`Deferred::keep_until_made`),
    /// with their IDs.
    kept: Vec<(u64, Box<dyn Send>)>,
}

impl Acks {
    /// The ID of the next stage operation.
    fn next(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.sent += 1;
        state.sent
    }

    fn acked(&self, id: u64) {
        let released: Vec<_> = {
            let mut state = self.state.lock().unwrap();
            state.acked = state.acked.max(id);
            self.acked.notify_all();
            let acked = state.acked;
            let (released, kept) =
                std::mem::take(&mut state.kept).into_iter().partition(|(id, _)| *id <= acked);
            state.kept = kept;
            released
        };
        // What the VM process no longer reaches, dropped without holding the lock.
        drop(released);
    }

    /// Keeps `keep` until stage operation `id` is acknowledged, or the VM process is gone.
    fn keep(&self, id: u64, keep: Box<dyn Send>) {
        let mut state = self.state.lock().unwrap();
        if state.acked < id && !state.closed {
            state.kept.push((id, keep));
        }
    }

    /// Waits for stage operation `id` to be acknowledged, or the VM process to be gone.
    fn wait(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        while state.acked < id && !state.closed {
            state = self.acked.wait(state).unwrap();
        }
    }

    fn close(&self) {
        let kept = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.kept)
        };
        self.acked.notify_all();
        drop(kept);
    }
}

/// A window's second stage in a VM process's memory: its changes go to the VM process, which makes
/// them in order before it takes the replies that follow them. One that takes access to pages away
/// is waited for, so that the guest has no access to a page the kernel frees, reuses or shares
/// next.
struct RemoteStage {
    sender: Arc<Sender>,
    acks: Arc<Acks>,
}

thread_local! {
    /// fxdax: while this thread defers waiting for stage operations
    /// (`SecondStage::begin_deferring`), the last one it would have waited for.
    static DEFERRED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

impl RemoteStage {
    fn send(&self, kind: u32, args: &[u64], wait: bool) {
        let id = self.acks.next();
        let header =
            Header::new(kind, id).with_args(args).with_flags(if wait { STAGE_ACK } else { 0 });
        if self.sender.send(header, &[], None).is_err() {
            // The VM process is gone, and its mappings with it.
            return;
        }
        if wait {
            match DEFERRED.get() {
                Some(last) => DEFERRED.set(Some(last.max(id))),
                None => self.acks.wait(id),
            }
        }
    }
}

/// fxdax: stage operations deferred, up to `id`, which the VM process makes in order.
struct Deferral {
    acks: Arc<Acks>,
    id: u64,
}

impl Deferred for Deferral {
    fn wait(&self) {
        self.acks.wait(self.id);
    }

    fn keep_until_made(&self, keep: Box<dyn Send>) {
        self.acks.keep(self.id, keep);
    }
}

impl SecondStage for RemoteStage {
    fn map(
        &self,
        offset: u64,
        _host_addr: *mut u8,
        phys_offset: u64,
        len: u64,
        protection: Protection,
    ) {
        self.send(STAGE_MAP, &[offset, phys_offset, len, protection_code(protection)], false);
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        let wait = protection != Protection::ReadWrite;
        self.send(STAGE_PROTECT, &[offset, len, protection_code(protection)], wait);
    }

    fn begin_deferring(&self) {
        DEFERRED.set(Some(0));
    }

    fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
        let id = DEFERRED.take().filter(|id| *id > 0)?;
        Some(Arc::new(Deferral { acks: self.acks.clone(), id }))
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.send(STAGE_UNMAP, &[offset, len], true);
    }
}

#[cfg(test)]
mod acks_tests {
    use super::Acks;
    use std::sync::Arc;

    /// fxdax: what is kept until a stage operation is acknowledged goes once it, or one after it,
    /// is, or the VM process is: not before.
    #[test]
    fn kept_until_acknowledged() {
        let acks = Acks::default();
        let (first, second, third) = (Arc::new(()), Arc::new(()), Arc::new(()));
        let ids: Vec<u64> = (0..3).map(|_| acks.next()).collect();
        acks.keep(ids[1], Box::new(first.clone()));
        acks.keep(ids[2], Box::new(second.clone()));
        acks.acked(ids[0]);
        assert_eq!((Arc::strong_count(&first), Arc::strong_count(&second)), (2, 2));
        acks.acked(ids[1]);
        assert_eq!((Arc::strong_count(&first), Arc::strong_count(&second)), (1, 2));
        // Acknowledged already, it isn't kept.
        acks.keep(ids[0], Box::new(third.clone()));
        assert_eq!(Arc::strong_count(&third), 1);
        acks.close();
        assert_eq!(Arc::strong_count(&second), 1);
    }
}
