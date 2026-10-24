// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the extent daemon's requests do with a store a connection may hold (TASK-44.6): resizing
//! it, through the connection holding it, whose guest agrees to a shrink first, or here, if none
//! does, growing it alone; and choosing the store a fork forks: the holder's, live, or the store
//! opened here, which no connection opens meanwhile. Generic over the store, its guest and its
//! connection, as `open_stores` is over its holders, so that its Shuttle tests run this code, the
//! daemon's own, with models of them: every interleaving of opens, commits, resizes, forks and
//! closes the scheduler chooses, against what the daemon promises.

use crate::Errno;
use crate::open_stores::{Holder, Paths, within};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(feature = "shuttle")]
use shuttle::sync::Mutex;
#[cfg(not(feature = "shuttle"))]
use std::sync::Mutex;

/// The rules the requests rely on, each of which the Shuttle tests show to be needed by turning
/// it off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A resize of a store a connection holds goes through the connection, rather than to the
    /// store here.
    pub through_holder: bool,
    /// A shrink waits for the guest to agree.
    pub guest_agrees: bool,
    /// A fork of a store a connection holds forks the connection's store, rather than the store
    /// opened here, which the daemon holds while it forks it.
    pub fork_through_holder: bool,
    /// A connection's resizes of its store run one at a time (TASK-79.41).
    pub resizes_one_at_a_time: bool,
    /// A fork's store opened here keeps what holds its path, let go once the store is closed,
    /// rather than the fork's call, which, cancelled, lets it go before then (TASK-79.34).
    pub forks_hold_their_paths: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        through_holder: true,
        guest_agrees: true,
        fork_through_holder: true,
        resizes_one_at_a_time: true,
        forks_hold_their_paths: true,
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

/// A store a request works on, which it closes once done.
pub(crate) trait Close: Send {
    /// Closes it: returns once it is, as a store opened here closes on a blocking thread.
    fn close(self) -> impl Future<Output = ()> + Send;
}

/// A store's capacity, as a request sees it.
pub(crate) trait Capacity: Close + Sync {
    /// Its capacity, and the bytes it holds, which no shrink goes below.
    fn capacity(&self) -> Result<(u64, u64), Errno>;

    /// Sets its capacity to `capacity` bytes, durably.
    fn set_capacity(&self, capacity: u64) -> impl Future<Output = Result<(), Errno>> + Send;
}

/// The guest of a connection holding a store.
pub(crate) trait Guest: Send + Sync {
    /// Tells the guest its store's capacity is `capacity` bytes, or, if `ask`, asks it to shrink
    /// to it, which it answers through the connection's [`Shrinking::answered`].
    fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno>;
}

/// A connection holding a store, which resizes it ([`resize_held`]).
pub(crate) trait Resizes: Holder {
    fn resize(&self, capacity: u64) -> impl Future<Output = Result<(), Errno>> + Send;
}

/// The shrink a connection's guest was asked to take, and where its answer goes; and the resize of
/// its store under way, which the next waits for ([`resize_held`], TASK-79.41).
pub(crate) struct Shrinking {
    asked: Mutex<Option<(u64, futures::channel::oneshot::Sender<bool>)>>,
    resizing: futures::lock::Mutex<()>,
}

impl Shrinking {
    pub fn new() -> Self {
        Self { asked: Mutex::new(None), resizing: futures::lock::Mutex::new(()) }
    }

    /// The guest's answer to the shrink to `capacity`: whether it took it. An answer to a shrink
    /// no longer waited for goes nowhere.
    pub fn answered(&self, capacity: u64, took: bool) {
        let mut shrinking = self.asked.lock().unwrap();
        if shrinking.as_ref().is_some_and(|(asked, _)| *asked == capacity) {
            let (_, answer) = shrinking.take().unwrap();
            let _ = answer.send(took);
        }
    }

    /// Stops waiting for the guest's answer, its connection gone: the shrink fails as unanswered
    /// (TASK-79.34).
    pub fn stop(&self) {
        let stopped = self.asked.lock().unwrap().take();
        drop(stopped);
    }

