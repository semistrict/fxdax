// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The stores the extent daemon's connections hold (`crate::extentd`), by their paths: a store has
//! one holder at a time, a connection that has it open, or the daemon itself while it forks,
//! syncs or resizes a store no connection has. A connection whose VM process is gone, still
//! holding its store, lets it go to the next that asks, as a VM restored from a snapshot at once
//! opens its VM's store (TASK-39.1). fxdax: a connection following a store, read-only, holds none
//! (backlog doc-8): the daemon holds it while one opens it for followers.
//!
//! Generic over its holders, so that its tests run it with holders of their own: under Shuttle
//! (awslabs/shuttle), with the `shuttle` feature, whose scheduler chooses every interleaving of
//! the threads holding, waiting for and releasing stores (TASK-44.6).

use crate::Errno;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "shuttle")]
use shuttle::sync::{Condvar, Mutex};
#[cfg(not(feature = "shuttle"))]
use std::sync::{Condvar, Mutex};

/// The rules the registry relies on, each of which its Shuttle tests show to be needed by turning
/// it off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A store held by a holder that isn't going holds no other.
    pub one_holder: bool,
    /// Letting a store go wakes those waiting to hold it.
    pub release_wakes: bool,
}

impl Rules {
    pub const ALL: Self = Self { one_holder: true, release_wakes: true };
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

/// Sets the rules, for the Shuttle tests of what uses the registry.
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

/// What holds a store: a connection.
pub(crate) trait Holder: Send + Sync {
    /// Whether it is going: its VM process gone, it lets its store go to the next that asks.
    fn going(&self) -> bool;
}

/// The stores held, by the paths that name them (`server::store_key`). A holder of none is the
/// daemon's own, which goes once its work is done.
pub(crate) struct Paths<H> {
    held: Mutex<HashMap<PathBuf, Weak<H>>>,
    /// Notified as a store's holder lets it go.
    released: Condvar,
    /// Likewise, for what waits to hold a store without a thread ([`Paths::hold_async`]).
    released_async: event_listener::Event,
}

impl<H: Holder> Paths<H> {
    pub fn new() -> Self {
        Self {
            held: Mutex::new(HashMap::new()),
            released: Condvar::new(),
            released_async: event_listener::Event::new(),
        }
    }

