// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's stores, by the paths that name them: those VM processes' connections have open,
//! and what the daemon does to a store by its path, for a connection or the control API
//! ([`super::control`]), holding it meanwhile, so that no connection opens it then.
//!
//! What the control API asks for is a future (TASK-43.6): its waits, for uploads, a guest's
//! answer, and a connection's letting a store go, hold no thread, and its stores' I/O runs on the
//! runtime's blocking threads ([`blocking`]), as do opening and closing a store, which waits for
//! another process's lock of it, and its branch's take from object storage.
//!
//! fxdax: a store open for connections is one instance ([`ExtentStore`]), which its writer's
//! connection, if any, holds the path of, and its followers' follow (backlog doc-8). Followers
//! that find no writer open it as a writer would, and a writer opening it then takes it over
//! ([`ExtentStore::take_over`]), as it does a writer's that went while followers followed it, if
//! nothing is open in it. A writer that went with writes uncommitted retires its store: its
//! followers are cut off, and neither a writer nor a follower opens it until they are gone.

use super::server::Connection;
use crate::kernel::Arena;
use crate::open_stores::{Holder, Paths};
use crate::store_requests::{self, Capacity, Close, Released};
use crate::{Errno, ExtentInfo, ExtentStore};
use extent_remote::cache::Cache;
use extent_remote::store::Stats;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The stores connections have open, which no other may open, with the connections: what a
/// resize of one finds. A store the daemon forks, syncs or resizes while no connection has it
/// is held by none.
pub(super) fn open_paths() -> &'static Paths<Connection> {
    static PATHS: OnceLock<Paths<Connection>> = OnceLock::new();
    PATHS.get_or_init(Paths::new)
}

/// fxdax: the stores open for connections, writers' and followers', by their keys, while any
/// connection has them; and whether retired (see the module docs).
fn instances() -> &'static Mutex<HashMap<PathBuf, (Weak<ExtentStore>, bool)>> {
    static INSTANCES: OnceLock<Mutex<HashMap<PathBuf, (Weak<ExtentStore>, bool)>>> =
        OnceLock::new();
    INSTANCES.get_or_init(Default::default)
}

/// fxdax: the store open at `key` for connections, if one is, and whether it is retired.
fn instance(key: &Path) -> Option<(Arc<ExtentStore>, bool)> {
    let instances = instances().lock().unwrap();
    let (store, retired) = instances.get(key)?;
    Some((store.upgrade()?, *retired))
}

/// fxdax: `store` is the one open at `key` for connections, for as long as one has it.
pub(super) fn register(key: &Path, store: &Arc<ExtentStore>) {
    let mut instances = instances().lock().unwrap();
    instances.retain(|_, (store, _)| store.strong_count() > 0);
    instances.insert(key.to_owned(), (Arc::downgrade(store), false));
}

/// fxdax: the store open at `key`, its writer gone with writes uncommitted, is retired: its
/// followers are cut off, and no connection opens it until they are gone.
pub(super) fn retire(key: &Path, store: &ExtentStore) {
    store.cut_followers();
    if let Some((_, retired)) = instances().lock().unwrap().get_mut(key) {
        *retired = true;
    }
}

/// fxdax: the store open at `key` for followers, or a writer that went, which the writer holding
/// it takes over ([`ExtentStore::take_over`]), if one is: `EBUSY` if it is open in an arena not
/// `arena` ([`in_arena`]), retired, or something is open in it.
pub(super) fn take_over(key: &Path, arena: &Arc<Arena>) -> Result<Option<Arc<ExtentStore>>, Errno> {
    let Some((store, retired)) = instance(key) else { return Ok(None) };
    if let Err(errno) = in_arena(&store, arena) {
        log::warn!("extentd: {} is open for followers in another arena: {errno}", key.display());
        return Err(errno);
    }
    let taken = if retired { Err(Errno::EBUSY) } else { store.take_over() };
    if let Err(errno) = taken {
        log::warn!(
            "extentd: {} is open for followers, which its writer that went left writes uncommitted \
             in: {errno}",
            key.display()
        );
        return Err(errno);
    }
    Ok(Some(store))
}