    /// Whether a shrink waits for the guest's answer.
    #[cfg(test)]
    pub fn awaited(&self) -> bool {
        self.asked.lock().unwrap().is_some()
    }
}

/// A shrink waiting for the guest's answer, in its connection's [`Shrinking`] until it stops
/// waiting, however it stops: answered, timed out, or its call dropped (TASK-43.6).
struct Waiting<'a> {
    shrinking: &'a Shrinking,
    answered: futures::channel::oneshot::Receiver<bool>,
}

impl<'a> Waiting<'a> {
    fn new(shrinking: &'a Shrinking, capacity: u64) -> Self {
        let (answer, answered) = futures::channel::oneshot::channel();
        *shrinking.asked.lock().unwrap() = Some((capacity, answer));
        Self { shrinking, answered }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut shrinking = self.shrinking.asked.lock().unwrap();
        // Unless a later shrink took its place.
        if shrinking.as_ref().is_some_and(|(_, answer)| answer.is_connected_to(&self.answered)) {
            shrinking.take();
        }
    }
}

/// A shrink the guest was asked to take, which, unless the store took it or the guest refused it,
/// tells the guest the capacity the store keeps as it is dropped, however it ends: unanswered in
/// time, its call dropped, or the store's new capacity failing; so that the guest, having taken it
/// late, grows back (TASK-79.41).
struct Asked<'a, G: Guest> {
    guest: &'a G,
    kept: u64,
    settled: bool,
}

impl<G: Guest> Drop for Asked<'_, G> {
    fn drop(&mut self) {
        if !self.settled {
            // Which fails, logged, once the guest is gone.
            let _ = self.guest.tell(self.kept, false);
        }
    }
}

/// Sets `store`'s capacity to `capacity` bytes, as the connection holding it does, whose guest is
/// `guest`: grows it, and tells the guest; or asks the guest to shrink, through `shrinking`, and
/// shrinks it if the guest does (`ENOSPC` if it doesn't), waiting `wait` at most for its answer
/// (`ETIMEDOUT`), holding no thread meanwhile. Never below what the store holds (`ENOSPC`). One at
/// a time: a grow landing between a shrink's answer and the store's taking it would leave the
/// store smaller than its guest was told (TASK-79.41).
pub(crate) async fn resize_held(
    store: &impl Capacity,
    guest: &impl Guest,
    shrinking: &Shrinking,
    capacity: u64,
    wait: Duration,
) -> Result<(), Errno> {
    let _resizing = match rules().resizes_one_at_a_time {
        true => Some(shrinking.resizing.lock().await),
        false => None,
    };
    let (current, held) = store.capacity()?;
    if capacity < held {
        return Err(Errno::ENOSPC);
    }
    if capacity >= current || !rules().guest_agrees {
        store.set_capacity(capacity).await?;
        return guest.tell(capacity, false);
    }
    let mut asked = Asked { guest, kept: current, settled: false };
    let mut waiting = Waiting::new(shrinking, capacity);
    guest.tell(capacity, true)?;
    let took = within(wait, &mut waiting.answered).await;
    drop(waiting);
    match took {
        Some(Ok(true)) => {
            store.set_capacity(capacity).await?;
            asked.settled = true;
            Ok(())
        }
        Some(Ok(false)) => {
            asked.settled = true;
            Err(Errno::ENOSPC)
        }
        // No answer in time, or its VM process gone without one.
        Some(Err(_)) | None => {
            log::warn!("extentd: no answer from the guest to a shrink to {capacity} bytes");
            Err(Errno::ETIMEDOUT)
        }
    }
}

/// The daemon holding a store, which no connection opens meanwhile, until it is dropped: by the
/// store opened here under it, as it closes, so that its path is let go only once it is
/// (TASK-79.34).
pub(crate) struct Released<'a, H: Holder> {
    paths: &'a Paths<H>,
    key: PathBuf,
}

impl<H: Holder> Drop for Released<'_, H> {
    fn drop(&mut self) {
        // Those waiting to open it may.
        self.paths.release(&self.key);
    }
}

