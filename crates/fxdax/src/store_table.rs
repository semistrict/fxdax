// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The extent daemon's stores (`crate::extentd`), by the keys that name them
//! (`extentd::stores::store_key`): one table, whose entry for a key says all there is of its store
//! (TASK-160.8). Who holds it: a connection writing it, or the daemon working on it. And what is
//! open of it for others: the store its writer's connection, or its followers', have open, or a
//! point the daemon keeps open for forks. Five owners said so before, kept in step by hand: the
//! holders, the stores open for connections and whether retired, the points kept, a connection's
//! own fields, and a store's following of its writer's; which TASK-79.34, 150.5, 54, 79.84,
//! 79.95, 92.5, 79.103 and 79.36 came of.
//!
//! A store has one holder at a time, whose hold ([`Hold`]) is a value that lets the store go as it
//! is dropped: a connection's, from its `OPEN` until it is gone and its store closed; the daemon's,
//! while it forks, syncs or resizes a store no connection writes. A connection whose VM process is
//! gone, still holding its store, lets it go to the next that asks, as a VM restored from a
//! snapshot at once opens its VM's store (TASK-39.1). What is open of a store outlives its holder:
//! a store followers follow, read-only (backlog doc-8), as long as one has it, which a writer
//! holding it then takes over, unless its writer retired it, gone with writes uncommitted; a point
//! kept open for forks, until a writer holds the store, which closes it first (TASK-92.5). A
//! connection holds no point: its entry ([`Entry::Writer`]) has no room for one.
//!
//! Generic over its connections, stores and points, so that its tests run it with models of them:
//! under Shuttle (awslabs/shuttle), with the `shuttle` feature, whose scheduler chooses every
//! interleaving of the threads writing, following, retiring and keeping points of a store as its
//! writer's VM process goes (TASK-44.6).

use crate::Errno;
use event_listener::{Event, EventListener};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "shuttle")]
use shuttle::sync::Mutex;
#[cfg(not(feature = "shuttle"))]
use std::sync::Mutex;

/// The rules the table relies on, each of which its Shuttle tests show to be needed by turning it
/// off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A store held by a holder that isn't going holds no other.
    pub one_holder: bool,
    /// Letting a store go wakes those waiting to hold it.
    pub release_wakes: bool,
    /// What is open of a store, the store followers have open or a point kept, stays as its holder
    /// lets the store go.
    pub open_outlives_holders: bool,
    /// A store retired stays so while followers have it.
    pub retirement: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        one_holder: true,
        release_wakes: true,
        open_outlives_holders: true,
        retirement: true,
    };
}

#[cfg(not(feature = "shuttle"))]
fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

#[cfg(feature = "shuttle")]
fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules, for the Shuttle tests of what uses the table.
#[cfg(all(test, feature = "shuttle"))]
pub(crate) fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}

/// `future`'s output, unless `wait` passes first (`None`). Under Shuttle, where time doesn't
/// pass, it waits for ever: what never wakes it is a deadlock, which Shuttle reports.
pub(crate) async fn within<F: std::future::Future>(wait: Duration, future: F) -> Option<F::Output> {
    #[cfg(feature = "shuttle")]
    {
        let _ = wait;
        Some(future.await)
    }
    #[cfg(not(feature = "shuttle"))]
    {
        tokio::time::timeout(wait, future).await.ok()
    }
}

/// Waits for `released`, on this thread, until `deadline` (for ever, if none): whether it came.
/// Under Shuttle, where time doesn't pass, it waits for ever, as [`within`] does, the scheduler's
/// thread blocked on Shuttle's terms.
fn wait_blocking(released: EventListener, deadline: Option<Instant>) -> bool {
    #[cfg(feature = "shuttle")]
    {
        let _ = deadline;
        shuttle::future::block_on(released);
        true
    }
    #[cfg(not(feature = "shuttle"))]
    {
        use event_listener::Listener as _;
        match deadline {
            None => {
                released.wait();
                true
            }
            Some(deadline) => released.wait_deadline(deadline).is_some(),
        }
    }
}

/// A connection writing a store.
pub(crate) trait Writer: Send + Sync {
    /// Whether it is going: its VM process gone, it lets its store go to the next that asks.
    fn going(&self) -> bool;
}

/// The stores, by their keys: an entry each, while anything holds a store or has it open.
pub(crate) struct StoreTable<C, S, P> {
    inner: Mutex<Inner<C, S, P>>,
}

struct Inner<C, S, P> {
    entries: HashMap<PathBuf, Entry<C, S, P>>,
    /// What says a store is let go, to those waiting to hold it, on threads or not, by its key:
    /// woken as its holder lets it go, and none of another's, as every store's were, at each
    /// store's letting go (TASK-92.5). From the first that waits until then.
    waiting: HashMap<PathBuf, Event>,
}

/// A key's entry: who holds its store, and what is open of it meanwhile.
enum Entry<C, S, P> {
    /// A connection writes it, from its `OPEN` on: its store, once open, which followers may follow
    /// too.
    Writer { connection: Weak<C>, store: Instance<S> },
    /// The daemon holds it, to work on it, over what is open of it, which stays.
    Daemon(Open<S, P>),
    /// None holds it: what is open of it, never [`Open::Nothing`].
    Unheld(Open<S, P>),
}

/// What is open of a store, for others than its holder.
enum Open<S, P> {
    Nothing,
    /// The store connections have open, its followers', its writer gone or never there.
    Followed(Instance<S>),
    /// A point kept open for forks (TASK-92.5).
    Point(P),
}

/// The store connections have open, a writer's or followers', while one of them has it: retired
/// once its writer went with writes uncommitted, which neither a writer nor a follower opens then.
struct Instance<S> {
    store: Weak<S>,
    retired: bool,
}

/// Who asks to hold a store.
enum Claim<C> {
    /// The daemon, to work on it.
    Daemon,
    /// The daemon, unless a connection not going writes it, which the daemon then works through.
    DaemonOrWriter,
    /// A connection, to write it.
    Writer(Weak<C>),
}

