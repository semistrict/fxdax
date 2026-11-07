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
//! Who holds a store, and what is open of it, is the daemon's one table's (`crate::store_table`,
//! TASK-160.8), each hold a value that lets the store go as it is dropped. fxdax: a store open for
//! connections is one instance ([`ExtentStore`]), its writer's, if a connection writes it, and its
//! followers', which follow it (backlog doc-8). Followers that find no writer open it as a writer
//! would, and a writer opening it then takes it over ([`ExtentStore::take_over`]), as it does a
//! writer's that went while followers followed it, if nothing is open in it. A writer that went
//! with writes uncommitted retires its store: its followers are cut off, and neither a writer nor
//! a follower opens it until they are gone.

use super::server::Connection;
use crate::extents::Opening;
use crate::pages::Arena;
use crate::store_requests::{self, Capacity, Close};
use crate::store_table::{self, StoreTable, Writer as _};
use crate::{Errno, ExtentInfo, ExtentStore};
use extent_remote::cache::Cache;
use extent_remote::store::Stats;
use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

/// The daemon's stores, by their keys ([`store_key`]): who holds each, a connection writing it or
/// the daemon working on it, and what is open of it, the store connections have open or a point
/// kept open for forks.
pub(super) type Table = StoreTable<Connection, ExtentStore, Point>;

/// A store held ([`table`]): by a connection writing it, or by the daemon working on it, which no
/// connection opens meanwhile; until it is dropped.
pub(super) type Hold = store_table::Hold<'static, Connection, ExtentStore, Point>;

/// The daemon's stores ([`Table`]).
pub(super) fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(StoreTable::new)
}

/// Holds the store `key` names for `connection` to write it, once a connection whose VM process
/// is gone lets it go, waiting at most [`CLOSING_WAIT`] (`EBUSY`), holding no thread meanwhile:
/// `EBUSY` while a connection not going writes it. fxdax: the point kept open of it for forks,
/// which the hold takes, closed, once the forks of it under way are done.
pub(super) async fn hold_to_write(key: &Path, connection: Weak<Connection>) -> Result<Hold, Errno> {
    let (hold, point) = table().hold_to_write(key, connection, CLOSING_WAIT).await?;
    close_point(point).await;
    Ok(hold)
}

/// fxdax: the store followers have open at the key `hold` holds for a writer, or a writer's that
/// went, which the writer takes over ([`ExtentStore::take_over`]), if one is: `EBUSY` if it is
/// open in an arena not `arena` ([`in_arena`]), retired, or something is open in it; and as the
/// store refuses a writer, a point or a public base ([`crate::StoreAccess::may_write`]).
pub(super) fn take_over(
    hold: &Hold,
    arena: &Arc<Arena>,
) -> Result<Option<Arc<ExtentStore>>, Errno> {
    let Some((store, retired)) = hold.instance() else { return Ok(None) };
    let key = hold.key();
    if let Err(errno) = in_arena(&store, arena) {
        log::warn!("extentd: {} is open for followers in another arena: {errno}", key.display());
        return Err(errno);
    }
    match if retired { Err(Errno::EBUSY) } else { store.take_over() } {
        Ok(()) => Ok(Some(store)),
        Err(Errno::EBUSY) => {
            log::warn!(
                "extentd: {} is open for followers, which its writer that went left writes \
                 uncommitted in",
                key.display()
            );
            Err(Errno::EBUSY)
        }
        Err(errno) => Err(errno),
    }
}

/// fxdax: retires `store`, which the connection holding it as `hold` wrote, gone with writes
/// uncommitted: marked so, and then its followers cut off ([`Hold::retire`]), and no connection
/// opens it until they are gone.
pub(super) fn retire(hold: &Hold, store: &ExtentStore) {
    hold.retire(|| store.cut_followers());
}