/// Sets the capacity of the store `key` names to `capacity` bytes: through the connection
/// holding it, if one does ([`Resizes::resize`]); or, if none does, here, as `open` opens it,
/// given what holds it meanwhile, which the store keeps until it is closed: growing it, and not
/// shrinking it (`EBUSY`), its guest not here to agree. `EBUSY` while a connection whose VM
/// process is gone holds it, or the daemon.
pub(crate) async fn resize<'a, H, S, F>(
    paths: &'a Paths<H>,
    key: &Path,
    capacity: u64,
    open: impl FnOnce(Option<Released<'a, H>>) -> F,
) -> Result<(), Errno>
where
    H: Resizes,
    S: Capacity,
    F: Future<Output = Result<S, Errno>>,
{
    let held = match rules().through_holder {
        true => match paths.holder_or_hold(key)? {
            Some(holder) => return holder.resize(capacity).await,
            None => Some(Released { paths, key: key.to_owned() }),
        },
        false => None,
    };
    let store = open(held).await?;
    let set = match store.capacity() {
        Ok((current, _)) if capacity < current => Err(Errno::EBUSY),
        Ok(_) => store.set_capacity(capacity).await,
        Err(errno) => Err(errno),
    };
    // Closed before those waiting to open it may (it once let them wait out CLOSING_WAIT: found
    // by open_stores.rs's Shuttle tests, TASK-44.6).
    store.close().await;
    set
}

/// The store a fork of the store `key` names forks: `live`'s, of the connection holding it, if
/// one does whose VM process isn't gone (`EBUSY` if it has none yet); or, if none does, the store
/// as `open` opens it here, given what holds it meanwhile, which the store keeps until it is
/// closed, so that no connection opens it until then; once one going lets it go, waiting `wait`
/// at most (`EBUSY`).
pub(crate) async fn forking<'a, H, S, F>(
    paths: &'a Paths<H>,
    key: &Path,
    live: impl FnOnce(&H) -> Option<S>,
    wait: Duration,
    open: impl FnOnce(Option<Released<'a, H>>) -> F,
) -> Result<S, Errno>
where
    H: Holder,
    S: Close,
    F: Future<Output = Result<S, Errno>>,
{
    if !rules().fork_through_holder {
        return open(None).await;
    }
    if let Some(holder) = paths.holder_or_hold_async(key, wait).await? {
        return live(&holder).ok_or(Errno::EBUSY);
    }
    let held = Released { paths, key: key.to_owned() };
    if !rules().forks_hold_their_paths {
        // Let go of as the call returns.
        return open(None).await;
    }
    open(Some(held)).await
}