/// fxdax: the store at `path` for a follower to follow (backlog doc-8): the one a writer's
/// connection, or followers', have open; or, if none has, the store opened as of its last commit,
/// as a writer opens it, its pages of `arena`, which a writer opening it then takes over. `EBUSY`
/// while a writer is opening it, or it is retired, or open in another arena ([`in_arena`]).
pub(super) fn followed(path: &Path, arena: &Arc<Arena>) -> Result<Arc<ExtentStore>, Errno> {
    let open = |key: &Path| match instance(key) {
        Some((_, true)) => Err(Errno::EBUSY),
        Some((store, false)) => in_arena(&store, arena).map(|()| Some(store)),
        None => Ok(None),
    };
    if let Some(store) = open(&store_key(path))? {
        return Ok(store);
    }
    // No other opens it meanwhile: a follower, a writer, or the daemon's work.
    let held = Held::new(path)?;
    if let Some(store) = open(&held.0)? {
        return Ok(store);
    }
    if !path.exists() {
        return Err(Errno::ENOENT);
    }
    let store = Arc::new(open_store(path, None, Some(arena))?);
    register(&held.0, &store);
    Ok(store)
}

/// fxdax: `EBUSY` unless `store`'s own pages are of `arena`: the connection's, which its VM
/// process holds (TASK-79.36). Its window maps no other arena's pages, and the default's read-only
/// alone, its tenant's being its own.
fn in_arena(store: &ExtentStore, arena: &Arc<Arena>) -> Result<(), Errno> {
    if store.arena() == arena.id() { Ok(()) } else { Err(Errno::EBUSY) }
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

    /// Holds the store at `path`, as [`Held::new`] does, holding no thread while it waits.
    async fn acquire(path: &Path) -> Result<Self, Errno> {
        let key = store_key(path);
        open_paths().hold_async(&key, Weak::new(), CLOSING_WAIT).await?;
        Ok(Self(key))
    }

    /// Holds the store at `path`, as [`Held::acquire`] does: `EBUSY` while followers have it open
    /// too.
    async fn alone(path: &Path) -> Result<Self, Errno> {
        let held = Self::acquire(path).await?;
        if instance(&held.0).is_some() {
            return Err(Errno::EBUSY);
        }
        Ok(held)
    }
}

/// Runs `work`, which blocks, on one of the runtime's blocking threads, and returns what it
/// returns.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Errno> + Send + 'static,
) -> Result<T, Errno> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|err| {
        log::error!("extentd: a store's blocking work: {err}");
        Err(Errno::EIO)
    })
}

/// A store the daemon works on, and what holds its path meanwhile, if anything does, dropped in
/// that order (TASK-79.34): the path is let go only once the store, one the daemon opened, is
/// closed, whoever drops it last, a call or a blocking thread whose work a cancelled call left
/// going.
struct HeldStore {
    store: Arc<ExtentStore>,
    _held: Option<Box<dyn Send + Sync>>,
}

impl Borrow<ExtentStore> for HeldStore {
    fn borrow(&self) -> &ExtentStore {
        &self.store
    }
}

/// A store the daemon works on ([`HeldStore`]): one it opened, or one open for connections, which
/// it lets go of on a blocking thread, as closing one it opened blocks, on its last commits'
/// uploads; its blocking work takes it so too.
struct Here(Option<Arc<HeldStore>>);

impl Here {
    /// The store at `path`, opened on a blocking thread ([`open_here`]), which `held` holds.
    async fn open(path: &Path, held: Option<Box<dyn Send + Sync>>) -> Result<Self, Errno> {
        let path = path.to_owned();
        let open = move || Ok(HeldStore { store: Arc::new(open_here(&path)?), _held: held });
        Ok(Self(Some(Arc::new(blocking(open).await?))))
    }

    /// `store`, open for connections, which `held` holds, if anything does.
    fn of(store: Arc<ExtentStore>, held: Option<Box<dyn Send + Sync>>) -> Self {
        Self(Some(Arc::new(HeldStore { store, _held: held })))
    }

    fn store(&self) -> &Arc<HeldStore> {
        self.0.as_ref().expect("an open store")
    }
}

impl Drop for Here {
    fn drop(&mut self) {
        if let Some(store) = self.0.take() {
            drop(tokio::task::spawn_blocking(move || drop(store)));
        }
    }
}

impl Close for Here {
    async fn close(mut self) {
        if let Some(store) = self.0.take() {
            if let Err(err) = tokio::task::spawn_blocking(move || drop(store)).await {
                log::error!("extentd: closing a store: {err}");
            }
        }
    }
}