/// fxdax: a follower of the store at `path` (backlog doc-8): of the one a writer's connection, or
/// followers', have open ([`crate::store_table::StoreTable::follow`]); or, if none has, of the
/// store opened as of its last commit, as a writer opens it, its pages of `arena`, which a writer
/// opening it then takes over. `EBUSY` while a writer is opening it, or it is retired, or as it is
/// followed, or open in another arena ([`in_arena`]).
pub(super) fn followed(path: &Path, arena: &Arc<Arena>) -> Result<ExtentStore, Errno> {
    let follow = |store: &Arc<ExtentStore>| in_arena(store, arena).map(|()| store.follow());
    let key = store_key(path);
    if let Some(follower) = table().follow(&key, follow)? {
        return Ok(follower);
    }
    // No other opens it meanwhile: a follower, a writer, or the daemon's work; nor retires it,
    // which only its writer does.
    let hold = hold_blocking(&key)?;
    match hold.instance() {
        Some((_, true)) => return Err(Errno::EBUSY),
        Some((store, false)) => return follow(&store),
        None => {}
    }
    if !path.exists() {
        return Err(Errno::ENOENT);
    }
    let store = Arc::new(open_store(path, None, Some(arena))?);
    hold.open(&store)?;
    Ok(store.follow())
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

/// The path that names the store at `path` in the daemon's [`table`]: whichever way a request
/// names it.
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

/// Holds the store `key` names for the daemon, which no connection opens meanwhile: `EBUSY` while
/// a connection writes it, once a connection whose VM process is gone lets it go, waiting at most
/// [`CLOSING_WAIT`]. fxdax: the point kept open of it for forks, if any, closed: the daemon's work
/// on it but forking it has it alone (TASK-92.5). It blocks.
fn hold_blocking(key: &Path) -> Result<Hold, Errno> {
    let hold = table().hold(key, Some(CLOSING_WAIT))?;
    close_point_blocking(hold.take_point());
    Ok(hold)
}

/// Holds the store at `path` for the daemon, as [`hold_blocking`] does, holding no thread while
/// it waits.
async fn hold(path: &Path) -> Result<Hold, Errno> {
    let hold = hold_to_fork(path).await?;
    close_point(hold.take_point()).await;
    Ok(hold)
}

/// fxdax: holds the store at `path` for the daemon, as [`hold`] does, but for the point kept open
/// of it, which it leaves open: to fork it, or open it to ([`point`]).
async fn hold_to_fork(path: &Path) -> Result<Hold, Errno> {
    table().hold_async(&store_key(path), CLOSING_WAIT).await
}

/// Holds the store at `path` for the daemon, as [`hold`] does: `EBUSY` while followers have it
/// open too.
async fn hold_alone(path: &Path) -> Result<Hold, Errno> {
    let hold = hold(path).await?;
    if hold.instance().is_some() {
        return Err(Errno::EBUSY);
    }
    Ok(hold)
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

/// A store the daemon works on, and what it lets go of once the store is closed, if anything,
/// dropped in that order (TASK-79.34): the daemon's hold of its path is let go only once the
/// store, one the daemon opened, is closed, whoever drops it last, a call or a blocking thread
/// whose work a cancelled call left going.
pub(super) struct HeldStore {
    store: Arc<ExtentStore>,
    _then: Option<Then>,
}

/// What a store the daemon works on lets go of once it is closed ([`HeldStore`]).
enum Then {
    /// The daemon's hold of its path.
    LetGo { _hold: Hold },
    /// fxdax: what says a point kept open is closed, to what closes it ([`Point`]).
    SayClosed { _closing: futures::channel::oneshot::Sender<std::convert::Infallible> },
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
    /// The store at `path`, opened on a blocking thread ([`open_here`]), which `hold` holds.
    async fn open(path: &Path, hold: Hold) -> Result<Self, Errno> {
        let path = path.to_owned();
        let open = move || Ok(HeldStore::new(Arc::new(open_here(&path)?), Some(hold)));
        Ok(Self(Some(Arc::new(blocking(open).await?))))
    }

    /// `store`, which `hold` holds, if anything does: one open for connections, or opened here.
    fn of(store: Arc<ExtentStore>, hold: Option<Hold>) -> Self {
        Self(Some(Arc::new(HeldStore::new(store, hold))))
    }

    /// fxdax: `point`, a point kept open for forks ([`keep`]).
    fn kept(point: Arc<HeldStore>) -> Self {
        Self(Some(point))
    }

    fn store(&self) -> &Arc<HeldStore> {
        self.0.as_ref().expect("an open store")
    }
}

impl HeldStore {
    /// `store`, which `hold` holds, if anything does.
    fn new(store: Arc<ExtentStore>, hold: Option<Hold>) -> Self {
        Self { store, _then: hold.map(|_hold| Then::LetGo { _hold }) }
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

/// fxdax: how long a point the daemon opened to fork stays open with no fork of it (TASK-92.5):
/// a zygote's restores, which fork its snapshot's point, come together, and the point's opening,
/// attaching and closing, a second or more with object storage a region away, is paid once for
/// them, rather than by each in turn, the last of them waiting for all those before it.
const LINGER: std::time::Duration = std::time::Duration::from_secs(60);

/// fxdax: a point the daemon opened to fork, kept open for the forks after it ([`keep`]): its
/// store, which they fork at once, as it is; what says it is closed, to what closes it, once the
/// forks forking it are done; and when it was last forked. Kept in the daemon's [`table`], at the
/// key of the store it is, of which no connection has anything open: what holds the store's path
/// to work on it, but to fork it, closes the point first ([`hold`], [`hold_to_write`]).
pub(super) struct Point {
    store: Arc<HeldStore>,
    closed: futures::channel::oneshot::Receiver<std::convert::Infallible>,
    forked: std::time::Instant,
}

/// fxdax: the point kept open at `key`, if one is, forked now.
fn kept(key: &Path) -> Option<Arc<HeldStore>> {
    table().point(key, |point| {
        point.forked = std::time::Instant::now();
        point.store.clone()
    })
}

/// fxdax: when `point`, kept open at `key`, was last forked, if it is kept still.
fn forked(key: &Path, point: &Weak<HeldStore>) -> Option<std::time::Instant> {
    let kept = |kept: &mut Point| std::ptr::eq(Arc::as_ptr(&kept.store), point.as_ptr());
    table().point(key, |point| kept(point).then_some(point.forked)).flatten()
}

/// fxdax: keeps `store`, opened to fork at the key `hold` holds, open for the forks after it
/// (TASK-92.5), until none forks it for [`LINGER`] ([`let_go_when_idle`]), a fork of it fails
/// ([`let_go_failed`]), or the daemon holds its path to work on it otherwise, or a connection to
/// write it. Its caller found nothing open there: were something, the table would refuse it, and
/// the point, not kept, would close once its caller's fork is done.
fn keep(hold: &Hold, store: ExtentStore) -> Arc<HeldStore> {
    let (closing, closed) = futures::channel::oneshot::channel();
    // What says it is closed goes after the store.
    let store = Arc::new(HeldStore {
        store: Arc::new(store),
        _then: Some(Then::SayClosed { _closing: closing }),
    });
    let point = Point { store: store.clone(), closed, forked: std::time::Instant::now() };
    match hold.keep(point) {
        Ok(()) => {
            let idle = let_go_when_idle(hold.key().to_owned(), Arc::downgrade(&store));
            // On the runtime that lives as long as the daemon, whatever its caller's.
            drop(super::server::IO.spawn(idle));
        }
        Err(_) => log::error!(
            "extentd: {} has something open already: the point opened isn't kept",
            hold.key().display()
        ),
    }
    store
}

/// fxdax: closes `point`, if given, a point kept open its holder took, and returns once it is
/// closed: once the forks forking it are done. It blocks.
fn close_point_blocking(point: Option<Point>) {
    let Some(Point { store, closed, .. }) = point else { return };
    drop(store);
    let _ = futures::executor::block_on(closed);
}

/// fxdax: closes `point`, as [`close_point_blocking`] does, holding no thread while it waits, its
/// closing on a blocking thread.
async fn close_point(point: Option<Point>) {
    let Some(Point { store, closed, .. }) = point else { return };
    drop(tokio::task::spawn_blocking(move || drop(store)));
    let _ = closed.await;
}

/// fxdax: closes every point kept open for forks, as the daemon drains (TASK-141.4), each under the
/// daemon's hold of its path, once the forks forking it are done, as [`let_go_when_idle`] closes
/// one: those it takes no hold of, logged, are closed past the drain's bound, if at all.
pub(super) async fn close_points() {
    for key in table().points() {
        if let Err(errno) = hold(&key).await {
            log::warn!("extentd: closing the point at {}: {errno}", key.display());
        }
    }
}

/// fxdax: closes `point`, kept open at `key`, once none has forked it for [`LINGER`], under the
/// daemon's hold of its path, so that no fork opens it anew as it closes; unless something closed
/// it before, as a connection's opening it does, which this then leaves to it.
async fn let_go_when_idle(key: PathBuf, point: Weak<HeldStore>) {
    loop {
        let Some(last) = forked(&key, &point) else { return };
        match LINGER.checked_sub(last.elapsed()).filter(|left| !left.is_zero()) {
            Some(left) => tokio::time::sleep(left).await,
            None => {
                let Ok(hold) = hold_to_fork(&key).await else { return };
                if forked(&key, &point).is_some_and(|last| last.elapsed() >= LINGER) {
                    return close_point(hold.take_point()).await;
                }
            }
        }
    }
}

/// fxdax: closes `point`, kept open at `key`, if it is kept still, one of its forks having failed,
/// as all do once another placement takes its branch: the next fork opens it anew, as each did
/// before points were kept. Under the daemon's hold of its path, as [`let_go_when_idle`] closes
/// one; its caller holds the point no more.
async fn let_go_failed(key: &Path, point: Weak<HeldStore>) {
    if forked(key, &point).is_none() {
        return;
    }
    let Ok(hold) = hold_to_fork(key).await else { return };
    if forked(key, &point).is_some() {
        close_point(hold.take_point()).await;
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
pub(super) async fn uploaded<S>(store: &Arc<S>) -> Result<(), Errno>
where
    S: Borrow<ExtentStore> + Send + Sync + 'static,
{
    let flushed = store.clone();
    let epoch = blocking(move || (*flushed).borrow().durable_epoch()).await?;
    (**store).borrow().uploaded(epoch).await
}

/// The store a connection has open at `path`, if one has, whose VM process isn't gone: one going
/// is let go once its last commits are durable, which [`hold`] waits for. fxdax: or, with no
/// writer's connection, its followers' (backlog doc-8).
fn open_by_connection(path: &Path) -> Option<Arc<ExtentStore>> {
    let key = store_key(path);
    match table().writer(&key) {
        Some(writer) if writer.going() => None,
        Some(writer) => writer.store().cloned(),
        None => table().instance(&key).map(|(store, _)| store),
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
    let own = arena.clone();
    let store = attached(path, |keeping| {
        let opening = Opening { arena: own, profiled: true, keeping };
        match capacity {
            Some(capacity) if !path.exists() => {
                ExtentStore::create_with(path, capacity, crate::host_store::new_branch(), opening)
            }
            _ => ExtentStore::open_with(path, opening),
        }
    })?;
    // fxdax: a fork attached to it opens when first read through to.
    store.open_bases(Arc::new(move |base| self::base(base, arena.as_ref())))?;
    Ok(store)
}

/// fxdax: the fork at `path`, attached to stores (backlog doc-10), for a store whose pages are of
/// `arena` to read through to: the one instance open of it, for all that attach it, in the default
/// arena, if public, or `arena`: `EBUSY` in another's, whose pages the connection's VM process
/// doesn't hold.
fn base(path: &Path, arena: Option<&Arc<Arena>>) -> Result<Arc<ExtentStore>, Errno> {
    let open = |found: Option<(Arc<ExtentStore>, bool)>| match found {
        Some((_, true)) => Err(Errno::EBUSY),
        Some((store, false)) if store.is_public() => Ok(Some(store)),
        Some((store, false)) => {
            let own = arena.map_or(crate::pages::ArenaId::DEFAULT, |arena| arena.id());
            if store.arena() == own { Ok(Some(store)) } else { Err(Errno::EBUSY) }
        }
        None => Ok(None),
    };
    let key = store_key(path);
    if let Some(store) = open(table().instance(&key))? {
        return Ok(store);
    }
    let hold = hold_blocking(&key)?;
    if let Some(store) = open(hold.instance())? {
        return Ok(store);
    }
    if !path.exists() {
        return Err(Errno::ENOENT);
    }
    let opened =
        attached(path, |keeping| ExtentStore::open_base_with(path, arena.cloned(), keeping));
    let store = Arc::new(opened?);
    hold.open(&store)?;
    Ok(store)
}

/// fxdax: attaches the fork at `base` to the store at `path` (`AttachBase`), a VM's or stopped:
/// `EACCES` unless `allowed` says its tenant may read a store that isn't public.
pub(super) async fn attach(
    path: PathBuf,
    base: PathBuf,
    allowed: impl FnOnce(&Path, &Path) -> bool + Send + 'static,
) -> Result<(), Errno> {
    let base = base.canonicalize().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => Errno::ENOENT,
        _ => Errno::EIO,
    })?;
    let (public, ancestry) = match table().instance(&store_key(&base)) {
        Some((store, _)) => (store.is_public(), store.ancestry()),
        None => {
            let here = Here::open(&base, hold(&base).await?).await?;
            let what = (here.store().store.is_public(), here.store().store.ancestry());
            here.close().await;
            what
        }
    };
    if !public && !allowed(&store_key(&path), &base) {
        return Err(Errno::EACCES);
    }
    if let Some(store) = open_by_connection(&path) {
        return blocking(move || store.attach_base(&base, &ancestry)).await;
    }
    let held = hold(&path).await?;
    blocking(move || {
        let _held = held;
        if !path.exists() {
            return Err(Errno::ENOENT);
        }
        crate::store_dir::attach(&path, &base, &ancestry)
    })
    .await
}

/// The store at `path`, as `open` opens it with the daemon's uploader's options ([`keeping`]),
/// keeping its branch in the bucket its description names: the daemon's, for the stores it made,
/// or another's, as an image's file bucket, for one it serves (TASK-165.6). Its opening takes the
/// branch's lease (TASK-165.9).
fn attached(
    path: &Path,
    open: impl FnOnce(extent_store::upload::Options) -> Result<ExtentStore, crate::Error>,
) -> Result<ExtentStore, Errno> {
    // The daemon's own first, which the stores naming it share (`host_store::bucket`).
    remote_bucket()?;
    let keeping = keeping()?;
    open(keeping).map_err(|err| errno_of(format_args!("opening {}", path.display()), err))
}

/// How the daemon's stores' uploaders keep their branches in their buckets: with this host's
/// placement's token, for a copy never placed; the host's cache of the bucket's objects; and the
/// upload interval ([`upload_interval`]).
fn keeping() -> Result<extent_store::upload::Options, Errno> {
    let token = crate::remote::host_token().map_err(|err| {
        log::error!("extentd: {err}");
        Errno::EINVAL
    })?;
    let (cache, interval) = (remote_cache()?, upload_interval());
    Ok(extent_store::upload::Options { cache, token, interval, ..Default::default() })
}

/// How long the stores' durable commits wait at most before they upload: the uploader's default;
/// in a test that asked for it, until something waits for an upload ([`uploads_when_asked`]).
fn upload_interval() -> std::time::Duration {
    #[cfg(test)]
    if UPLOADS_WHEN_ASKED.load(std::sync::atomic::Ordering::Relaxed) {
        return std::time::Duration::MAX;
    }
    extent_store::upload::Options::default().interval
}

/// The store at `path` as the daemon opens it to work on it, not a VM: its pages of the default
/// arena, which reads no boot profile into the cache ([`ExtentStore::open_without_profile`]).
fn open_here(path: &Path) -> Result<ExtentStore, Errno> {
    attached(path, |keeping| {
        ExtentStore::open_with(path, Opening { arena: None, profiled: false, keeping })
    })
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
    let held = hold(&path).await?;
    blocking(move || {
        // Let go of once the work is done, though the call may give up on it before.
        let _held = held;
        // fxdax: what a making its host died in left, unmade, is no store: made anew (TASK-196).
        removed_if_unmade(&path)?;
        if path.exists() {
            return Err(Errno::EEXIST);
        }
        let branch =
            branch.filter(|branch| *branch != 0).unwrap_or_else(crate::host_store::new_branch);
        let created = attached(&path, |keeping| {
            let opening = Opening { arena: None, profiled: false, keeping };
            ExtentStore::create_with(&path, capacity, branch, opening)
        });
        drop(created?);
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
    let held = hold(&path).await?;
    blocking(move || {
        let _held = held;
        // fxdax: what an import its host died in left, unmade, is no store: made anew
        // (TASK-196).
        removed_if_unmade(&path)?;
        if path.exists() {
            return Err(Errno::EEXIST);
        }
        // As the daemon's stores keep their branches: with the host's cache, which what the bucket
        // acknowledged of the image goes into and the import's check reads (TASK-237).
        let imported = (image.as_path(), reference.as_deref());
        let made = crate::import::store_keeping(&path, capacity, imported, keeping()?);
        futures::executor::block_on(made)
            .map_err(|err| errno_of(format_args!("importing {}", image.display()), err))?;
        Ok(open_here(&path)?.branch())
    })
    .await
}

/// The store at `path`, or, if it doesn't exist, one of `branch`, of `capacity` bytes, taken from
/// the daemon's bucket: what the branch holds there.
/// fxdax: a store of `branch` there whose write-ahead log another writer's commits moved its branch
/// past is opened as the branch is, the log dropped (`ExtentStore::take_with`, TASK-165.9).
fn take_store(path: &Path, branch: u64, capacity: u64) -> Result<ExtentStore, Errno> {
    if !path.exists() && branch == 0 {
        return Err(Errno::ENOENT);
    }
    let branch = match branch {
        0 => extent_store::host_branch(path).map_err(|_| Errno::ENOENT)?,
        branch => branch,
    };
    attached(path, |keeping| {
        let opening = Opening { arena: None, profiled: false, keeping };
        ExtentStore::take_with(path, (capacity, branch, None), opening)
    })
}

/// Removes what a store's making, or a fork's, left at `path`, unmade, its host crashing
/// (`host_store::unmade`): no store, which a fork or a restore to `path` makes anew. Its holder
/// holds `path`.
pub(super) fn removed_if_unmade(path: &Path) -> Result<(), Errno> {
    if !crate::host_store::unmade(path) {
        return Ok(());
    }
    std::fs::remove_dir_all(path).map_err(|err| {
        log::warn!("extentd: removing {}, a store unmade: {err}", path.display());
        Errno::EIO
    })
}

/// Forks the store at `from`, the point a snapshot of a VM was taken at, to a new store at `path`,
/// a branch of its own, which the VM restored from the snapshot opens (TASK-39.1). fxdax: restores
/// of one snapshot, a zygote's, fork its point at once, as it is, kept open for them ([`point`],
/// TASK-92.5); a connection having it open makes it busy (`EBUSY`), unless its VM process is gone.
/// A point that doesn't exist is taken from the daemon's bucket, a store of `branch` of
/// `capacity` bytes, as on another host than the snapshot's: read, as any number of hosts do at
/// once, and written by none (doc-7, Points). Its waits hold no thread; its fork blocks. A store
/// that is no point, kept open since an earlier restore whose fork fails, as once another host, or
/// this one's fxdaxctl, took its branch, which fences it, is let go of, and opened anew for a fork
/// once more, which takes the branch back, as each restore's did before stores were kept open: a
/// restore fails only if the store it opened fails it.
pub(super) async fn fork_from(
    from: &Path,
    branch: u64,
    capacity: u64,
    path: &Path,
) -> Result<(), Errno> {
    match fork_point(from, branch, capacity, path).await {
        (Err(errno), Opened::Before) if errno != Errno::EEXIST => {
            // What a crash left at `path` goes first, as before any restore's.
            removed_if_unmade(path)?;
            fork_point(from, branch, capacity, path).await.0
        }
        (forked, _) => forked,
    }
}

/// fxdax: `Ok` if `store`, at a restore's `path` already, holds what the point at `from` does
/// ([`ExtentStore::holds_what`]), the point taken from the bucket, of `branch` and `capacity`
/// bytes, as [`fork_from`] takes it: the store the snapshot was taken of, written nothing since,
/// or a fork of the point an earlier restore made, its VM having written nothing (TASK-194).
/// `EEXIST` otherwise, logged: its guest, of the snapshot's memory, would read extents freed since
/// and commit over what was written since. Nothing of the store is changed.
pub(super) async fn at_point(
    from: &Path,
    branch: u64,
    capacity: u64,
    path: &Path,
    store: &ExtentStore,
) -> Result<(), Errno> {
    let (point, _) = point(from, branch, capacity).await?;
    if store.holds_what(&point.store)? {
        return Ok(());
    }
    log::warn!(
        "extentd: refusing a restore from {} into {}: written since its point, it holds another \
         state than the snapshot's",
        from.display(),
        path.display()
    );
    Err(Errno::EEXIST)
}

/// fxdax: forks the point at `from` to `path`, as [`fork_from`] does, once: and says when the
/// point was opened. A point whose fork fails is let go of ([`let_go_failed`]).
async fn fork_point(
    from: &Path,
    branch: u64,
    capacity: u64,
    path: &Path,
) -> (Result<(), Errno>, Opened) {
    let (point, opened) = match point(from, branch, capacity).await {
        Ok(point) => point,
        Err(errno) => return (Err(errno), Opened::Now),
    };
    let forked = point.store.fork(path, crate::host_store::new_branch()).map_err(|err| {
        errno_of(format_args!("forking {} to {}", from.display(), path.display()), err)
    });
    let failed = Arc::downgrade(&point);
    drop(point);
    // fxdax: but for its being there already, which is no fault of the point's.
    if forked.as_ref().is_err_and(|errno| *errno != Errno::EEXIST) {
        let_go_failed(&store_key(from), failed).await;
    }
    (forked, opened)
}

/// fxdax: when a restore's point was opened ([`point`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opened {
    /// For it, or for the restores beside it.
    Now,
    /// For a restore before it, kept open since ([`keep`]).
    Before,
}

/// fxdax: the point at `from` for a restore to fork, of `branch` and `capacity` bytes if the
/// daemon takes it from its bucket ([`take_store`]), which forks fork at once (TASK-92.5): the
/// point kept open, if one is ([`kept`]); or, under the daemon's hold of its path, which no
/// connection opens it meanwhile, the store followers have open, forked as it is, held until the
/// fork is done; or the point opened, kept open from then on ([`keep`]). And when it was opened.
/// A restore waiting for another's opening of it holds no thread.
async fn point(from: &Path, branch: u64, capacity: u64) -> Result<(Arc<HeldStore>, Opened), Errno> {
    if let Some(point) = kept(&store_key(from)) {
        return Ok((point, Opened::Before));
    }
    let hold = hold_to_fork(from).await?;
    // Opened by a restore that held it before, beside this one.
    if let Some(point) = kept(hold.key()) {
        return Ok((point, Opened::Now));
    }
    if let Some((store, _)) = hold.instance() {
        return Ok((Arc::new(HeldStore::new(store, Some(hold))), Opened::Now));
    }
    Ok((keep(&hold, take_store(from, branch, capacity)?), Opened::Now))
}

/// Forks the store at `from` to a new branch, `branch`, at `to`: live, if a connection has it
/// open, once nothing is open (`EBUSY` otherwise); fxdax: or its followers, with no writer; or
/// the point kept open of it, as restores fork it (TASK-92.5), opened here, if none is, which no
/// connection opens meanwhile, and kept open for the forks after it. Its commits' uploads, which
/// the fork's branch follows, are waited for first, holding no thread; the fork, as of then, is
/// made on a blocking thread. fxdax: the store at `to` is held throughout, so that a connection
/// opens the child only once the fork is made, in the bucket too: before, it found no record of
/// the fork there, and took a branch of its own, which failed every writer of the parent after,
/// finding the child's record another's (TASK-79.95). Of a `branch` of 0, the fork's is a new one,
/// and returned: a point kept open whose fork fails then, as once another host took its branch, is
/// opened anew for a fork once more, as a restore's ([`fork_from`], TASK-79.103), to another new
/// branch: the failed fork's ID stays taken in the bucket, so a `branch` the caller named gets the
/// one try. The fork is a point if `point` ([`ExtentStore::point`], TASK-158), live or not, once
/// nothing is open.
pub(super) async fn fork(
    from: PathBuf,
    to: PathBuf,
    branch: u64,
    point: bool,
) -> Result<u64, Errno> {
    let _child = hold_to_fork(&to).await?;
    let key = store_key(&from);
    let named = branch != 0;
    let new = || if named { branch } else { crate::host_store::new_branch() };
    let was_kept = kept(&key).is_some();
    let first = new();
    match fork_held(&from, &to, first, point, &key).await {
        Err(errno) if !named && was_kept && errno != Errno::EEXIST => {
            let second = new();
            fork_held(&from, &to, second, point, &key).await.map(|()| second)
        }
        forked => forked.map(|()| first),
    }
}

/// fxdax: forks the store at `from`, whose key is `key`, to `to`, which the caller holds, as
/// [`fork`] does, once, making a point if `point`. A point whose fork fails is let go of
/// ([`let_go_failed`]).
async fn fork_held(
    from: &Path,
    to: &Path,
    branch: u64,
    point: bool,
    key: &Path,
) -> Result<(), Errno> {
    let live = |connection: &Connection| {
        Some(Forked { store: Here::of(connection.store()?.clone(), None), live: true })
    };
    let open = |hold: Option<Hold>| async {
        if let Some((store, _)) = table().instance(key) {
            return Ok(Forked { store: Here::of(store, hold), live: true });
        }
        // Opened by a restore that held it before.
        if let Some(point) = kept(key) {
            return Ok(Forked { store: Here::kept(point), live: false });
        }
        let opening = from.to_owned();
        let store = blocking(move || open_here(&opening)).await?;
        let store = match hold {
            // Let go of once kept.
            Some(hold) => Here::kept(keep(&hold, store)),
            None => Here::of(Arc::new(store), None),
        };
        Ok(Forked { store, live: false })
    };
    let forking = match kept(key) {
        Some(point) => Forked { store: Here::kept(point), live: false },
        None => store_requests::forking(table(), key, live, CLOSING_WAIT, open).await?,
    };
    let (store, live) = (forking.store.store().clone(), forking.live);
    let held = Arc::downgrade(&store);
    let to = to.to_owned();
    let forked = async {
        uploaded(&store).await?;
        fail::fail_point!("extentd::fork_uploaded", |_| Err(Errno::EIO));
        blocking(move || {
            let store = &store.store;
            let forked = match (point, live) {
                (true, _) => store.point(&to, branch),
                (false, true) => store.fork_quiescent(&to, branch),
                (false, false) => store.fork(&to, branch),
            };
            forked.map_err(|err| errno_of(format_args!("forking to {}", to.display()), err))
        })
        .await
    }
    .await;
    forking.close().await;
    // fxdax: but for its being there already, which is no fault of the point's.
    if forked.as_ref().is_err_and(|errno| *errno != Errno::EEXIST) {
        let_go_failed(key, held).await;
    }
    forked
}

/// Waits until every commit of the store at `path` is in the daemon's bucket, holding no thread:
/// through the connection's store, if one has it open; or opened here, which uploads what it
/// holds, if none has.
pub(super) async fn sync(path: PathBuf) -> Result<(), Errno> {
    if let Some(store) = open_by_connection(&path) {
        return uploaded(&store).await;
    }
    let here = Here::open(&path, hold(&path).await?).await?;
    let synced = uploaded(here.store()).await;
    here.close().await;
    synced
}

/// Deletes the store at `path`, or the fork whose handle it is, and its branch in its bucket
/// ([`ExtentStore::delete`], [`bucket_of`]): `EBUSY` while a connection has it open.
pub(super) async fn delete(path: PathBuf) -> Result<(), Errno> {
    let bucket = bucket_of(&path)?;
    delete_alone(path, Some(bucket)).await
}

/// The bucket the store at `path`, or the fork whose handle it is, keeps its branch in, as its
/// description names it (TASK-165.6): or, if its description is not there, as for what a
/// deletion that stopped partway left, which deletes nothing in a bucket, the daemon's.
fn bucket_of(path: &Path) -> Result<Arc<extent_remote::store::Store>, Errno> {
    match crate::host_store::bucket_at(path) {
        Ok(Some(bucket)) => Ok(bucket),
        Ok(None) => remote_bucket(),
        Err(err) => {
            log::warn!("extentd: the bucket of {}: {err:#}", path.display());
            Err(Errno::EIO)
        }
    }
}

/// fxdax: deletes this host's copy of the store at `path`, or of the fork whose handle it is
/// (TASK-154): as [`delete`] does, but for its branch in the daemon's bucket, which stays, for what
/// else restores it: another host's copy, or this one's taken again. What its commits hold that
/// the bucket doesn't goes with it. A point the VM manager took for a tenant goes so: the tenant
/// restores it no more, and the snapshot lives on.
pub(super) async fn drop_copy(path: PathBuf) -> Result<(), Errno> {
    delete_alone(path, None).await
}

/// Deletes the store at `path`, and its branch in `bucket`, if given: `EBUSY` while a connection
/// has it open.
async fn delete_alone(
    path: PathBuf,
    bucket: Option<Arc<extent_remote::store::Store>>,
) -> Result<(), Errno> {
    let held = hold_alone(&path).await?;
    blocking(move || {
        let _held = held;
        ExtentStore::delete(&path, bucket.as_deref())
            .map_err(|err| errno_of(format_args!("deleting {}", path.display()), err))
    })
    .await
}

/// Sets the capacity of the store at `path` to `capacity` bytes: through the connection that has
/// it open, if one does, which grows it, or shrinks it if its guest agrees, holding no thread while
/// it waits for the guest's answer; or, if none does, here, growing it (`EBUSY` to shrink it, its
/// guest not here to agree), no connection opening it meanwhile. fxdax: as the store's access says
/// ([`crate::StoreAccess::may_resize`]): a point's capacity, as all else of it, changes nowhere
/// (`EROFS`).
pub(super) async fn resize(path: PathBuf, capacity: u64) -> Result<(), Errno> {
    let key = store_key(&path);
    let open = |hold: Option<Hold>| {
        let key = key.clone();
        blocking(move || {
            // fxdax: open for followers, it grows as it is; the point kept open of it for forks,
            // it opens anew, closed first (TASK-92.5): attached, as any store the daemon opens,
            // which says whether it is a point (TASK-160.10).
            let store = match table().instance(&key) {
                Some((store, _)) => store,
                None => {
                    close_point_blocking(hold.as_ref().and_then(Hold::take_point));
                    if !key.exists() {
                        return Err(Errno::ENOENT);
                    }
                    Arc::new(open_here(&key)?)
                }
            };
            Ok(Here::of(store, hold))
        })
    };
    store_requests::resize(table(), &key, capacity, open).await
}

/// Places the store at `path`, or the fork whose handle it is, at the placement whose fencing
/// token is `token` ([`ExtentStore::place`]): `EBUSY` while a connection has it open, whose
/// writer took its branch with the token it had.
pub(super) async fn place(path: PathBuf, token: u64) -> Result<(), Errno> {
    let held = hold_alone(&path).await?;
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
pub(super) async fn make_public(path: PathBuf, generation: bool) -> Result<(), Errno> {
    let held = hold_alone(&path).await?;
    blocking(move || {
        let _held = held;
        let store = open_here(&path)?;
        if generation { store.make_generation_public() } else { store.make_public() }
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
    pub upload: Option<extent_store::upload::UploadStatus>,
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
    let stores = table().instances();
    stores.into_iter().map(|(path, store)| Ok((path, state_of(&store, true)?))).collect()
}

/// What the store at `path` is, and holds: as the connection that has it open has it, if one does.
pub(super) async fn stat(path: PathBuf) -> Result<StoreState, Errno> {
    if let Some(store) = open_by_connection(&path) {
        return state_of(&store, true);
    }
    let held = hold(&path).await?;
    blocking(move || {
        let _held = held;
        if !path.exists() {
            return Err(Errno::ENOENT);
        }
        let store = open_here(&path)?;
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
    table().writers()
}

/// fxdax: the stores open for connections, writers' and followers'.
fn open_instances() -> Vec<Arc<ExtentStore>> {
    table().instances().into_iter().map(|(_, store)| store).collect()
}

/// The branches of the stores open for connections whose commits upload now, and what their
/// uploaders have left to do (`metrics`, TASK-91.3).
pub(super) fn upload_statuses() -> Vec<(u64, extent_store::upload::UploadStatus)> {
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
    let bucket = crate::host_store::bucket(&host.bucket).map_err(|err| {
        log::error!("extentd: FXDAX_EXTENTD_REMOTE: {err:#}");
        Errno::EIO
    })?;
    Ok(BUCKET
        .get_or_init(|| {
            watch_stats(Arc::downgrade(&bucket));
            bucket
        })
        .clone())
}

/// Whether the stores opened upload only when something waits for an upload ([`uploads_when_asked`]).
#[cfg(test)]
static UPLOADS_WHEN_ASKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Has the stores opened from now on upload only when something waits for an upload (a fork, a
/// sync, a commit stalled at the lag's bound, the store closing), never as their interval runs
/// out, as `remote::fork_faults`' stores do. For tests that set a failpoint to fail once for one
/// store's work: failpoints are the process's, and another store's uploader, woken by its
/// interval, would pass it first (TASK-157).
#[cfg(test)]
pub(super) fn uploads_when_asked() {
    UPLOADS_WHEN_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Makes `bucket` the daemon's, before any store opens: for tests that put themselves between the
/// daemon's stores and their objects. The stores naming `FXDAX_EXTENTD_REMOTE`'s bucket open it
/// (`host_store::use_bucket`).
#[cfg(test)]
pub(super) fn use_bucket(bucket: extent_remote::store::Store) {
    let bucket = Arc::new(bucket);
    let host = crate::remote::Host::from_env().expect("the daemon's host");
    crate::host_store::use_bucket(&host.bucket, bucket.clone());
    assert!(BUCKET.set(bucket.clone()).is_ok(), "the daemon has its bucket already");
    watch_stats(Arc::downgrade(&bucket));
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
    // fxdax: and how long its stores' commits waited to be made durable (TASK-165.9): each
    // bucket of `extent_store::Lags` a count of its own, within its bound, the last beyond.
    let lags = extent_store::durability_lags();
    let quantile = |fraction| lags.quantile_ms(fraction).unwrap_or(0);
    let last = extent_store::LAG_BUCKETS - 1;
    let bucket = |(bucket, count): (usize, &u64)| match bucket == last {
        false => format!("durable_lag_le_{}ms={count}", 1u64 << bucket),
        true => format!("durable_lag_gt_{}ms={count}", 1u64 << (bucket - 1)),
    };
    let buckets: Vec<String> = lags.buckets.iter().enumerate().map(bucket).collect();
    // And where it went: how long its flushes and framings took, phase by phase.
    let timings = extent_store::timings();
    let timed = [
        timed_fields("flush", &timings.flushes, &extent_store::FLUSH_PHASES),
        timed_fields("framing", &timings.framings, &extent_store::FRAMING_PHASES),
        vec![
            format!("own_sync_max_us={}", timings.own_sync_max_us),
            format!("raw_sync_max_us={}", timings.raw_sync_max_us),
        ],
    ]
    .concat();
    format!(
        "daemon: {} durable_lag_count={} durable_lag_max_us={} durable_lag_p50_ms={} \
         durable_lag_p99_ms={} durable_lag_within_50ms={} {} {}",
        named.join(" "),
        lags.count,
        lags.max_us,
        quantile(0.5),
        quantile(0.99),
        lags.within_turn,
        buckets.join(" "),
        timed.join(" ")
    )
}

/// fxdax: `timed`, of `kind`, as `NAME=COUNT` fields: so many, so many slow, the slowest's
/// phases, by `names`, and each phase's longest (TASK-165.9).
fn timed_fields<const N: usize>(
    kind: &str,
    timed: &extent_store::Timed<N>,
    names: &[&str; N],
) -> Vec<String> {
    let mut fields =
        vec![format!("{kind}_count={}", timed.count), format!("{kind}_slow={}", timed.slow)];
    for (which, phases) in [("slowest", &timed.slowest), ("max", &timed.max)] {
        let named = names.iter().zip(phases.us);
        fields.extend(named.map(|(phase, us)| format!("{kind}_{which}_{phase}_us={us}")));
    }
    fields
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
/// none), where what the stores opened read from their bucket is kept. Opened once, by the first
/// store to open, those opening meanwhile waiting for it: a cache holds its directory locked, and
/// two opened at once, as a daemon's VMs booting at once open their stores, failed the second
/// (TASK-67). A failure is the caller's, and the next tries again.
fn remote_cache() -> Result<Option<Arc<Cache>>, Errno> {
    static CACHE: std::sync::Mutex<Option<Option<Arc<Cache>>>> = std::sync::Mutex::new(None);
    let mut opened = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cache) = &*opened {
        return Ok(cache.clone());
    }
    let cache = match std::env::var_os("FXDAX_EXTENTD_CACHE") {
        Some(dir) if !dir.is_empty() => match Cache::open(dir.as_ref(), u64::MAX, CACHE_FLOOR) {
            Ok(cache) => Some(Arc::new(cache)),
            Err(err) => {
                log::error!("extentd: FXDAX_EXTENTD_CACHE: {err}");
                return Err(Errno::EIO);
            }
        },
        _ => None,
    };
    Ok(opened.insert(cache).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fxdax: the daemon's first stores, opened at once, share the one cache, which holds its
    /// directory locked: each found none yet and opened its own, and the second's lock failed, its
    /// store with EIO, as a daemon started again served its VMs booted again all at once
    /// (fxfs-guest-soak, TASK-67: a VM of five exited 1 at boot).
    #[test]
    fn stores_opened_at_once_share_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: nextest runs each test in a process of its own, whose other threads don't read
        // the environment yet.
        unsafe { std::env::set_var("FXDAX_EXTENTD_CACHE", dir.path()) };
        let at_once = std::sync::Barrier::new(16);
        let caches: Vec<_> = std::thread::scope(|scope| {
            let opening: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        at_once.wait();
                        remote_cache()
                    })
                })
                .collect();
            opening.into_iter().map(|opening| opening.join().unwrap()).collect()
        });
        let first = caches[0].clone().expect("a cache").expect("FXDAX_EXTENTD_CACHE's");
        for cache in caches {
            let cache = cache.expect("a cache").expect("FXDAX_EXTENTD_CACHE's");
            assert!(Arc::ptr_eq(&cache, &first), "two caches of one directory");
        }
    }
}