    /// Has `holder` hold the store `key` names: once its holder, if it has one, lets it go, if
    /// that holder is going, or is the daemon's, waiting at most `wait` (for ever, if none);
    /// unless a holder that isn't going has it, which fails with `EBUSY`.
    pub fn hold(&self, key: &Path, holder: Weak<H>, wait: Option<Duration>) -> Result<(), Errno> {
        let deadline = wait.map(|wait| Instant::now() + wait);
        let mut held = self.held.lock().unwrap();
        while let Some(other) = held.get(key).filter(|_| rules().one_holder) {
            let going = other.upgrade().is_none_or(|other| other.going());
            if !going {
                return Err(Errno::EBUSY);
            }
            held = match deadline {
                None => self.released.wait(held).unwrap(),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Errno::EBUSY);
                    }
                    self.released.wait_timeout(held, left).unwrap().0
                }
            };
        }
        held.insert(key.to_owned(), holder);
        Ok(())
    }

    /// Has `holder` hold the store `key` names, as [`Paths::hold`] does, waiting at most `wait`: a
    /// future, which holds no thread while it waits (TASK-43.6).
    pub async fn hold_async(
        &self,
        key: &Path,
        holder: Weak<H>,
        wait: Duration,
    ) -> Result<(), Errno> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            let released = {
                let mut held = self.held.lock().unwrap();
                let Some(other) = held.get(key).filter(|_| rules().one_holder) else {
                    held.insert(key.to_owned(), holder);
                    return Ok(());
                };
                if !other.upgrade().is_none_or(|other| other.going()) {
                    return Err(Errno::EBUSY);
                }
                self.released_async.listen()
            };
            self.released_within(deadline, released).await?;
        }
    }

    /// The connection holding the store `key` names, if one does that isn't going; or, if none
    /// does, the daemon holds it, until [`release`](Self::release): once a holder that is going,
    /// or the daemon's, lets it go, waiting at most `wait` (`EBUSY`), holding no thread meanwhile.
    pub async fn holder_or_hold_async(
        &self,
        key: &Path,
        wait: Duration,
    ) -> Result<Option<Arc<H>>, Errno> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            let released = {
                let mut held = self.held.lock().unwrap();
                let Some(other) = held.get(key) else {
                    held.insert(key.to_owned(), Weak::new());
                    return Ok(None);
                };
                if let Some(holder) = other.upgrade().filter(|holder| !holder.going()) {
                    return Ok(Some(holder));
                }
                self.released_async.listen()
            };
            self.released_within(deadline, released).await?;
        }
    }

    /// Waits for `released`, a store let go, until `deadline` (for ever, if none): `EBUSY` once
    /// it passes.
    async fn released_within(
        &self,
        deadline: Option<Instant>,
        released: event_listener::EventListener,
    ) -> Result<(), Errno> {
        let Some(deadline) = deadline else {
            released.await;
            return Ok(());
        };
        let left = deadline.saturating_duration_since(Instant::now());
        within(left, released).await.ok_or(Errno::EBUSY)
    }

    /// Lets the store `key` names go, which the next waiting to hold it then may.
    pub fn release(&self, key: &Path) {
        self.held.lock().unwrap().remove(key);
        if rules().release_wakes {
            self.released.notify_all();
            self.released_async.notify(usize::MAX);
        }
    }

    /// How many stores holders hold: not the daemon's.
    pub fn held_by_holders(&self) -> usize {
        self.held.lock().unwrap().values().filter(|holder| holder.upgrade().is_some()).count()
    }

    /// The connection holding the store `key` names, if one does.
    pub fn holder(&self, key: &Path) -> Option<Arc<H>> {
        self.held.lock().unwrap().get(key).and_then(Weak::upgrade)
    }

    /// The connection holding the store `key` names, if one does; or, if none does, the daemon
    /// holds it, until [`release`](Self::release). `EBUSY` if a holder of it is going, or the
    /// daemon's.
    pub fn holder_or_hold(&self, key: &Path) -> Result<Option<Arc<H>>, Errno> {
        let mut held = self.held.lock().unwrap();
        match held.get(key) {
            Some(holder) => Ok(Some(holder.upgrade().ok_or(Errno::EBUSY)?)),
            None => {
                held.insert(key.to_owned(), Weak::new());
                Ok(None)
            }
        }
    }
}