/// What a claim got: the store held, with the point kept open of it that a writer's claim took
/// out, to close, and, past [`Rules::one_holder`], the connection holding it, which it passed:
/// dropped once the table's lock is, as a [`Refused`]'s; or the connection writing it.
enum Claimed<C, P> {
    Held { point: Option<P>, passed: Option<Arc<C>> },
    Writer(Arc<C>),
}

/// Why a claim failed, with the connection holding the store, if one does: dropped once the
/// table's lock is, as a connection, dropped last, lets its store go, which takes the lock.
enum Refused<C> {
    /// A connection not going writes it: `EBUSY`.
    Busy(Arc<C>),
    /// A connection going holds it, or the daemon, which lets it go soon.
    Wait(Option<Arc<C>>),
}

/// A store held, by a connection writing it or by the daemon working on it, which nothing else
/// holds until it is dropped: it lets the store go then, and those waiting to hold it may. What is
/// open of the store stays ([`Rules::open_outlives_holders`]): the store its connection had open,
/// if followers have it still.
#[must_use = "a store held is let go as its hold is dropped"]
pub(crate) struct Hold<'t, C, S, P> {
    table: &'t StoreTable<C, S, P>,
    key: PathBuf,
}

/// A store a connection writes, or held by the daemon ([`StoreTable::writer_or_hold`]).
pub(crate) enum Holding<'t, C, S, P> {
    Writer(Arc<C>),
    Held(Hold<'t, C, S, P>),
}

impl<S> Instance<S> {
    /// No store yet: a writer's, as it opens it.
    fn none() -> Self {
        Self { store: Weak::new(), retired: false }
    }

    /// The store, if one has it still, and whether retired.
    fn get(&self) -> Option<(Arc<S>, bool)> {
        Some((self.store.upgrade()?, self.retired))
    }
}

impl<S, P> Open<S, P> {
    /// As it is, but a store none has open any more, which is nothing.
    fn live(self) -> Self {
        match self {
            Open::Followed(instance) if instance.store.strong_count() == 0 => Open::Nothing,
            open => open,
        }
    }

    fn is_nothing(&self) -> bool {
        matches!(self, Open::Nothing)
    }

    /// Whether nothing is open: nothing, or a store none has open any more.
    fn is_closed(&self) -> bool {
        match self {
            Open::Nothing => true,
            Open::Followed(instance) => instance.store.strong_count() == 0,
            Open::Point(_) => false,
        }
    }
}

impl<C: Writer, S, P> Entry<C, S, P> {
    /// What a claim of the store meets, if anything holds it: a connection not going, or a
    /// connection going, or the daemon, which let it go soon.
    fn holder(&self) -> Option<Refused<C>> {
        match self {
            Entry::Writer { connection, .. } => Some(match connection.upgrade() {
                Some(writer) if !writer.going() => Refused::Busy(writer),
                going => Refused::Wait(going),
            }),
            Entry::Daemon(_) => Some(Refused::Wait(None)),
            Entry::Unheld(_) => None,
        }
    }
}

impl<C, S, P> Entry<C, S, P> {
    /// What is open of the store, its holder gone: the store a writer had open, followers' now.
    fn into_open(self) -> Open<S, P> {
        match self {
            Entry::Writer { store, .. } => Open::Followed(store).live(),
            Entry::Daemon(open) | Entry::Unheld(open) => open.live(),
        }
    }

    /// The store connections have open, its writer's or followers', if one is.
    fn instance(&mut self) -> Option<&mut Instance<S>> {
        match self {
            Entry::Writer { store, .. }
            | Entry::Daemon(Open::Followed(store))
            | Entry::Unheld(Open::Followed(store)) => Some(store),
            _ => None,
        }
    }

    /// The point kept open of the store, if one is.
    fn point(&mut self) -> Option<&mut P> {
        match self {
            Entry::Daemon(Open::Point(point)) | Entry::Unheld(Open::Point(point)) => Some(point),
            _ => None,
        }
    }

    /// Whether it is followers' store, none having it any more: a stale entry.
    fn is_stale(&self) -> bool {
        matches!(self, Entry::Unheld(Open::Followed(instance)) if instance.store.strong_count() == 0)
    }
}

impl<C: Writer, S, P> Inner<C, S, P> {
    /// Holds the store `key` names for `claim`, if it may now: unless a connection not going
    /// writes it, which fails, or a connection going holds it, or the daemon, which is waited for.
    /// A writer takes over the store followers have open, and the point kept open, to close.
    fn claim(&mut self, key: &Path, claim: &Claim<C>) -> Result<Claimed<C, P>, Refused<C>> {
        let passed = match (self.entries.get(key).and_then(Entry::holder), claim) {
            (Some(Refused::Busy(writer)), Claim::DaemonOrWriter) => {
                return Ok(Claimed::Writer(writer));
            }
            (Some(Refused::Busy(writer)), _) if !rules().one_holder => Some(writer),
            (Some(Refused::Wait(going)), _) if !rules().one_holder => going,
            (Some(refused), _) => return Err(refused),
            (None, _) => None,
        };
        let open = self.entries.remove(key).map_or(Open::Nothing, Entry::into_open);
        let (entry, evicted) = match claim {
            Claim::Writer(connection) => {
                let connection = connection.clone();
                match open {
                    Open::Followed(store) => (Entry::Writer { connection, store }, None),
                    Open::Point(point) => {
                        (Entry::Writer { connection, store: Instance::none() }, Some(point))
                    }
                    Open::Nothing => (Entry::Writer { connection, store: Instance::none() }, None),
                }
            }
            Claim::Daemon | Claim::DaemonOrWriter => (Entry::Daemon(open), None),
        };
        self.entries.insert(key.to_owned(), entry);
        Ok(Claimed::Held { point: evicted, passed })
    }
}