/// The requests under Shuttle: a VM's connection opening a store, committing, its guest
/// answering shrinks, and closing once its manager's resizes are done; a fork of the store at any
/// point; and the VM restored, opening the store as the first lets it go: every interleaving of
/// them the scheduler chooses. Invariants: the store is open once at a time, by a connection or
/// here; the resizes end with the store's capacity and its guest's view agreeing, the guest never
/// shrunk below what it holds without agreeing; no fork misses a commit made before it. Each rule
/// turned off fails: the guest's agreement as it prevents, the others opening the store twice.
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests {
    use super::*;
    use crate::shuttle_testing::{config, fails_with, runs};
    use shuttle::future::block_on;
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use shuttle::sync::mpsc;
    use shuttle::{Runner, thread};
    use std::sync::Arc;

    const KEY: &str = "/store";
    /// The store's capacity at first.
    const CAPACITY: u64 = 8;
    /// What the guest holds of its store, which it agrees to shrink no lower.
    const GUEST_HOLDS: u64 = 4;

    /// A store: its capacity, its commits, ordered and made durable, how many have it open, and
    /// how many had it open at once, at most.
    struct Model {
        capacity: AtomicU64,
        ordered: AtomicU64,
        durable: AtomicU64,
        open: AtomicUsize,
        most_open: AtomicUsize,
    }

    impl Model {
        /// A store of [`CAPACITY`], which nothing has open, nor committed to.
        fn new() -> Arc<Self> {
            Arc::new(Model {
                capacity: CAPACITY.into(),
                ordered: 0.into(),
                durable: 0.into(),
                open: 0.into(),
                most_open: 0.into(),
            })
        }

        /// One more has it open, which none other may: checked by the tasks before what they check
        /// that follows from it, and once the run is done ([`Model::opened_once_at_a_time`]), not
        /// here, where a request may hold the holders' lock. A task panicking with a Shuttle lock
        /// held fails the others' locks with panics that can't unwind, which abort the process
        /// (TASK-55).
        fn open(&self) {
            let others = self.open.fetch_add(1, Ordering::SeqCst);
            self.most_open.fetch_max(others + 1, Ordering::SeqCst);
        }

        fn opened_once_at_a_time(&self) {
            let most = self.most_open.load(Ordering::SeqCst);
            assert!(most <= 1, "the store is open twice at once: {most} at most");
        }

        fn close(&self) {
            self.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A store as a request sees it: a connection's, as it holds it, or opened here, as its files
    /// hold it, as of its last durable commit, with what holds it meanwhile, if anything does.
    enum View<'a> {
        Live(Arc<Model>),
        Here(Arc<Model>, Option<Released<'a, Conn>>),
    }

    impl View<'_> {
        fn model(&self) -> &Model {
            match self {
                View::Live(model) | View::Here(model, _) => model,
            }
        }

        /// The last commit a fork of it holds.
        fn forked(&self) -> u64 {
            match self {
                View::Live(model) => model.ordered.load(Ordering::SeqCst),
                View::Here(model, _) => model.durable.load(Ordering::SeqCst),
            }
        }
    }

    impl Close for View<'_> {
        async fn close(self) {
            if let View::Here(model, held) = self {
                model.close();
                drop(held);
            }
        }
    }

    impl Capacity for View<'_> {
        fn capacity(&self) -> Result<(u64, u64), Errno> {
            Ok((self.model().capacity.load(Ordering::SeqCst), 1))
        }

        async fn set_capacity(&self, capacity: u64) -> Result<(), Errno> {
            thread::yield_now();
            self.model().capacity.store(capacity, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A guest: the capacity it sees its store as having, and where the shrinks it is asked to
    /// take go, to be answered ([`answer`]).
    struct ModelGuest {
        view: AtomicU64,
        asks: std::sync::Mutex<Option<mpsc::Sender<u64>>>,
    }

    impl Guest for ModelGuest {
        fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
            if ask {
                let asks = self.asks.lock().unwrap();
                asks.as_ref().ok_or(Errno::EIO)?.send(capacity).map_err(|_| Errno::EIO)?;
                return Ok(());
            }
            assert!(capacity >= GUEST_HOLDS, "the guest was shrunk without agreeing");
            self.view.store(capacity, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A connection holding the store, with its guest.
    struct Conn {
        store: Arc<Model>,
        guest: ModelGuest,
        shrinking: Shrinking,
        going: AtomicBool,
    }

    impl Conn {
        fn new(store: Arc<Model>, asks: Option<mpsc::Sender<u64>>) -> Arc<Self> {
            Arc::new(Conn {
                store,
                guest: ModelGuest { view: CAPACITY.into(), asks: asks.into() },
                shrinking: Shrinking::new(),
                going: false.into(),
            })
        }
    }

    impl Holder for Conn {
        fn going(&self) -> bool {
            self.going.load(Ordering::SeqCst)
        }
    }

    impl Resizes for Conn {
        async fn resize(&self, capacity: u64) -> Result<(), Errno> {
            let store = View::Live(self.store.clone());
            resize_held(&store, &self.guest, &self.shrinking, capacity, Duration::MAX).await
        }
    }

    /// Answers the shrinks `conn`'s guest is asked to take, as its guest does: taking those that
    /// leave it what it holds, and seeing its store so.
    fn answer(conn: Arc<Conn>, asks: mpsc::Receiver<u64>) {
        while let Ok(capacity) = asks.recv() {
            let took = capacity >= GUEST_HOLDS;
            if took {
                conn.guest.view.store(capacity, Ordering::SeqCst);
            }
            conn.shrinking.answered(capacity, took);
        }
    }

    /// Opens the store here, as its files hold it, keeping what holds it until it is closed.
    fn here<'a>(
        model: &Arc<Model>,
    ) -> impl FnOnce(Option<Released<'a, Conn>>) -> std::future::Ready<Result<View<'a>, Errno>>
    {
        let model = model.clone();
        move |held| {
            model.open();
            std::future::ready(Ok(View::Here(model, held)))
        }
    }

    fn run() {
        let key = Path::new(KEY);
        let paths = Arc::new(Paths::<Conn>::new());
        let model = Model::new();
        // The commits made, as the guest saw them return.
        let committed = Arc::new(AtomicU64::new(0));
        let (opened_tx, opened) = mpsc::channel::<()>();
        let (resized_tx, resized) = mpsc::channel::<()>();
        let (going_tx, going) = mpsc::channel::<()>();

        // The VM's connection: opens the store, commits, lets its guest answer shrinks, and
        // closes once the resizes are done, its commits durable first.
        let first = {
            let (paths, model, committed) = (paths.clone(), model.clone(), committed.clone());
            thread::spawn(move || {
                let (asks_tx, asks) = mpsc::channel();
                let conn = Conn::new(model.clone(), Some(asks_tx));
                paths.hold(key, Arc::downgrade(&conn), None).unwrap();
                model.open();
                let guest = {
                    let conn = conn.clone();
                    thread::spawn(move || answer(conn, asks))
                };
                opened_tx.send(()).unwrap();
                for _ in 0..2 {
                    let epoch = model.ordered.fetch_add(1, Ordering::SeqCst) + 1;
                    committed.store(epoch, Ordering::SeqCst);
                    thread::yield_now();
                }
                resized.recv().unwrap();
                model.opened_once_at_a_time();
                let capacity = model.capacity.load(Ordering::SeqCst);
                let view = conn.guest.view.load(Ordering::SeqCst);
                assert_eq!(capacity, view, "the store's capacity and its guest's view differ");
                // Its VM process gone: its store goes once its commits are durable.
                conn.going.store(true, Ordering::SeqCst);
                going_tx.send(()).unwrap();
                model.durable.store(model.ordered.load(Ordering::SeqCst), Ordering::SeqCst);
                conn.guest.asks.lock().unwrap().take();
                guest.join().unwrap();
                model.close();
                paths.release(key);
            })
        };

        // The VM's manager: resizes the store while the VM has it open: grows it, shrinks it,
        // which the guest agrees to, and shrinks it below what the guest holds, which it doesn't.
        let resizer = {
            let (paths, model) = (paths.clone(), model.clone());
            thread::spawn(move || {
                opened.recv().unwrap();
                for (capacity, expected) in [(12, Ok(())), (6, Ok(())), (3, Err(Errno::ENOSPC))] {
                    let resized = block_on(resize(&paths, key, capacity, here(&model)));
                    model.opened_once_at_a_time();
                    assert_eq!(resized, expected, "resizing to {capacity}");
                }
                resized_tx.send(()).unwrap();
            })
        };

        // A fork of the store, at any point: of the connection's store, live, or of the store as
        // its files hold it, once no connection has it open.
        let fork = {
            let (paths, model, committed) = (paths.clone(), model.clone(), committed.clone());
            thread::spawn(move || {
                let before = committed.load(Ordering::SeqCst);
                let live = |conn: &Conn| Some(View::Live(conn.store.clone()));
                let forking =
                    block_on(forking(&paths, key, live, Duration::MAX, here(&model))).unwrap();
                let forked = forking.forked();
                // Checked once closed: a task panicking unwinds through what it holds, whose
                // drops take Shuttle's locks, which fail in a failed run with panics that abort.
                block_on(forking.close());
                model.opened_once_at_a_time();
                assert!(forked >= before, "the fork misses a commit made before it");
            })
        };

        // The VM restored from a snapshot, opening its store as the first goes.
        let second = {
            let (paths, model) = (paths.clone(), model.clone());
            thread::spawn(move || {
                going.recv().unwrap();
                let conn = Conn::new(model.clone(), None);
                paths.hold(key, Arc::downgrade(&conn), None).unwrap();
                model.open();
                model.close();
                paths.release(key);
            })
        };
        for thread in [first, resizer, fork, second] {
            thread.join().unwrap();
        }
        model.opened_once_at_a_time();
        assert!(paths.holder(key).is_none(), "the store is held once all let it go");
    }

    fn randomly() {
        Runner::new(RandomScheduler::new(runs(2000)), config()).run(run);
    }

    /// A store opened here whose closing, its call cancelled, is left to a thread of its own, as
    /// `extentd::stores::Here`'s is to a blocking thread, with what holds it, which it lets go once
    /// the store is closed.
    struct ClosedBehind(Option<(Arc<Model>, Option<Released<'static, Conn>>)>);

    impl Close for ClosedBehind {
        async fn close(mut self) {
            if let Some((model, held)) = self.0.take() {
                model.close();
                drop(held);
            }
        }
    }

    impl Drop for ClosedBehind {
        fn drop(&mut self) {
            if let Some((model, held)) = self.0.take() {
                thread::spawn(move || {
                    model.close();
                    drop(held);
                });
            }
        }
    }

    /// TASK-79.34: a fork of the store opened here, its call cancelled once it has the store,
    /// whose closing is left to a thread of its own, lets the store's path go once it is closed:
    /// a connection opening it at once finds it closed.
    fn cancelled_fork() {
        let key = Path::new(KEY);
        // As the daemon's are, which outlive what the closing thread holds of them.
        let paths: &'static Paths<Conn> = Box::leak(Box::new(Paths::new()));
        let model = Model::new();
        let (forked_tx, forked) = mpsc::channel::<()>();
        let fork = {
            let model = model.clone();
            thread::spawn(move || {
                let here = move |held| {
                    model.open();
                    std::future::ready(Ok(ClosedBehind(Some((model, held)))))
                };
                let live = |_: &Conn| None;
                let forking = block_on(forking(paths, key, live, Duration::MAX, here)).unwrap();
                forked_tx.send(()).unwrap();
                // Its call cancelled.
                drop(forking);
            })
        };
        let opening = {
            let model = model.clone();
            thread::spawn(move || {
                forked.recv().unwrap();
                let conn = Conn::new(model.clone(), None);
                paths.hold(key, Arc::downgrade(&conn), None).unwrap();
                model.open();
                model.close();
                paths.release(key);
            })
        };
        fork.join().unwrap();
        opening.join().unwrap();
        model.opened_once_at_a_time();
    }

    #[test]
    fn cancelled_forks_let_their_paths_go_once_closed_randomly() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(RandomScheduler::new(runs(2000)), config()).run(cancelled_fork);
    }

    #[test]
    fn cancelled_forks_let_their_paths_go_once_closed_by_pct() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(PctScheduler::new(3, runs(2000)), config()).run(cancelled_fork);
    }

    /// Let go of as its call returns, a fork's store's path is opened as the store is still open.
    #[test]
    fn needs_forks_to_hold_their_paths() {
        *RULES.lock().unwrap() = Rules { forks_hold_their_paths: false, ..Rules::ALL };
        fails_with("open twice at once", || {
            Runner::new(RandomScheduler::new(runs(2000)), config()).run(cancelled_fork);
        });
    }

    /// TASK-79.41: two resizes at once of a store a connection holds, a grow and a shrink its
    /// guest agrees to, both through the connection, end with the store's capacity and its guest's
    /// view agreeing.
    fn resizes_at_once() {
        let key = Path::new(KEY);
        let paths = Arc::new(Paths::<Conn>::new());
        let model = Model::new();
        let (asks_tx, asks) = mpsc::channel();
        let conn = Conn::new(model.clone(), Some(asks_tx));
        paths.hold(key, Arc::downgrade(&conn), None).unwrap();
        let guest = {
            let conn = conn.clone();
            thread::spawn(move || answer(conn, asks))
        };
        let resizers: Vec<_> = [12, 6]
            .into_iter()
            .map(|capacity| {
                let (paths, model) = (paths.clone(), model.clone());
                thread::spawn(move || block_on(resize(&paths, key, capacity, here(&model))))
            })
            .collect();
        for resizer in resizers {
            assert_eq!(resizer.join().unwrap(), Ok(()));
        }
        conn.guest.asks.lock().unwrap().take();
        guest.join().unwrap();
        let capacity = model.capacity.load(Ordering::SeqCst);
        let view = conn.guest.view.load(Ordering::SeqCst);
        assert_eq!(capacity, view, "the store's capacity and its guest's view differ");
        paths.release(key);
    }

    #[test]
    fn resizes_at_once_agree_randomly() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(RandomScheduler::new(runs(2000)), config()).run(resizes_at_once);
    }

    #[test]
    fn resizes_at_once_agree_by_pct() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(PctScheduler::new(3, runs(2000)), config()).run(resizes_at_once);
    }

    /// A grow landing between a shrink's answer and its new capacity leaves the store smaller than
    /// its guest was told.
    #[test]
    fn needs_resizes_one_at_a_time() {
        *RULES.lock().unwrap() = Rules { resizes_one_at_a_time: false, ..Rules::ALL };
        fails_with("the store's capacity and its guest's view differ", || {
            Runner::new(RandomScheduler::new(runs(2000)), config()).run(resizes_at_once);
        });
    }

    #[test]
    fn requests_race_opens_and_closes_randomly() {
        *RULES.lock().unwrap() = Rules::ALL;
        randomly();
    }

    #[test]
    fn requests_race_opens_and_closes_by_pct() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(PctScheduler::new(3, runs(2000)), config()).run(run);
    }

    /// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`.
    fn fails_without(rules: Rules, expected: &str) {
        *RULES.lock().unwrap() = rules;
        fails_with(expected, randomly);
    }

    /// Resized here while a connection has it open, the store is open twice.
    #[test]
    fn needs_resizes_through_the_holder() {
        fails_without(Rules { through_holder: false, ..Rules::ALL }, "open twice at once");
    }

    #[test]
    fn needs_the_guest_to_agree() {
        fails_without(Rules { guest_agrees: false, ..Rules::ALL }, "shrunk without agreeing");
    }

    /// Forked here while a connection has it open, the store is open twice; which would miss
    /// the commits the connection didn't make durable yet.
    #[test]
    fn needs_forks_through_the_holder() {
        fails_without(Rules { fork_through_holder: false, ..Rules::ALL }, "open twice at once");
    }

    /// The store is open once at a time: as open_stores.rs's Shuttle tests show, here through the
    /// requests.
    #[test]
    fn needs_one_holder() {
        *RULES.lock().unwrap() = Rules::ALL;
        crate::open_stores::set_rules(crate::open_stores::Rules {
            one_holder: false,
            ..crate::open_stores::Rules::ALL
        });
        fails_with("open twice at once", randomly);
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;

    /// A store of 8 bytes, holding 1.
    struct Store;

    impl Close for Store {
        async fn close(self) {}
    }

    impl Capacity for Store {
        fn capacity(&self) -> Result<(u64, u64), Errno> {
            Ok((8, 1))
        }

        async fn set_capacity(&self, _capacity: u64) -> Result<(), Errno> {
            panic!("the guest never took the shrink")
        }
    }

    /// A guest that answers nothing, and what it was told.
    #[derive(Default)]
    struct Silent(std::sync::Mutex<Vec<(u64, bool)>>);

    impl Guest for Silent {
        fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
            self.0.lock().unwrap().push((capacity, ask));
            Ok(())
        }
    }

    /// TASK-79.41: a shrink its guest leaves unanswered tells the guest the store's capacity as it
    /// stays, as an answer coming after, the guest having taken the shrink, shrinks nothing.
    #[test]
    fn shrinks_unanswered_tell_the_guest_the_capacity_kept() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (guest, shrinking) = (Silent::default(), Shrinking::new());
        let wait = Duration::from_millis(10);
        let resized = runtime.block_on(resize_held(&Store, &guest, &shrinking, 6, wait));
        assert_eq!(resized, Err(Errno::ETIMEDOUT));
        assert_eq!(*guest.0.lock().unwrap(), [(6, true), (8, false)]);
    }
}