impl Capacity for Here {
    fn capacity(&self) -> Result<(u64, u64), Errno> {
        capacity(&self.store().store)
    }

    async fn set_capacity(&self, capacity: u64) -> Result<(), Errno> {
        set_capacity(self.store(), capacity).await
    }
}

/// A store open for connections, as a request works on it, which it leaves open.
pub(super) struct Live(pub(super) Arc<ExtentStore>);

impl Close for Live {
    async fn close(self) {}
}

impl Capacity for Live {
    fn capacity(&self) -> Result<(u64, u64), Errno> {
        capacity(&self.0)
    }

    async fn set_capacity(&self, capacity: u64) -> Result<(), Errno> {
        set_capacity(&self.0, capacity).await
    }
}

/// `store`'s capacity, and the bytes it holds.
fn capacity(store: &ExtentStore) -> Result<(u64, u64), Errno> {
    let info = store.info()?;
    Ok((info.capacity, info.capacity - info.available))
}

/// Sets `store`'s capacity to `capacity` bytes, on a blocking thread, which holds it meanwhile.
async fn set_capacity<S>(store: &Arc<S>, capacity: u64) -> Result<(), Errno>
where
    S: Borrow<ExtentStore> + Send + Sync + 'static,
{
    let store = store.clone();
    blocking(move || (*store).borrow().set_capacity(capacity)).await
}

/// The store a fork forks ([`Here`]), and whether connections have it open: their writer's,
/// live, or their followers' (backlog doc-8), rather than opened here.
struct Forked {
    store: Here,
    live: bool,
}

impl Close for Forked {
    async fn close(self) {
        self.store.close().await;
    }
}

/// Returns once every commit of `store` ordered so far is durable and uploaded: its flush on a
/// blocking thread, which holds it meanwhile, the wait for its uploads holding none.
async fn uploaded<S>(store: &Arc<S>) -> Result<(), Errno>
where
    S: Borrow<ExtentStore> + Send + Sync + 'static,
{
    let flushed = store.clone();
    let epoch = blocking(move || (*flushed).borrow().durable_epoch()).await?;
    (**store).borrow().uploaded(epoch).await
}

impl Drop for Held {
    fn drop(&mut self) {
        // Those waiting to open it may.
        open_paths().release(&self.0);
    }
}

/// The store a connection has open at `path`, if one has, whose VM process isn't gone: one going
/// is let go once its last commits are durable, which [`Held::new`] waits for. fxdax: or, with no
/// writer's connection, its followers' (backlog doc-8).
fn open_by_connection(path: &Path) -> Option<Arc<ExtentStore>> {
    let key = store_key(path);
    match open_paths().holder(&key) {
        Some(holder) if holder.going() => None,
        Some(holder) => holder.store().cloned(),
        None => instance(&key).map(|(store, _)| store),
    }
}

/// The store at `path`, created with `capacity` bytes if given and it doesn't exist, keeping its
/// branch in the daemon's bucket, and its own extents' pages in `arena`, which it holds, or in the
/// default arena, without one ([`ExtentStore::open_in`]).
pub(super) fn open_store(
    path: &Path,
    capacity: Option<u64>,
    arena: Option<&Arc<Arena>>,
) -> Result<ExtentStore, Errno> {
    let arena = arena.cloned();
    attached(path, || match capacity {
        Some(capacity) if !path.exists() => {
            ExtentStore::create_in(path, capacity, crate::host_store::new_branch(), arena)
        }
        _ => ExtentStore::open_in(path, arena),
    })
}

/// The store at `path`, as `open` opens it, keeping its branch in the daemon's bucket.
fn attached(
    path: &Path,
    open: impl FnOnce() -> Result<ExtentStore, crate::Error>,
) -> Result<ExtentStore, Errno> {
    let bucket = remote_bucket()?;
    let store = open().map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
    let token = crate::remote::host_token().map_err(|err| {
        log::error!("extentd: {err}");
        Errno::EINVAL
    })?;
    let options = crate::remote::Options { cache: remote_cache()?, token, ..Default::default() };
    if let Err(err) = store.attach_remote(bucket, rand::random(), options) {
        log::warn!("extentd: keeping branch {:x} in object storage: {err}", store.branch());
        return Err(match err {
            crate::remote::Failure::Stale => Errno::ESTALE,
            _ => Errno::EIO,
        });
    }
    Ok(store)
}