impl<C, S, P> StoreTable<C, S, P> {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner { entries: HashMap::new(), waiting: HashMap::new() }) }
    }

    /// The connection writing the store `key` names, if one does, going or not.
    pub fn writer(&self, key: &Path) -> Option<Arc<C>> {
        match self.inner.lock().unwrap().entries.get(key)? {
            Entry::Writer { connection, .. } => connection.upgrade(),
            _ => None,
        }
    }

    /// The store connections have open at `key`, its writer's or followers', if one is, and
    /// whether it is retired.
    pub fn instance(&self, key: &Path) -> Option<(Arc<S>, bool)> {
        self.inner.lock().unwrap().entries.get_mut(key)?.instance()?.get()
    }

    /// `f`'s of the point kept open at `key`, if one is: under the table's lock, which `f` takes
    /// for no longer than to look at it.
    pub fn point<R>(&self, key: &Path, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        self.inner.lock().unwrap().entries.get_mut(key)?.point().map(f)
    }

    /// The stores connections have open, writers' and followers', by their keys.
    pub fn instances(&self) -> Vec<(PathBuf, Arc<S>)> {
        let mut inner = self.inner.lock().unwrap();
        (inner.entries.iter_mut())
            .filter_map(|(key, entry)| Some((key.clone(), entry.instance()?.store.upgrade()?)))
            .collect()
    }

    /// How many stores connections write.
    pub fn writers(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        (inner.entries.values())
            .filter(|entry| matches!(entry, Entry::Writer { connection, .. } if connection.strong_count() > 0))
            .count()
    }

    /// Whether something waits to hold the store `key` names.
    #[cfg(test)]
    pub fn is_waited_for(&self, key: &Path) -> bool {
        self.inner.lock().unwrap().waiting.contains_key(key)
    }

    /// Whether nothing holds a store, nor has one open.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.entries.values().all(Entry::is_stale)
    }

    /// Lets the store `key` names go, its hold dropped: what is open of it stays, but a store none
    /// has open any more; those waiting for it alone are woken.
    fn release(&self, key: &Path) {
        let mut inner = self.inner.lock().unwrap();
        let left = inner.entries.remove(key).map_or(Open::Nothing, Entry::into_open);
        let discarded = match rules().open_outlives_holders {
            true if !left.is_nothing() => {
                inner.entries.insert(key.to_owned(), Entry::Unheld(left));
                None
            }
            _ => Some(left),
        };
        if rules().release_wakes
            && let Some(released) = inner.waiting.remove(key)
        {
            released.notify(usize::MAX);
        }
        drop(inner);
        // Without the lock: a point closes as it is dropped.
        drop(discarded);
    }
}

impl<C: Writer, S, P> StoreTable<C, S, P> {
    /// Holds the store `key` names, for the daemon: once its holder, if it has one, lets it go,
    /// if that holder is going, or is the daemon, waiting at most `wait` (for ever, if none);
    /// unless a connection not going writes it, which fails with `EBUSY`. What is open of it stays.
    /// It blocks.
    pub fn hold(&self, key: &Path, wait: Option<Duration>) -> Result<Hold<'_, C, S, P>, Errno> {
        let deadline = wait.map(|wait| Instant::now() + wait);
        match self.claim_blocking(key, &Claim::Daemon, deadline)? {
            Claimed::Held { .. } => Ok(self.held(key)),
            Claimed::Writer(_) => unreachable!("the daemon's claim got a writer"),
        }
    }

    /// Holds the store `key` names, for the daemon, as [`StoreTable::hold`] does, waiting at most
    /// `wait`: a future, which holds no thread while it waits (TASK-43.6).
    pub async fn hold_async(&self, key: &Path, wait: Duration) -> Result<Hold<'_, C, S, P>, Errno> {
        match self.claim_async(key, Claim::Daemon, wait).await? {
            Claimed::Held { .. } => Ok(self.held(key)),
            Claimed::Writer(_) => unreachable!("the daemon's claim got a writer"),
        }
    }

    /// Holds the store `key` names, for `connection` to write it, as [`StoreTable::hold_async`]
    /// does; with the point kept open of it, if one was, which the connection closes before it
    /// opens the store. The store followers have open, it takes over, or finds retired
    /// ([`Hold::instance`]).
    pub async fn hold_to_write(
        &self,
        key: &Path,
        connection: Weak<C>,
        wait: Duration,
    ) -> Result<(Hold<'_, C, S, P>, Option<P>), Errno> {
        match self.claim_async(key, Claim::Writer(connection), wait).await? {
            Claimed::Held { point, .. } => Ok((self.held(key), point)),
            Claimed::Writer(_) => unreachable!("a writer's claim got a writer"),
        }
    }

    /// The connection writing the store `key` names, if one does that isn't going; or, if none
    /// does, the store held for the daemon. `EBUSY` if a connection going holds it, or the
    /// daemon: one going would carry out no request of its guest's, which is gone (TASK-79.34).
    pub fn writer_or_hold(&self, key: &Path) -> Result<Holding<'_, C, S, P>, Errno> {
        match self.try_claim(key, &Claim::DaemonOrWriter, Some(Instant::now()))? {
            Ok(claimed) => Ok(self.holding(key, claimed)),
            Err(_) => Err(Errno::EBUSY),
        }
    }

    /// The connection writing the store `key` names, if one does that isn't going; or, if none
    /// does, the store held for the daemon: once a connection going, or the daemon, lets it go,
    /// waiting at most `wait` (`EBUSY`), holding no thread meanwhile.
    pub async fn writer_or_hold_async(
        &self,
        key: &Path,
        wait: Duration,
    ) -> Result<Holding<'_, C, S, P>, Errno> {
        let claimed = self.claim_async(key, Claim::DaemonOrWriter, wait).await?;
        Ok(self.holding(key, claimed))
    }

    fn held(&self, key: &Path) -> Hold<'_, C, S, P> {
        Hold { table: self, key: key.to_owned() }
    }

    fn holding(&self, key: &Path, claimed: Claimed<C, P>) -> Holding<'_, C, S, P> {
        match claimed {
            Claimed::Held { .. } => Holding::Held(self.held(key)),
            Claimed::Writer(writer) => Holding::Writer(writer),
        }
    }

    /// Claims the store `key` names for `claim` now; or, if its holder lets it go soon, says what
    /// tells it did, unless `deadline` passed: `EBUSY` then, and if a connection not going writes
    /// it.
    fn try_claim(
        &self,
        key: &Path,
        claim: &Claim<C>,
        deadline: Option<Instant>,
    ) -> Result<Result<Claimed<C, P>, EventListener>, Errno> {
        let mut inner = self.inner.lock().unwrap();
        let (tried, refused) = match inner.claim(key, claim) {
            Ok(claimed) => (Ok(Ok(claimed)), None),
            Err(Refused::Wait(going))
                if deadline.is_none_or(|deadline| Instant::now() < deadline) =>
            {
                let released = inner.waiting.entry(key.to_owned()).or_default().listen();
                (Ok(Err(released)), going)
            }
            Err(Refused::Wait(going)) => (Err(Errno::EBUSY), going),
            Err(Refused::Busy(writer)) => (Err(Errno::EBUSY), Some(writer)),
        };
        drop(inner);
        drop(refused);
        tried
    }

    /// Claims the store `key` names, waiting on this thread, until `deadline` at most (for ever,
    /// if none): `EBUSY` then.
    fn claim_blocking(
        &self,
        key: &Path,
        claim: &Claim<C>,
        deadline: Option<Instant>,
    ) -> Result<Claimed<C, P>, Errno> {
        loop {
            let released = match self.try_claim(key, claim, deadline)? {
                Ok(claimed) => return Ok(claimed),
                Err(released) => released,
            };
            if !wait_blocking(released, deadline) {
                return Err(Errno::EBUSY);
            }
        }
    }

    /// Claims the store `key` names, waiting at most `wait` (`EBUSY` then), holding no thread
    /// meanwhile.
    async fn claim_async(
        &self,
        key: &Path,
        claim: Claim<C>,
        wait: Duration,
    ) -> Result<Claimed<C, P>, Errno> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            let released = match self.try_claim(key, &claim, deadline)? {
                Ok(claimed) => return Ok(claimed),
                Err(released) => released,
            };
            let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            match left {
                None => released.await,
                Some(left) => within(left, released).await.ok_or(Errno::EBUSY)?,
            }
        }
    }
}