/// The registry under Shuttle: connections opening a store, one whose VM process goes, and a
/// resize of it while no connection holds it, every interleaving of them Shuttle's scheduler
/// chooses; waits are unbounded, so that a holder that never wakes those waiting is a deadlock,
/// which Shuttle reports, rather than a wait of `CLOSING_WAIT`. Each rule turned off fails.
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests {
    use super::*;
    use crate::shuttle_testing::{config, fails_with, runs};
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use shuttle::{Runner, thread};

    /// A connection: going once its VM process is.
    struct Conn(AtomicBool);

    impl Holder for Conn {
        fn going(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Holds the store as `holder`, checking no other holds it meanwhile, then lets it go.
    fn holding(paths: &Paths<Conn>, held: &AtomicUsize, then: impl FnOnce()) {
        let others = held.fetch_add(1, Ordering::SeqCst);
        assert_eq!(others, 0, "two holders hold the store at once");
        thread::yield_now();
        then();
        held.fetch_sub(1, Ordering::SeqCst);
        paths.release(Path::new("/store"));
    }

    fn run() {
        let key = Path::new("/store");
        let paths = Arc::new(Paths::<Conn>::new());
        let held = Arc::new(AtomicUsize::new(0));

        // A VM's connection, whose VM process goes while it holds the store.
        let first = {
            let (paths, held) = (paths.clone(), held.clone());
            thread::spawn(move || {
                let conn = Arc::new(Conn(AtomicBool::new(false)));
                if paths.hold(key, Arc::downgrade(&conn), None).is_ok() {
                    holding(&paths, &held, || conn.0.store(true, Ordering::SeqCst));
                }
            })
        };
        // The VM restored from its snapshot, opening the store at once: it gets it once the
        // first lets it go, or EBUSY while the first's VM process is there.
        let second = {
            let (paths, held) = (paths.clone(), held.clone());
            thread::spawn(move || {
                let conn = Arc::new(Conn(AtomicBool::new(false)));
                if paths.hold(key, Arc::downgrade(&conn), None).is_ok() {
                    holding(&paths, &held, || {});
                }
            })
        };
        // A resize of the store, which the daemon holds while no connection does.
        let resize = {
            let (paths, held) = (paths.clone(), held.clone());
            thread::spawn(move || {
                if let Ok(None) = paths.holder_or_hold(key) {
                    holding(&paths, &held, || {});
                }
            })
        };
        first.join().unwrap();
        second.join().unwrap();
        resize.join().unwrap();
        assert!(paths.holder(key).is_none(), "the store is held after all let it go");
    }

    fn randomly() {
        Runner::new(RandomScheduler::new(runs(3000)), config()).run(run);
    }

    #[test]
    fn opens_going_holders_and_resizes_randomly() {
        *RULES.lock().unwrap() = Rules::ALL;
        randomly();
    }

    #[test]
    fn opens_going_holders_and_resizes_by_pct() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(PctScheduler::new(3, runs(3000)), config()).run(run);
    }

    /// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`.
    fn fails_without(rules: Rules, expected: &str) {
        *RULES.lock().unwrap() = rules;
        fails_with(expected, randomly);
    }

    #[test]
    fn needs_one_holder() {
        fails_without(Rules { one_holder: false, ..Rules::ALL }, "two holders hold the store");
    }

    /// The resize that let the store go without waking those waiting for it (fixed by having it
    /// release through `release`): they waited `CLOSING_WAIT` out.
    #[test]
    fn needs_releases_to_wake() {
        fails_without(Rules { release_wakes: false, ..Rules::ALL }, "deadlock!");
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Connection(AtomicBool);

    impl Holder for Connection {
        fn going(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn connection(going: bool) -> Arc<Connection> {
        Arc::new(Connection(AtomicBool::new(going)))
    }

    /// One waiting for a store a going holder never lets go gives up once its wait is over.
    #[test]
    fn waits_for_stores_never_let_go_end() {
        let paths = Paths::new();
        let (going, next) = (connection(true), connection(false));
        let key = Path::new("store");
        paths.hold(key, Arc::downgrade(&going), None).unwrap();
        let wait = Duration::from_millis(10);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let held = runtime.block_on(paths.hold_async(key, Arc::downgrade(&next), wait));
        assert_eq!(held, Err(Errno::EBUSY));
        let held = runtime.block_on(paths.holder_or_hold_async(key, wait));
        assert_eq!(held.err(), Some(Errno::EBUSY));
    }

    /// The stores holders hold are counted, not the daemon's, nor those of holders gone.
    #[test]
    fn stores_held_by_holders_are_counted() {
        let paths = Paths::new();
        let (first, second) = (connection(false), connection(false));
        paths.hold(Path::new("first"), Arc::downgrade(&first), None).unwrap();
        paths.hold(Path::new("second"), Arc::downgrade(&second), None).unwrap();
        paths.hold(Path::new("daemon's"), Weak::new(), None).unwrap();
        assert_eq!(paths.held_by_holders(), 2);
        drop(second);
        assert_eq!(paths.held_by_holders(), 1);
    }
}
