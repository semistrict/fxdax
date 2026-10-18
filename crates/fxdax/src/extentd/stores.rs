// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's stores, by the paths that name them: those VM processes' connections have open,
//! and what the daemon does to a store by its path, for a connection or the control API
//! ([`super::control`]), holding it meanwhile, so that no connection opens it then. These block:
//! on the store's I/O, and on waits (for uploads, for a guest's answer, for a store to be let go).

use super::server::Connection;
use crate::open_stores::{Holder, Paths};
use crate::{Errno, ExtentInfo, ExtentStore};
use extent_remote::cache::Cache;
use extent_remote::store::Stats;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

/// The stores connections have open, which no other may open, with the connections: what a
/// resize of one finds. A store the daemon forks, syncs or resizes while no connection has it
/// is held by none.
pub(super) fn open_paths() -> &'static Paths<Connection> {
    static PATHS: OnceLock<Paths<Connection>> = OnceLock::new();
    PATHS.get_or_init(Paths::new)
}

/// How long holding a store waits for a connection whose VM process is gone to let it go: as a VM
/// restored from a snapshot at once opens the store its VM had (TASK-39.1).
pub(super) const CLOSING_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The path that names the store at `path` in [`open_paths`]: whichever way a request names it.
pub(super) fn store_key(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    // A store not yet made: where it will be.
    match (path.parent().map(std::fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_owned(),
    }
}

/// The daemon holding a store, which no connection opens meanwhile, until it is dropped.
struct Held(PathBuf);

impl Held {
    /// Holds the store at `path`: `EBUSY` while a connection has it open, once a connection whose
    /// VM process is gone lets it go, waiting at most [`CLOSING_WAIT`].
    fn new(path: &Path) -> Result<Self, Errno> {
        let key = store_key(path);
        open_paths().hold(&key, Weak::new(), Some(CLOSING_WAIT))?;
        Ok(Self(key))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Those waiting to open it may.
        open_paths().release(&self.0);
    }
}

/// The store a connection has open at `path`, if one has, whose VM process isn't gone: one going
/// is let go once its last commits are durable, which [`Held::new`] waits for.
fn open_by_connection(path: &Path) -> Option<Arc<ExtentStore>> {
    let holder = open_paths().holder(&store_key(path)).filter(|holder| !holder.going())?;
    holder.store().cloned()
}

/// The store at `path`, created with `capacity` bytes if given and it doesn't exist, keeping its
/// branch in the daemon's bucket, if it has one, and its own extents' pages in `arena`
/// ([`ExtentStore::open_in`]).
pub(super) fn open_store(
    path: &Path,
    capacity: Option<u64>,
    arena: crate::kernel::ArenaId,
) -> Result<ExtentStore, Errno> {
    let bucket = remote_bucket()?;
    let opened = match capacity {
        Some(capacity) if !path.exists() => {
            ExtentStore::create_in(path, capacity, crate::storage::new_branch(), arena)
        }
        _ => ExtentStore::open_in(path, arena),
    };
    let store = opened.map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
    if let Some(bucket) = bucket {
        let token = crate::remote::host_token().map_err(|err| {
            log::error!("extentd: {err}");
            Errno::EINVAL
        })?;
        let options =
            crate::remote::Options { cache: remote_cache()?, token, ..Default::default() };
        if let Err(err) = store.attach_remote(bucket, rand::random(), options) {
            log::warn!("extentd: keeping branch {:x} in object storage: {err}", store.branch());
            return Err(match err {
                crate::remote::Failure::Stale => Errno::ESTALE,
                _ => Errno::EIO,
            });
        }
    }
    Ok(store)
}

/// The store at `path` as the daemon opens it to work on it, not a VM: its pages of the default
/// arena.
fn open_here(path: &Path) -> Result<ExtentStore, Errno> {
    open_store(path, None, crate::kernel::ArenaId::DEFAULT)
}

/// The errno of `err`, which doing `what` failed with: its own; `EBUSY` if it gave up waiting for
/// another process's lock of the store; or `EIO`, logged.
pub(super) fn errno_of(what: std::fmt::Arguments<'_>, err: crate::Error) -> Errno {
    // Another process's lock of the store, waited for: `EBUSY`, as for a connection's.
    if err.is_busy() {
        return Errno::EBUSY;
    }
    err.errno().unwrap_or_else(|| {
        log::warn!("extentd: {what}: {err:#}");
        Errno::EIO
    })
}

/// Makes an empty store at `path`, which must not exist (`EEXIST`), of `capacity` bytes, and
/// returns its branch: a new one, or `branch`, if given, whose store kept in the daemon's bucket
/// the daemon takes once it opens it (`fxdaxctl take`); placed at `token` if given
/// ([`place`]).
pub(super) fn create(
    path: &Path,
    capacity: u64,
    branch: Option<u64>,
    token: Option<u64>,
) -> Result<u64, Errno> {
    let _held = Held::new(path)?;
    if path.exists() {
        return Err(Errno::EEXIST);
    }
    let branch = branch.filter(|branch| *branch != 0).unwrap_or_else(crate::storage::new_branch);
    let created = ExtentStore::create_for_branch(path, capacity, branch);
    drop(created.map_err(|err| errno_of(format_args!("creating {}", path.display()), err))?);
    if let Some(token) = token {
        ExtentStore::place(path, token)
            .map_err(|err| errno_of(format_args!("placing {}", path.display()), err))?;
    }
    Ok(branch)
}

/// Makes a store at `path`, which must not exist (`EEXIST`), of `capacity` bytes, holding a
/// filesystem of the container image at `image` (`fxdax::import::store`), and returns its branch.
pub(super) fn import(
    path: &Path,
    capacity: u64,
    image: &Path,
    reference: Option<&str>,
) -> Result<u64, Errno> {
    let _held = Held::new(path)?;
    if path.exists() {
        return Err(Errno::EEXIST);
    }
    let imported =
        futures::executor::block_on(crate::import::store(path, capacity, image, reference));
    imported.map_err(|err| errno_of(format_args!("importing {}", image.display()), err))?;
    let store = ExtentStore::open(path)
        .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
    Ok(store.branch())
}

/// The store at `path`, or, if it doesn't exist, one of `branch`, of `capacity` bytes, taken from
/// the daemon's bucket: what the branch holds there.
fn take_store(path: &Path, branch: u64, capacity: u64) -> Result<ExtentStore, Errno> {
    if !path.exists() {
        if remote_bucket()?.is_none() || branch == 0 {
            return Err(Errno::ENOENT);
        }
        let created = ExtentStore::create_for_branch(path, capacity, branch);
        drop(created.map_err(|err| {
            errno_of(format_args!("taking branch {branch:x} to {}", path.display()), err)
        })?);
    }
    open_here(path)
}

/// Forks the store at `from`, the point a snapshot of a VM was taken at, to a new store at `path`,
/// a branch of its own, which the VM restored from the snapshot opens (TASK-39.1). Restores of one
/// snapshot fork its point one at a time, so that two, a zygote's, do not find it busy; a
/// connection having it open makes it so (`EBUSY`), unless its VM process is gone. A point that
/// doesn't exist is taken from the daemon's bucket, a store of `branch` of `capacity` bytes, as on
/// another host than the snapshot's; without a bucket, `ENOENT`.
pub(super) fn fork_from(from: &Path, branch: u64, capacity: u64, path: &Path) -> Result<(), Errno> {
    let _held = Held::new(from)?;
    let store = take_store(from, branch, capacity)?;
    store.fork(path, rand::random::<u64>().max(1)).map_err(|err| {
        errno_of(format_args!("forking {} to {}", from.display(), path.display()), err)
    })
}

/// Forks the store at `from` to a new branch, `branch`, at `to`: live, if a connection has it
/// open, once nothing is open (`EBUSY` otherwise); or opened here, if none has.
pub(super) fn fork(from: &Path, to: &Path, branch: u64) -> Result<(), Errno> {
    let forking = |err| errno_of(format_args!("forking to {}", to.display()), err);
    if let Some(store) = open_by_connection(from) {
        return store.fork_quiescent(to, branch).map_err(forking);
    }
    let _held = Held::new(from)?;
    open_here(from)?.fork(to, branch).map_err(forking)
}

/// Waits until every commit of the store at `path` is in the daemon's bucket (`ENOTSUP` without
/// one): through the connection's store, if one has it open; or opened here, which uploads what
/// it holds, if none has.
pub(super) fn sync(path: &Path) -> Result<(), Errno> {
    if remote_bucket()?.is_none() {
        return Err(Errno::ENOTSUP);
    }
    if let Some(store) = open_by_connection(path) {
        return store.wait_uploaded();
    }
    let _held = Held::new(path)?;
    open_here(path)?.wait_uploaded()
}

/// Deletes the store at `path`, or the fork whose handle it is, and its branch in the daemon's
/// bucket ([`ExtentStore::delete`]): `EBUSY` while a connection has it open.
pub(super) fn delete(path: &Path) -> Result<(), Errno> {
    let _held = Held::new(path)?;
    let bucket = remote_bucket()?;
    ExtentStore::delete(path, bucket.as_deref())
        .map_err(|err| errno_of(format_args!("deleting {}", path.display()), err))
}

/// Sets the capacity of the store at `path` to `capacity` bytes: through the connection that has
/// it open, if one does, which grows it, or shrinks it if its guest agrees; or, if none does, here,
/// growing it (`EBUSY` to shrink it, its guest not here to agree).
pub(super) fn resize(path: &Path, capacity: u64) -> Result<(), Errno> {
    let key = store_key(path);
    // If no connection has it, none opens it meanwhile.
    if let Some(connection) = open_paths().holder_or_hold(&key)? {
        return connection.resize(capacity);
    }
    let held = Held(key);
    let store = ExtentStore::open(&held.0).map_err(|err| {
        log::warn!("extentd: opening the store at {} to resize it: {err:#}", held.0.display());
        Errno::ENOENT
    })?;
    if capacity < store.info()?.capacity {
        return Err(Errno::EBUSY);
    }
    store.set_capacity(capacity)
    // Dropped, `held` lets the connections waiting to open it know (it once didn't, and they
    // waited out CLOSING_WAIT: found by open_stores.rs's Shuttle tests, TASK-44.6).
}

/// Places the store at `path`, or the fork whose handle it is, at the placement whose fencing
/// token is `token` ([`ExtentStore::place`]): `EBUSY` while a connection has it open, whose
/// writer took its branch with the token it had.
pub(super) fn place(path: &Path, token: u64) -> Result<(), Errno> {
    let _held = Held::new(path)?;
    ExtentStore::place(path, token)
        .map_err(|err| errno_of(format_args!("placing {}", path.display()), err))
}

/// Makes the store at `path` a public base, and the branches it inherited from
/// ([`ExtentStore::make_public`]): `EBUSY` while a connection has it open, whose guest could
/// write it still.
pub(super) fn make_public(path: &Path) -> Result<(), Errno> {
    let _held = Held::new(path)?;
    let store = ExtentStore::open(path)
        .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
    store.make_public()
}

/// What the store at `path` is, and holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StoreState {
    pub branch: u64,
    pub info: ExtentInfo,
    /// Whether it is a public base.
    pub public: bool,
    /// Whether a connection has it open.
    pub open: bool,
}

/// What the store at `path` is, and holds: as the connection that has it open has it, if one does.
pub(super) fn stat(path: &Path) -> Result<StoreState, Errno> {
    let state = |store: &ExtentStore, open| {
        Ok(StoreState {
            branch: store.branch(),
            info: store.info()?,
            public: store.is_public(),
            open,
        })
    };
    if let Some(store) = open_by_connection(path) {
        return state(&store, true);
    }
    let _held = Held::new(path)?;
    if !path.exists() {
        return Err(Errno::ENOENT);
    }
    let store = ExtentStore::open(path)
        .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
    state(&store, false)
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
