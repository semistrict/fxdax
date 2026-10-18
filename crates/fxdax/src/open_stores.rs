// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The stores the extent daemon's connections hold (`crate::extentd`), by their paths: a store has
//! one holder at a time, a connection that has it open, or the daemon itself while it forks,
//! syncs or resizes a store no connection has. A connection whose VM process is gone, still
//! holding its store, lets it go to the next that asks, as a VM restored from a snapshot at once
//! opens its VM's store (TASK-39.1).
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
}

impl<H: Holder> Paths<H> {
    pub fn new() -> Self {
        Self { held: Mutex::new(HashMap::new()), released: Condvar::new() }
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

    /// Lets the store `key` names go, which the next waiting to hold it then may.
    pub fn release(&self, key: &Path) {
        self.held.lock().unwrap().remove(key);
        if rules().release_wakes {
            self.released.notify_all();
        }
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
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use shuttle::{Config, Runner, thread};

    /// The schedules each test runs: some seconds; `SHUTTLE_SOAK` multiplies them.
    fn runs() -> usize {
        let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
        3000 * soak
    }

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

    fn config() -> Config {
        let mut config = Config::new();
        config.stack_size = 1 << 20;
        config
    }

    fn randomly() {
        Runner::new(RandomScheduler::new(runs()), config()).run(run);
    }

    #[test]
    fn opens_going_holders_and_resizes_randomly() {
        *RULES.lock().unwrap() = Rules::ALL;
        randomly();
    }

    #[test]
    fn opens_going_holders_and_resizes_by_pct() {
        *RULES.lock().unwrap() = Rules::ALL;
        Runner::new(PctScheduler::new(3, runs()), config()).run(run);
    }

    /// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`.
    /// The panics are collected by a hook installed for good, which Shuttle's chains to (a failing
    /// run may leave the thread counted as panicking, when the hook can't be set back); each test
    /// is a process of its own (nextest).
    fn fails_without(rules: Rules, expected: &str) {
        static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        std::panic::set_hook(Box::new(|info| {
            PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
        }));
        *RULES.lock().unwrap() = rules;
        let failed = match std::panic::catch_unwind(randomly) {
            Ok(()) => false,
            // Its payload may hold Shuttle's objects, which can't be dropped outside a run.
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        };
        *RULES.lock().unwrap() = Rules::ALL;
        let panics = std::mem::take(&mut *PANICS.lock().unwrap());
        assert!(failed, "no schedule failed with {rules:?}");
        assert!(
            panics.iter().any(|panic| panic.contains(expected)),
            "with {rules:?}, expected {expected:?}, got {panics:#?}"
        );
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