/// The store at `path` as the daemon opens it to work on it, not a VM: its pages of the default
/// arena, which reads no boot profile into the cache ([`ExtentStore::open_without_profile`]).
fn open_here(path: &Path) -> Result<ExtentStore, Errno> {
    attached(path, || ExtentStore::open_without_profile(path))
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
pub(super) async fn create(
    path: PathBuf,
    capacity: u64,
    branch: Option<u64>,
    token: Option<u64>,
) -> Result<u64, Errno> {
    let held = Held::acquire(&path).await?;
    blocking(move || {
        // Let go of once the work is done, though the call may give up on it before.
        let _held = held;
        if path.exists() {
            return Err(Errno::EEXIST);
        }
        let branch =
            branch.filter(|branch| *branch != 0).unwrap_or_else(crate::host_store::new_branch);
        let created = ExtentStore::create_for_branch(&path, capacity, branch);
        drop(created.map_err(|err| errno_of(format_args!("creating {}", path.display()), err))?);
        if let Some(token) = token {
            ExtentStore::place(&path, token)
                .map_err(|err| errno_of(format_args!("placing {}", path.display()), err))?;
        }
        Ok(branch)
    })
    .await
}

/// Makes a store at `path`, which must not exist (`EEXIST`), of `capacity` bytes, holding a
/// filesystem of the container image at `image` (`fxdax::import::store`), and returns its branch.
pub(super) async fn import(
    path: PathBuf,
    capacity: u64,
    image: PathBuf,
    reference: Option<String>,
) -> Result<u64, Errno> {
    let held = Held::acquire(&path).await?;
    blocking(move || {
        let _held = held;
        if path.exists() {
            return Err(Errno::EEXIST);
        }
        let made = crate::import::store(&path, capacity, &image, reference.as_deref());
        futures::executor::block_on(made)
            .map_err(|err| errno_of(format_args!("importing {}", image.display()), err))?;
        let store = ExtentStore::open(&path)
            .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
        Ok(store.branch())
    })
    .await
}

/// The store at `path`, or, if it doesn't exist, one of `branch`, of `capacity` bytes, taken from
/// the daemon's bucket: what the branch holds there.
fn take_store(path: &Path, branch: u64, capacity: u64) -> Result<ExtentStore, Errno> {
    if !path.exists() {
        if branch == 0 {
            return Err(Errno::ENOENT);
        }
        let created = ExtentStore::create_for_branch(path, capacity, branch);
        drop(created.map_err(|err| {
            errno_of(format_args!("taking branch {branch:x} to {}", path.display()), err)
        })?);
    }
    open_here(path)
}

/// Finishes deleting the fork at `path`, if its fork failed ([`ExtentStore::fork_failed`]), and
/// its branch in the daemon's bucket, as [`delete`] would: a deletion of its that stopped partway
/// (TASK-79.84). Its holder holds `path`.
pub(super) fn deleted_if_failed(path: &Path) -> Result<(), Errno> {
    if !ExtentStore::fork_failed(path) {
        return Ok(());
    }
    let bucket = remote_bucket()?;
    ExtentStore::delete(path, Some(&bucket)).map_err(|err| {
        errno_of(format_args!("deleting {}, a fork that failed", path.display()), err)
    })
}

/// Forks the store at `from`, the point a snapshot of a VM was taken at, to a new store at `path`,
/// a branch of its own, which the VM restored from the snapshot opens (TASK-39.1). Restores of one
/// snapshot fork its point one at a time, so that two, a zygote's, do not find it busy; a
/// connection having it open makes it so (`EBUSY`), unless its VM process is gone. A point that
/// doesn't exist is taken from the daemon's bucket, a store of `branch` of `capacity` bytes, as on
/// another host than the snapshot's.
pub(super) fn fork_from(from: &Path, branch: u64, capacity: u64, path: &Path) -> Result<(), Errno> {
    let held = Held::new(from)?;
    // fxdax: the point open for followers, forked as it is.
    let store = match instance(&held.0) {
        Some((store, _)) => store,
        None => Arc::new(take_store(from, branch, capacity)?),
    };
    store.fork(path, crate::host_store::new_branch()).map_err(|err| {
        errno_of(format_args!("forking {} to {}", from.display(), path.display()), err)
    })
}

/// Forks the store at `from` to a new branch, `branch`, at `to`: live, if a connection has it
/// open, once nothing is open (`EBUSY` otherwise); fxdax: or its followers, with no writer; or
/// opened here, if none has, which no connection opens meanwhile. Its commits' uploads, which the
/// fork's branch follows, are waited for first, holding no thread; the fork, as of then, is made
/// on a blocking thread.
pub(super) async fn fork(from: PathBuf, to: PathBuf, branch: u64) -> Result<(), Errno> {
    let key = store_key(&from);
    let live = |connection: &Connection| {
        let store = connection.store().cloned()?;
        Some(Forked { store: Here::of(store, None), live: true })
    };
    let open = |held: Option<Released<'static, Connection>>| async {
        let held = held.map(|held| Box::new(held) as Box<dyn Send + Sync>);
        match instance(&key) {
            Some((store, _)) => Ok(Forked { store: Here::of(store, held), live: true }),
            None => Ok(Forked { store: Here::open(&from, held).await?, live: false }),
        }
    };
    let forking = store_requests::forking(open_paths(), &key, live, CLOSING_WAIT, open).await?;
    let (store, live) = (forking.store.store().clone(), forking.live);
    let forked = async {
        uploaded(&store).await?;
        blocking(move || {
            let store = &store.store;
            let forked =
                if live { store.fork_quiescent(&to, branch) } else { store.fork(&to, branch) };
            forked.map_err(|err| errno_of(format_args!("forking to {}", to.display()), err))
        })
        .await
    }
    .await;
    forking.close().await;
    forked
}