impl<C, S, P> Hold<'_, C, S, P> {
    /// The key of the store it holds.
    pub fn key(&self) -> &Path {
        &self.key
    }

    /// The store connections have open at its key, if one is, and whether it is retired: its
    /// writer's, or followers', which a writer holding it takes over.
    pub fn instance(&self) -> Option<(Arc<S>, bool)> {
        self.table.instance(&self.key)
    }

    /// `store` is the store connections have open at its key from now on, while one has it: the
    /// writer's holding it, or, held by the daemon, followers'. `EBUSY`, and nothing changed, if
    /// a store is open there, or a point kept: what holds the store opens it only where it found
    /// nothing open, or takes over what it found ([`Hold::instance`]).
    ///
    /// Its methods change its entry only if it is its holder's, which nothing but a claim past
    /// [`Rules::one_holder`] takes from it, and only as they say, refusing otherwise: none panics,
    /// which, under the table's lock, would poison every store's entry, as under Shuttle it would
    /// abort the run (TASK-55).
    pub fn open(&self, store: &Arc<S>) -> Result<(), Errno> {
        let instance = Instance { store: Arc::downgrade(store), retired: false };
        let mut inner = self.table.inner.lock().unwrap();
        inner.entries.retain(|_, entry| !entry.is_stale());
        match inner.entries.get_mut(&self.key) {
            Some(Entry::Writer { store, .. }) if store.store.strong_count() == 0 => {
                *store = instance;
                Ok(())
            }
            Some(Entry::Daemon(open)) if open.is_closed() => {
                *open = Open::Followed(instance);
                Ok(())
            }
            _ => Err(Errno::EBUSY),
        }
    }

    /// Retires the store connections have open at its key, its writer gone with writes
    /// uncommitted: `cut` cuts its followers off, without the table's lock, and neither a writer
    /// nor a follower opens it while followers have it.
    pub fn retire(&self, cut: impl FnOnce()) {
        cut();
        if !rules().retirement {
            return;
        }
        let mut inner = self.table.inner.lock().unwrap();
        if let Some(instance) = inner.entries.get_mut(&self.key).and_then(Entry::instance) {
            instance.retired = true;
        }
    }

    /// Keeps `point` open at its key for forks, after its hold too: the daemon's, which found
    /// nothing open there. Refused, `point` given back, if a store is open there or a point kept
    /// already, or a writer holds it.
    pub fn keep(&self, point: P) -> Result<(), P> {
        let mut inner = self.table.inner.lock().unwrap();
        match inner.entries.get_mut(&self.key) {
            Some(Entry::Daemon(open)) if open.is_closed() => {
                *open = Open::Point(point);
                Ok(())
            }
            // Dropped without the lock: a point closes as it is dropped.
            _ => Err(point),
        }
    }

    /// Takes the point kept open at its key, if one is, to close it.
    pub fn take_point(&self) -> Option<P> {
        let mut inner = self.table.inner.lock().unwrap();
        match inner.entries.get_mut(&self.key) {
            Some(Entry::Daemon(open)) => match std::mem::replace(open, Open::Nothing) {
                Open::Point(point) => Some(point),
                other => {
                    *open = other;
                    None
                }
            },
            _ => None,
        }
    }
}

impl<C, S, P> Drop for Hold<'_, C, S, P> {
    fn drop(&mut self) {
        // Those waiting to hold it may.
        self.table.release(&self.key);
    }
}

