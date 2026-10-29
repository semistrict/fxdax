// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What physical memory synchronizes with beyond its locks. Its locks are parking_lot's, or, with
//! the `shuttle` feature, Shuttle's behind parking_lot's API (shuttle-parking_lot), which has no
//! condition variable: so its condition variable is this module's, parking_lot's or one over
//! Shuttle's primitives (TASK-44.3); and so are the waits made of them, [`Pulse`] and [`Signal`].
//!
//! And the rules its concurrency relies on ([`Rules`]), which its Shuttle tests turn off one at a
//! time to see a test fail: a test that never fails proves nothing.

#[cfg(not(feature = "shuttle"))]
pub(crate) use parking_lot::Condvar;
use parking_lot::Mutex;

/// The rules physical memory's concurrency relies on, each of which a Shuttle test shows to be
/// needed by turning it off (`tests::shuttle`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// An allocation that chunks going back would make room for, as the host takes them back
    /// outside the PMM's lock, waits for them (`Arenas::take`, TASK-92.35).
    pub allocations_wait_for_chunks_going_back: bool,
    /// A page a second stage mapped since it was allocated, freed, waits for every second stage
    /// to have made the changes taking it away before it is free (`quarantine_with`, TASK-79.9).
    pub staged_pages_wait_for_their_stages: bool,
}

impl Rules {
    pub(crate) const ALL: Self = Self {
        allocations_wait_for_chunks_going_back: true,
        staged_pages_wait_for_their_stages: true,
    };
}

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(not(feature = "shuttle"))]
pub(crate) fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(feature = "shuttle")]
pub(crate) fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules in force, for the Shuttle tests that turn one off.
#[cfg(all(test, feature = "shuttle"))]
pub(crate) fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}

/// Starts physical memory's thread `name`, running `f`: the arenas' relief, which acts as
/// allocations ask. Under Shuttle none starts, as Shuttle's threads are its tests' alone. Fails as
/// the host fails to start it.
pub(crate) fn try_spawn<F, T>(
    name: &str,
    f: F,
) -> std::io::Result<Option<std::thread::JoinHandle<T>>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if cfg!(feature = "shuttle") {
        return Ok(None);
    }
    std::thread::Builder::new().name(name.into()).spawn(f).map(Some)
}

/// Wakes every waiter as it pulses: a waiter that read the count of pulses ([`Pulse::count`])
/// before what it waits for could change waits for the next ([`Pulse::wait_past`]), and misses
/// none.
pub(crate) struct Pulse {
    count: Mutex<u64>,
    cond: Condvar,
}

impl Pulse {
    pub(crate) fn new() -> Self {
        Self { count: Mutex::new(0), cond: Condvar::new() }
    }

    /// The pulses so far, for [`Pulse::wait_past`].
    pub(crate) fn count(&self) -> u64 {
        *self.count.lock()
    }

    /// Wakes every waiter.
    pub(crate) fn pulse(&self) {
        let mut count = self.count.lock();
        *count += 1;
        self.cond.notify_all();
    }

    /// Waits for a pulse after the `count`th.
    pub(crate) fn wait_past(&self, count: u64) {
        let mut now = self.count.lock();
        while *now == count {
            self.cond.wait(&mut now);
        }
    }
}

/// An event that each signal releases one wait of, after which it is unsignaled again.
pub(crate) struct Signal {
    signaled: Mutex<bool>,
    cond: Condvar,
}

impl Signal {
    pub(crate) fn new() -> Self {
        Self { signaled: Mutex::new(false), cond: Condvar::new() }
    }

    /// Signals it, releasing a wait, now or the next.
    pub(crate) fn signal(&self) {
        *self.signaled.lock() = true;
        self.cond.notify_one();
    }

    /// Waits for it to be signaled, and unsignals it.
    pub(crate) fn wait(&self) {
        let mut signaled = self.signaled.lock();
        while !*signaled {
            self.cond.wait(&mut signaled);
        }
        *signaled = false;
    }
}

#[cfg(feature = "shuttle")]
pub(crate) use shuttle_condvar::Condvar;

#[cfg(feature = "shuttle")]
mod shuttle_condvar {
    use parking_lot::MutexGuard;

    /// A condition variable over parking_lot's guards, on Shuttle's primitives. A wait holds the
    /// generation's lock from before it lets the guard go until it sleeps, so that no notification
    /// between the two is lost; a notification ends every wait that began before it, which, for
    /// waits [`notify_one`](Self::notify_one) doesn't wake, is when they next wake: spuriously, as
    /// a condition variable's waits may.
    #[derive(Default)]
    pub(crate) struct Condvar {
        generation: shuttle::sync::Mutex<u64>,
        changed: shuttle::sync::Condvar,
    }

    impl Condvar {
        pub const fn new() -> Self {
            Self {
                generation: shuttle::sync::Mutex::new(0),
                changed: shuttle::sync::Condvar::new(),
            }
        }

        pub fn wait<T>(&self, guard: &mut MutexGuard<'_, T>) {
            let generation = self.generation.lock().unwrap();
            let seen = *generation;
            MutexGuard::unlocked(guard, move || {
                drop(self.changed.wait_while(generation, |now| *now == seen).unwrap());
            });
        }

        pub fn notify_one(&self) {
            *self.generation.lock().unwrap() += 1;
            self.changed.notify_one();
        }

        pub fn notify_all(&self) {
            *self.generation.lock().unwrap() += 1;
            self.changed.notify_all();
        }
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A wait for a pulse past the count read before it ends with the pulse, however late it
    /// starts: a pulse after the count is never missed.
    #[test]
    fn pulses_past_the_count_read_end_waits() {
        let pulse = Arc::new(Pulse::new());
        let count = pulse.count();
        pulse.pulse();
        pulse.wait_past(count);
        let count = pulse.count();
        let waiter = std::thread::spawn({
            let pulse = pulse.clone();
            move || pulse.wait_past(count)
        });
        pulse.pulse();
        waiter.join().unwrap();
        assert_eq!(pulse.count(), count + 1);
    }

    /// A signal releases one wait, then or later, and is unsignaled by it.
    #[test]
    fn signals_release_one_wait() {
        let signal = Arc::new(Signal::new());
        signal.signal();
        signal.wait();
        assert!(!*signal.signaled.lock(), "unsignaled by the wait");
        let waiter = std::thread::spawn({
            let signal = signal.clone();
            move || signal.wait()
        });
        signal.signal();
        waiter.join().unwrap();
        assert!(!*signal.signaled.lock());
    }
}