/// Waits until every commit of the store at `path` is in the daemon's bucket, holding no thread:
/// through the connection's store, if one has it open; or opened here, which uploads what it
/// holds, if none has.
pub(super) async fn sync(path: PathBuf) -> Result<(), Errno> {
    if let Some(store) = open_by_connection(&path) {
        return uploaded(&store).await;
    }
    let held = Held::acquire(&path).await?;
    let here = Here::open(&path, Some(Box::new(held))).await?;
    let synced = uploaded(here.store()).await;
    here.close().await;
    synced
}

/// Deletes the store at `path`, or the fork whose handle it is, and its branch in the daemon's
/// bucket ([`ExtentStore::delete`]): `EBUSY` while a connection has it open.
pub(super) async fn delete(path: PathBuf) -> Result<(), Errno> {
    let held = Held::alone(&path).await?;
    blocking(move || {
        let _held = held;
        let bucket = remote_bucket()?;
        ExtentStore::delete(&path, Some(&bucket))
            .map_err(|err| errno_of(format_args!("deleting {}", path.display()), err))
    })
    .await
}

/// Sets the capacity of the store at `path` to `capacity` bytes: through the connection that has
/// it open, if one does, which grows it, or shrinks it if its guest agrees, holding no thread while
/// it waits for the guest's answer; or, if none does, here, growing it (`EBUSY` to shrink it, its
/// guest not here to agree), no connection opening it meanwhile.
pub(super) async fn resize(path: PathBuf, capacity: u64) -> Result<(), Errno> {
    let key = store_key(&path);
    let open = |held: Option<Released<'static, Connection>>| {
        let key = key.clone();
        let held = held.map(|held| Box::new(held) as Box<dyn Send + Sync>);
        blocking(move || {
            // fxdax: open for followers, it grows as it is.
            let store = match instance(&key) {
                Some((store, _)) => store,
                None => Arc::new(ExtentStore::open_without_profile(&key).map_err(|err| {
                    let path = key.display();
                    log::warn!("extentd: opening the store at {path} to resize it: {err:#}");
                    Errno::ENOENT
                })?),
            };
            Ok(Here::of(store, held))
        })
    };
    store_requests::resize(open_paths(), &key, capacity, open).await
}

/// Places the store at `path`, or the fork whose handle it is, at the placement whose fencing
/// token is `token` ([`ExtentStore::place`]): `EBUSY` while a connection has it open, whose
/// writer took its branch with the token it had.
pub(super) async fn place(path: PathBuf, token: u64) -> Result<(), Errno> {
    let held = Held::alone(&path).await?;
    blocking(move || {
        let _held = held;
        ExtentStore::place(&path, token)
            .map_err(|err| errno_of(format_args!("placing {}", path.display()), err))
    })
    .await
}