/// The table under Shuttle: a VM's connection writing a store, whose VM process goes, with
/// writes uncommitted or not; as it goes, the VM restored from its snapshot opening the store to
/// write it, and followers following it, before and after it went; and a fork of the store at any
/// point, which keeps it open as a point if nothing has it open. Every interleaving of them
/// Shuttle's scheduler chooses (TASK-160.8). Waits are unbounded, so that a holder that never
/// wakes those waiting is a deadlock, which Shuttle reports, rather than a wait of `CLOSING_WAIT`.
/// Invariants: one holder at a time; the store open once at a time, as a writer's, followers' or a
/// point; no follower following the store once its writer retired it. Each rule turned off fails.
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests {
    use super::*;
    use crate::shuttle_testing::{config, fails_with, runs};
    use shuttle::future::block_on;
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::sync::atomic::{AtomicBool, Ordering};
    use shuttle::sync::mpsc;
    use shuttle::{Runner, thread};

    const KEY: &str = "/store";

    /// A connection: going once its VM process is; and, if it writes the store, the store and its
    /// hold of it, let go as the last of the connection is dropped, the store first, as
    /// `extentd::server::Connection`'s are: wherever that is, but under the table's lock, which
    /// letting the hold go takes.
    struct Conn {
        going: AtomicBool,
        writing: std::sync::Mutex<Option<Writing>>,
    }

    impl Conn {
        fn new() -> Arc<Self> {
            Arc::new(Conn { going: AtomicBool::new(false), writing: Default::default() })
        }
    }

    impl Writer for Conn {
        fn going(&self) -> bool {
            self.going.load(Ordering::SeqCst)
        }
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            let writing = self.writing.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(Writing { store, hold }) = writing.take() else { return };
            drop(store);
            // Gone, its hold not let go yet: what claims the store meets a writer whose
            // connection is gone, and waits.
            if hold.hold().table.is_waited_for(Path::new(KEY)) {
                reached("a claim waited for a writer gone");
            }
            drop(hold);
        }
    }

    /// What a run counts: the store's openings open at once, and the most at once; its holders at
    /// once, and whether ever two. Checked once the run is done: a task panicking unwinds through
    /// the holds it has, whose drops take Shuttle's locks, which fail in a failed run with panics
    /// that abort (TASK-55). Not Shuttle's atomics, whose every access the scheduler may switch
    /// threads at: a store's closing is counted as its last holder drops it, rather than after
    /// another thread found it gone and opened it anew, which the table doesn't order: the store's
    /// own lock of its files does (`ExtentStore::open`).
    #[derive(Default)]
    struct Counts {
        open: std::sync::atomic::AtomicUsize,
        most_open: std::sync::atomic::AtomicUsize,
        holders: std::sync::atomic::AtomicUsize,
        two_holders: std::sync::atomic::AtomicBool,
    }

    /// The store, opened: counted open until it is dropped, closed; dirty once its writer went
    /// with writes uncommitted. A point's says it is closed, dropping what does.
    struct Store {
        counts: Arc<Counts>,
        dirty: AtomicBool,
        _closing: Option<mpsc::Sender<()>>,
    }

    impl Store {
        fn open(counts: &Arc<Counts>) -> Arc<Self> {
            Self::opened(counts, None)
        }

        fn opened(counts: &Arc<Counts>, closing: Option<mpsc::Sender<()>>) -> Arc<Self> {
            let others = counts.open.fetch_add(1, Ordering::SeqCst);
            counts.most_open.fetch_max(others + 1, Ordering::SeqCst);
            let dirty = AtomicBool::new(false);
            Arc::new(Store { counts: counts.clone(), dirty, _closing: closing })
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            self.counts.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A point kept open, as `extentd::stores::Point` is: the store opened, which each fork of it
    /// holds while it forks it, and what says it is closed, once the last lets it go.
    struct Point {
        store: Arc<Store>,
        closed: mpsc::Receiver<()>,
    }

    impl Point {
        fn open(counts: &Arc<Counts>) -> Self {
            let (closing, closed) = mpsc::channel();
            Point { store: Store::opened(counts, Some(closing)), closed }
        }
    }

    /// Closes `point`, if given, and returns once it is closed: once the forks forking it are
    /// done, as `extentd::stores::close_point` does.
    fn close(point: Option<Point>) {
        let Some(Point { store, closed }) = point else { return };
        drop(store);
        if closed.try_recv() == Err(mpsc::TryRecvError::Empty) {
            reached("a point closed waited for a fork of it");
        }
        let _ = closed.recv();
    }

    type Table = StoreTable<Conn, Store, Point>;

    /// What the runs reached, of the ways through the table a test is to take
    /// ([`reaching`]): across runs, outside Shuttle's.
    static REACHED: std::sync::Mutex<std::collections::BTreeSet<&'static str>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());

    fn reached(what: &'static str) {
        REACHED.lock().unwrap().insert(what);
    }

    /// Runs `runs`, checking they reached each of `ways`: schedules that never reach them would
    /// pass, testing nothing.
    fn reaching(ways: &[&'static str], runs: impl Fn()) {
        runs();
        let reached = REACHED.lock().unwrap();
        let missed: Vec<_> = ways.iter().filter(|way| !reached.contains(*way)).collect();
        assert!(missed.is_empty(), "no run reached {missed:?}, only {reached:?}");
    }

    /// A hold counted among the store's holders, which none other may be, until dropped.
    struct Counted<'t> {
        counts: Arc<Counts>,
        _hold: Hold<'t, Conn, Store, Point>,
    }

    impl<'t> Counted<'t> {
        fn new(counts: &Arc<Counts>, hold: Hold<'t, Conn, Store, Point>) -> Self {
            if counts.holders.fetch_add(1, Ordering::SeqCst) > 0 {
                counts.two_holders.store(true, Ordering::SeqCst);
            }
            Self { counts: counts.clone(), _hold: hold }
        }

        fn hold(&self) -> &Hold<'t, Conn, Store, Point> {
            &self._hold
        }
    }

    impl Drop for Counted<'_> {
        fn drop(&mut self) {
            self.counts.holders.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// What a connection writing the store has: its store, closed before its hold lets it go
    /// ([`Conn`]).
    struct Writing {
        store: Arc<Store>,
        hold: Counted<'static>,
    }

    /// Opens the store for `conn` to write, as a connection does (`extentd::server`): holding it,
    /// closing the point kept of it, taking over the store followers have open, unless it is
    /// retired, or not quiescent, its writer gone with writes uncommitted (`ExtentStore::take_over`);
    /// or opening it. The connection has it from then on.
    fn write(table: &'static Table, counts: &Arc<Counts>, conn: &Arc<Conn>) -> Result<(), Errno> {
        let (hold, point) =
            block_on(table.hold_to_write(Path::new(KEY), Arc::downgrade(conn), Duration::MAX))?;
        let hold = Counted::new(counts, hold);
        if point.is_some() {
            reached("a writer closed a point kept");
        }
        close(point);
        let store = match hold.hold().instance() {
            Some((_, true)) => {
                reached("a writer found the store retired");
                return Err(Errno::EBUSY);
            }
            Some((store, false)) if store.dirty.load(Ordering::SeqCst) => {
                return Err(Errno::EBUSY);
            }
            Some((store, false)) => {
                reached("a writer took the store over");
                store
            }
            None => Store::open(counts),
        };
        hold.hold().open(&store)?;
        *conn.writing.lock().unwrap() = Some(Writing { store, hold });
        Ok(())
    }

    /// Follows the store, as a follower does (`extentd::stores::followed`): the store connections
    /// have open, unless retired; or, held by the daemon, its point closed, the store opened.
    fn follow(table: &Table, counts: &Arc<Counts>) -> Result<Arc<Store>, Errno> {
        let open = |found: Option<(Arc<Store>, bool)>| match found {
            Some((_, true)) => Err(Errno::EBUSY),
            Some((store, false)) => Ok(Some(store)),
            None => Ok(None),
        };
        if let Some(store) = open(table.instance(Path::new(KEY)))? {
            reached("a follower followed the store open");
            return Ok(store);
        }
        let hold = Counted::new(counts, table.hold(Path::new(KEY), None)?);
        close(hold.hold().take_point());
        if let Some(store) = open(hold.hold().instance())? {
            return Ok(store);
        }
        reached("a follower opened the store");
        let store = Store::open(counts);
        hold.hold().open(&store)?;
        Ok(store)
    }

    /// Forks the store, as the daemon does (`extentd::stores::fork`): through its writer, or the
    /// store followers have open, live; or the point kept, or else the store opened, kept open as
    /// a point, which it holds while it forks it, as a restore's fork does, its hold let go.
    fn fork(table: &Table, counts: &Arc<Counts>) -> Result<(), Errno> {
        let kept = |point: &mut Point| point.store.clone();
        let forking = match table.point(Path::new(KEY), kept) {
            Some(forking) => forking,
            None => {
                let hold =
                    match block_on(table.writer_or_hold_async(Path::new(KEY), Duration::MAX))? {
                        Holding::Writer(_) => return Ok(()),
                        Holding::Held(hold) => Counted::new(counts, hold),
                    };
                if hold.hold().instance().is_some() {
                    return Ok(());
                }
                match table.point(Path::new(KEY), kept) {
                    Some(forking) => forking,
                    None => {
                        reached("a fork kept a point");
                        let point = Point::open(counts);
                        let forking = point.store.clone();
                        // Refused only past one holder, which the run then fails for.
                        let _ = hold.hold().keep(point);
                        forking
                    }
                }
            }
        };
        thread::yield_now();
        drop(forking);
        Ok(())
    }

    /// The run, its first writer going with writes uncommitted if `dirty`.
    fn run(dirty: bool) {
        let table: &'static Table = Box::leak(Box::new(StoreTable::new()));
        let counts = Arc::new(Counts::default());
        let (followed_tx, followed) = mpsc::channel::<()>();
        let (later_tx, later) = mpsc::channel::<()>();
        let (going_tx, going) = mpsc::channel::<()>();
        let released = Arc::new(AtomicBool::new(false));

        // The VM's connection, whose VM process goes while it writes the store.
        let first = {
            let (counts, released) = (counts.clone(), released.clone());
            thread::spawn(move || {
                let conn = Conn::new();
                write(table, &counts, &conn).unwrap();
                followed_tx.send(()).unwrap();
                later_tx.send(()).unwrap();
                conn.going.store(true, Ordering::SeqCst);
                going_tx.send(()).unwrap();
                if dirty {
                    let writing = conn.writing.lock().unwrap();
                    let Writing { store, hold } = writing.as_ref().unwrap();
                    store.dirty.store(true, Ordering::SeqCst);
                    hold.hold().retire(|| ());
                }
                // The last of it, unless a claim of the store has it as it meets it: it lets the
                // store go then, outside the table's lock.
                drop(conn);
                released.store(true, Ordering::SeqCst);
            })
        };
        // The VM restored from its snapshot, opening the store as the first goes: it gets it
        // once the first lets it go, unless followers have the first's store, retired.
        let second = {
            let counts = counts.clone();
            thread::spawn(move || {
                going.recv().unwrap();
                let conn = Conn::new();
                if write(table, &counts, &conn).is_ok() {
                    thread::yield_now();
                }
                drop(conn);
            })
        };
        // A follower of the first's store, following it for a while.
        let follower = {
            let counts = counts.clone();
            thread::spawn(move || {
                followed.recv().unwrap();
                if let Ok(store) = follow(table, &counts) {
                    thread::yield_now();
                    thread::yield_now();
                    drop(store);
                }
            })
        };
        // A follower, following the store as the first goes: never its store once it retired it.
        let following_later = {
            let (counts, released) = (counts.clone(), released.clone());
            thread::spawn(move || {
                later.recv().unwrap();
                let after = released.load(Ordering::SeqCst);
                if let Ok(store) = follow(table, &counts) {
                    let retired = after && store.dirty.load(Ordering::SeqCst);
                    drop(store);
                    assert!(!retired, "a follower followed a store its writer retired");
                }
            })
        };
        // A fork of the store, at any point.
        let forking = {
            let counts = counts.clone();
            thread::spawn(move || fork(table, &counts).unwrap())
        };
        for thread in [first, second, follower, following_later, forking] {
            thread.join().unwrap();
        }
        // The point a fork kept, closed, as it is once no fork forks it.
        let hold = table.hold(Path::new(KEY), None).unwrap();
        close(hold.take_point());
        drop(hold);
        let two = counts.two_holders.load(Ordering::SeqCst);
        assert!(!two, "two holders hold the store at once");
        let most = counts.most_open.load(Ordering::SeqCst);
        assert!(most <= 1, "the store is open twice at once: {most} at most");
        assert!(table.is_empty(), "the store is held or open once all let it go");
    }

    fn randomly(dirty: bool) {
        Runner::new(RandomScheduler::new(runs(3000)), config()).run(move || run(dirty));
    }

    fn by_pct(dirty: bool) {
        Runner::new(PctScheduler::new(3, runs(3000)), config()).run(move || run(dirty));
    }

    /// What runs of a writer going quiescent reach: its store taken over, followed, and points
    /// kept and closed.
    const QUIESCENT: &[&str] = &[
        "a writer took the store over",
        "a writer closed a point kept",
        "a point closed waited for a fork of it",
        "a claim waited for a writer gone",
        "a follower followed the store open",
        "a follower opened the store",
        "a fork kept a point",
    ];

    /// What runs of a writer going with writes uncommitted reach besides: its store retired.
    const UNCOMMITTED: &[&str] =
        &["a writer found the store retired", "a follower followed the store open"];

    #[test]
    fn writers_going_quiescent_randomly() {
        set_rules(Rules::ALL);
        reaching(QUIESCENT, || randomly(false));
    }

    #[test]
    fn writers_going_quiescent_by_pct() {
        set_rules(Rules::ALL);
        reaching(QUIESCENT, || by_pct(false));
    }

    #[test]
    fn writers_going_with_writes_uncommitted_randomly() {
        set_rules(Rules::ALL);
        reaching(UNCOMMITTED, || randomly(true));
    }

    #[test]
    fn writers_going_with_writes_uncommitted_by_pct() {
        set_rules(Rules::ALL);
        reaching(UNCOMMITTED, || by_pct(true));
    }

    /// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`.
    fn fails_without(rules: Rules, dirty: bool, expected: &str) {
        set_rules(rules);
        fails_with(expected, || randomly(dirty));
    }

    #[test]
    fn needs_one_holder() {
        let rules = Rules { one_holder: false, ..Rules::ALL };
        fails_without(rules, false, "two holders hold the store");
    }

    /// The resize that let the store go without waking those waiting for it (fixed by having it
    /// release through `release`): they waited `CLOSING_WAIT` out.
    #[test]
    fn needs_releases_to_wake() {
        fails_without(Rules { release_wakes: false, ..Rules::ALL }, false, "deadlock!");
    }

    /// A writer's store followers have, forgotten as it goes, is opened anew by the next.
    #[test]
    fn needs_what_is_open_to_outlive_its_holders() {
        let rules = Rules { open_outlives_holders: false, ..Rules::ALL };
        fails_without(rules, false, "open twice at once");
    }

    /// A store its writer retired, forgotten as such, is followed after.
    #[test]
    fn needs_retirement() {
        let rules = Rules { retirement: false, ..Rules::ALL };
        fails_without(rules, true, "followed a store its writer retired");
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Connection(AtomicBool);

    impl Writer for Connection {
        fn going(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn connection(going: bool) -> Arc<Connection> {
        Arc::new(Connection(AtomicBool::new(going)))
    }

    /// A store, and a point kept open, each named.
    type Table = StoreTable<Connection, &'static str, &'static str>;

    /// Holds `key` for `connection` to write it, at once.
    fn write<'t>(
        table: &'t Table,
        key: &Path,
        connection: &Arc<Connection>,
    ) -> Result<(Hold<'t, Connection, &'static str, &'static str>, Option<&'static str>), Errno>
    {
        block_on(table.hold_to_write(key, Arc::downgrade(connection), Duration::ZERO))
    }

    /// One waiting for a store a going holder never lets go gives up once its wait is over.
    #[test]
    fn waits_for_stores_never_let_go_end() {
        let table = Table::new();
        let (going, next) = (connection(true), connection(false));
        let key = Path::new("store");
        let _held = write(&table, key, &going).unwrap();
        let wait = Duration::from_millis(10);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let held = runtime.block_on(table.hold_to_write(key, Arc::downgrade(&next), wait));
        assert_eq!(held.err(), Some(Errno::EBUSY));
        let held = runtime.block_on(table.writer_or_hold_async(key, wait));
        assert_eq!(held.err(), Some(Errno::EBUSY));
        assert_eq!(table.hold(key, Some(wait)).err(), Some(Errno::EBUSY));
    }

    /// TASK-79.34: a store whose holder is going, its VM process gone, is the daemon's neither to
    /// work on through its holder, nor here, until it lets it go: `EBUSY`.
    #[test]
    fn stores_of_going_holders_are_busy() {
        let table = Table::new();
        let going = connection(true);
        let key = Path::new("store");
        let held = write(&table, key, &going).unwrap();
        assert_eq!(table.writer_or_hold(key).err(), Some(Errno::EBUSY));
        drop(held);
        assert!(matches!(table.writer_or_hold(key), Ok(Holding::Held(_))));
    }

    /// A store a connection not going writes is its: the daemon works through it, and no other
    /// holds it, but fails at once with `EBUSY`.
    #[test]
    fn stores_writers_write_are_theirs() {
        let table = Table::new();
        let (writer, next) = (connection(false), connection(false));
        let key = Path::new("store");
        let _held = write(&table, key, &writer).unwrap();
        let through = table.writer_or_hold(key);
        assert!(matches!(through, Ok(Holding::Writer(found)) if Arc::ptr_eq(&found, &writer)));
        assert_eq!(write(&table, key, &next).err(), Some(Errno::EBUSY));
        assert_eq!(table.hold(key, None).err(), Some(Errno::EBUSY));
    }

    /// TASK-92.5: a store let go wakes those waiting to hold it, whose wait goes with it: those
    /// waiting for another store wait on.
    #[test]
    fn releases_wake_those_waiting_for_their_store() {
        let table = Table::new();
        let going = connection(true);
        let (first, second) = (Path::new("first"), Path::new("second"));
        let first_held = write(&table, first, &going).unwrap();
        let _second_held = write(&table, second, &going).unwrap();
        let (ours, theirs) = (connection(false), connection(false));
        let wait = Duration::from_secs(60);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let ours_held = runtime.block_on(async {
            let mut holding_first =
                std::pin::pin!(table.hold_to_write(first, Arc::downgrade(&ours), wait));
            let mut holding_second =
                std::pin::pin!(table.hold_to_write(second, Arc::downgrade(&theirs), wait));
            assert!(futures::poll!(&mut holding_first).is_pending());
            assert!(futures::poll!(&mut holding_second).is_pending());
            assert_eq!(table.inner.lock().unwrap().waiting.len(), 2);
            drop(first_held);
            assert_eq!(table.inner.lock().unwrap().waiting.len(), 1);
            let (held, _) = holding_first.await.unwrap();
            assert!(futures::poll!(&mut holding_second).is_pending());
            held
        });
        assert!(Arc::ptr_eq(&table.writer(first).unwrap(), &ours));
        drop(ours_held);
    }

    /// The stores connections write are counted, not the daemon's, nor those of connections gone.
    #[test]
    fn stores_writers_write_are_counted() {
        let table = Table::new();
        let (first, second) = (connection(false), connection(false));
        let _first = write(&table, Path::new("first"), &first).unwrap();
        let _second = write(&table, Path::new("second"), &second).unwrap();
        let _daemons = table.hold(Path::new("daemon's"), None).unwrap();
        assert_eq!(table.writers(), 2);
        drop(second);
        assert_eq!(table.writers(), 1);
    }

    /// A writer's store outlives its writer while followers have it: the next writer takes it
    /// over; gone once none has it, it is nothing, and the next opens its own.
    #[test]
    fn stores_followers_have_outlive_their_writers() {
        let table = Table::new();
        let key = Path::new("store");
        let (writer, next) = (connection(false), connection(false));
        let (held, _) = write(&table, key, &writer).unwrap();
        let store = Arc::new("writer's");
        held.open(&store).unwrap();
        let followed = table.instance(key).unwrap().0;
        writer.0.store(true, Ordering::Relaxed);
        drop((held, store));
        assert_eq!(
            table.instance(key).map(|(store, retired)| (*store, retired)),
            Some(("writer's", false))
        );
        let (held, _) = write(&table, key, &next).unwrap();
        assert!(Arc::ptr_eq(&held.instance().unwrap().0, &followed));
        drop(held);
        drop(followed);
        assert_eq!(table.instance(key), None);
        assert!(table.is_empty());
        let (held, _) = write(&table, key, &next).unwrap();
        assert_eq!(held.instance(), None);
    }

    /// A store retired, its writer gone with writes uncommitted, is so while followers have it:
    /// to the next writer, and to the daemon.
    #[test]
    fn stores_retired_stay_retired_while_followed() {
        let table = Table::new();
        let key = Path::new("store");
        let writer = connection(false);
        let (held, _) = write(&table, key, &writer).unwrap();
        let store = Arc::new("writer's");
        held.open(&store).unwrap();
        held.retire(|| ());
        drop(held);
        assert_eq!(table.instance(key).map(|(_, retired)| retired), Some(true));
        let held = table.hold(key, None).unwrap();
        assert_eq!(held.instance().map(|(_, retired)| retired), Some(true));
        drop(held);
        let (held, _) = write(&table, key, &connection(false)).unwrap();
        assert_eq!(held.instance().map(|(_, retired)| retired), Some(true));
        drop((held, store));
        assert!(table.is_empty());
    }

    /// A point kept open outlives the daemon's hold, which forks see; the daemon's next holds keep
    /// it, unless they take it; a writer's takes it, to close.
    #[test]
    fn points_kept_go_to_writers() {
        let table = Table::new();
        let key = Path::new("point");
        let held = table.hold(key, None).unwrap();
        held.keep("point").unwrap();
        drop(held);
        assert_eq!(table.point(key, |point| *point), Some("point"));
        let held = table.hold(key, None).unwrap();
        assert_eq!(table.point(key, |point| *point), Some("point"));
        drop(held);
        let (held, evicted) = write(&table, key, &connection(false)).unwrap();
        assert_eq!(evicted, Some("point"));
        assert_eq!(table.point(key, |point| *point), None);
        drop(held);
        assert!(table.is_empty());
        let held = table.hold(key, None).unwrap();
        held.keep("again").unwrap();
        assert_eq!(held.take_point(), Some("again"));
        drop(held);
        assert!(table.is_empty());
    }

    /// A store opened where one is open, or a point kept, is refused, and changes nothing: its
    /// holder takes over what it found, or works on it, rather than open it twice.
    #[test]
    fn stores_opened_where_something_is_open_are_refused() {
        let table = Table::new();
        let key = Path::new("store");
        let (held, _) = write(&table, key, &connection(false)).unwrap();
        let (first, second) = (Arc::new("first"), Arc::new("second"));
        held.open(&first).unwrap();
        assert_eq!(held.open(&second), Err(Errno::EBUSY));
        drop(held);
        let held = table.hold(key, None).unwrap();
        assert_eq!(held.open(&second), Err(Errno::EBUSY));
        assert!(Arc::ptr_eq(&held.instance().unwrap().0, &first));
        drop((held, first));
        let held = table.hold(key, None).unwrap();
        held.keep("point").unwrap();
        assert_eq!(held.open(&second), Err(Errno::EBUSY));
        assert_eq!(held.instance(), None);
        assert_eq!(held.take_point(), Some("point"));
        held.open(&second).unwrap();
        assert!(Arc::ptr_eq(&held.instance().unwrap().0, &second));
    }

    /// A point kept where one is kept already, or a store is open, or by a writer, is refused,
    /// given back, and changes nothing.
    #[test]
    fn points_kept_where_something_is_open_are_refused() {
        let table = Table::new();
        let key = Path::new("point");
        let held = table.hold(key, None).unwrap();
        held.keep("first").unwrap();
        assert_eq!(held.keep("second"), Err("second"));
        assert_eq!(table.point(key, |point| *point), Some("first"));
        assert_eq!(held.take_point(), Some("first"));
        let store = Arc::new("store");
        held.open(&store).unwrap();
        assert_eq!(held.keep("second"), Err("second"));
        drop((held, store));
        let (held, _) = write(&table, key, &connection(false)).unwrap();
        assert_eq!(held.keep("second"), Err("second"));
        assert_eq!(table.point(key, |point| *point), None);
    }
}