/// Makes the store at `path` a public base, and the branches it inherited from
/// ([`ExtentStore::make_public`]): `EBUSY` while a connection has it open, whose guest could
/// write it still.
pub(super) async fn make_public(path: PathBuf) -> Result<(), Errno> {
    let held = Held::alone(&path).await?;
    blocking(move || {
        let _held = held;
        let store = ExtentStore::open_without_profile(&path)
            .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
        store.make_public()
    })
    .await
}

/// What the store at `path` is, and holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StoreState {
    pub branch: u64,
    pub info: ExtentInfo,
    /// Whose the bytes its extents hold are (TASK-49).
    pub bytes: crate::StoreBytes,
    /// Whether it is a public base.
    pub public: bool,
    /// Whether a connection has it open.
    pub open: bool,
    /// The epoch of its latest commit.
    pub committed: u64,
    /// What its uploader has done and has left to do, if its commits upload now.
    pub upload: Option<crate::remote::UploadStatus>,
}

/// What `store` is, and holds; `open` if a connection has it open.
fn state_of(store: &ExtentStore, open: bool) -> Result<StoreState, Errno> {
    Ok(StoreState {
        branch: store.branch(),
        info: store.info()?,
        bytes: store.bytes()?,
        public: store.is_public(),
        open,
        committed: store.committed()?,
        upload: store.upload_status()?,
    })
}

/// The stores connections have open, writers' and followers', by their paths, with what each is
/// and holds (TASK-50.5).
pub(super) fn list() -> Result<Vec<(PathBuf, StoreState)>, Errno> {
    let stores: Vec<(PathBuf, Arc<ExtentStore>)> = {
        let instances = instances().lock().unwrap();
        (instances.iter())
            .filter_map(|(path, (store, _))| Some((path.clone(), store.upgrade()?)))
            .collect()
    };
    stores.into_iter().map(|(path, store)| Ok((path, state_of(&store, true)?))).collect()
}

/// What the store at `path` is, and holds: as the connection that has it open has it, if one does.
pub(super) async fn stat(path: PathBuf) -> Result<StoreState, Errno> {
    if let Some(store) = open_by_connection(&path) {
        return state_of(&store, true);
    }
    let held = Held::acquire(&path).await?;
    blocking(move || {
        let _held = held;
        if !path.exists() {
            return Err(Errno::ENOENT);
        }
        let store = ExtentStore::open_without_profile(&path)
            .map_err(|err| errno_of(format_args!("opening {}", path.display()), err))?;
        state_of(&store, false)
    })
    .await
}

/// The bucket [`remote_bucket`] made, once a store needs it.
static BUCKET: OnceLock<Arc<extent_remote::store::Store>> = OnceLock::new();

/// What the daemon did with its bucket so far, if it made it (`metrics`).
pub(super) fn bucket_stats() -> Option<extent_remote::store::Stats> {
    BUCKET.get().map(|bucket| bucket.stats())
}

/// How many stores connections have open, to write (`metrics`).
pub(super) fn open_count() -> usize {
    open_paths().held_by_holders()
}

/// fxdax: the stores open for connections, writers' and followers'.
fn open_instances() -> Vec<Arc<ExtentStore>> {
    let instances = instances().lock().unwrap();
    instances.values().filter_map(|(store, _)| store.upgrade()).collect()
}

/// The branches of the stores open for connections whose commits upload now, and what their
/// uploaders have left to do (`metrics`, TASK-91.3).
pub(super) fn upload_statuses() -> Vec<(u64, crate::remote::UploadStatus)> {
    (open_instances().iter())
        .filter_map(|store| Some((store.branch(), store.upload_status().ok()??)))
        .collect()
}

/// How many views followers have of the stores open for connections (`metrics`): counted of the
/// views alone, which hold none of the stores' state, their guests' (TASK-92.17).
pub(super) fn follower_views() -> usize {
    open_instances().iter().map(|store| store.view_stats().views).sum()
}

/// fxdax: the bucket `FXDAX_EXTENTD_REMOTE` names, which the stores opened keep their branches in:
/// `EIO` if it names none, as the daemon's host must (`crate::remote::Host`, TASK-45).
pub(crate) fn remote_bucket() -> Result<Arc<extent_remote::store::Store>, Errno> {
    if let Some(bucket) = BUCKET.get() {
        return Ok(bucket.clone());
    }
    let host = crate::remote::Host::from_env().map_err(|err| {
        log::error!("extentd: {err}");
        Errno::EIO
    })?;
    let bucket = extent_remote::store::Store::from_url(&host.bucket).map_err(|err| {
        log::error!("extentd: FXDAX_EXTENTD_REMOTE: {err}");
        Errno::EIO
    })?;
    Ok(BUCKET
        .get_or_init(|| {
            let bucket = Arc::new(bucket);
            watch_stats(Arc::downgrade(&bucket));
            bucket
        })
        .clone())
}

/// fxdax: how often the bucket's requests and bytes are looked at, and how often, at most, they
/// are logged while they change.
const STATS_POLL: std::time::Duration = std::time::Duration::from_millis(200);
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// fxdax: what the stores' followers are (backlog doc-8, TASK-42.7): their views now, and the
/// most views, commits behind, time behind and bytes of frees kept for them seen since the daemon
/// started, which the followers' measurements report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FollowerStats {
    views: usize,
    max_views: usize,
    max_behind: u64,
    max_stale_ms: u64,
    max_kept: u64,
}

impl FollowerStats {
    /// Takes the stores open for connections as they are now: those followers view alone, as
    /// those no follower views keep nothing for them, whose state their guests' requests hold is
    /// then taken for the bytes of frees views keep alone, which it counts (TASK-92.17).
    fn update(&mut self) {
        self.views = 0;
        for store in open_instances() {
            let views = store.view_stats();
            if views.views == 0 {
                continue;
            }
            self.views += views.views;
            self.max_behind = self.max_behind.max(views.behind);
            self.max_stale_ms = self.max_stale_ms.max(views.stale_for.as_millis() as u64);
            if let Ok(kept) = store.kept_for_views() {
                self.max_kept = self.max_kept.max(kept);
            }
        }
        self.max_views = self.max_views.max(self.views);
    }

    fn line(&self) -> String {
        format!(
            "followers: views={} max-views={} max-behind={} max-stale-ms={} max-kept={}",
            self.views, self.max_views, self.max_behind, self.max_stale_ms, self.max_kept
        )
    }
}

/// fxdax: keeps what `bucket` did, its requests by kind and its bytes up and down
/// (`extent_remote::store::Stats::line`), what the stores' followers are ([`FollowerStats`]),
/// what serving the stores cost ([`daemon_line`]) and the requests served
/// (`super::metrics::requests_line`), while it lives: in the file `FXDAX_EXTENTD_STATS` names, if
/// it names one, a line each, rewritten whole within [`STATS_POLL`] of each change, which the
/// bench reports (`scripts/bench-report.py`); and the bucket's in the log every [`STATS_EVERY`]
/// that it changed. fxdax: what it keeps but the bucket's, it looks at only to keep it in the file
/// (TASK-92.17).
fn watch_stats(bucket: std::sync::Weak<extent_remote::store::Store>) {
    let file = std::env::var_os("FXDAX_EXTENTD_STATS").filter(|file| !file.is_empty());
    let file = file.map(PathBuf::from);
    let watching = std::thread::Builder::new().name("extentd stats".into()).spawn(move || {
        let mut logged = Stats::default();
        let mut followers = FollowerStats::default();
        let mut written = Vec::new();
        let mut at = std::time::Instant::now();
        loop {
            std::thread::sleep(STATS_POLL);
            let Some(bucket) = bucket.upgrade() else { return };
            let stats = bucket.stats();
            if let Some(file) = &file {
                followers.update();
                let lines = [
                    stats.line(),
                    followers.line(),
                    daemon_line(),
                    super::metrics::requests_line(),
                ];
                if lines[..] != written[..] {
                    if let Err(err) = write_stats(file, &lines) {
                        log::warn!("extentd: writing {}: {err}", file.display());
                    }
                    written = lines.to_vec();
                }
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

/// fxdax: what serving the stores cost the daemon (`crate::stats::Stats::named`), as a line of
/// `NAME=COUNT` after `daemon:`.
fn daemon_line() -> String {
    let named = crate::stats::stats().named().map(|(name, count)| format!("{name}={count}"));
    format!("daemon: {}", named.join(" "))
}

/// fxdax: replaces `file` with `lines`, whole: a reader finds the old lines or the new.
fn write_stats(file: &std::path::Path, lines: &[String]) -> std::io::Result<()> {
    let new = file.with_extension("new");
    std::fs::write(&new, lines.join("\n") + "\n")?;
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
