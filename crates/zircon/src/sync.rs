// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: what the kernel waits with beyond its locks. Its locks are parking_lot's, or, with
//! the `shuttle` feature, Shuttle's behind parking_lot's API (shuttle-parking_lot), which has no
//! condition variable: so the kernel's is this module's, parking_lot's or one over Shuttle's
//! primitives (TASK-44.3).

#[cfg(not(feature = "shuttle"))]
pub(crate) use parking_lot::Condvar;

/// Starts the kernel's thread `name`, running `f`: one of its services that act as time passes,
/// which are timers and their wakeups, the scanner, the page queues' aging, eviction and the
/// memory watchdog. Under Shuttle time doesn't pass, as its timed waits don't time out, and none
/// starts: the kernel runs without them, and its tests' threads are Shuttle's alone.
pub(crate) fn spawn<F, T>(name: &str, f: F) -> Option<std::thread::JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if cfg!(feature = "shuttle") {
        return None;
    }
    let thread = std::thread::Builder::new().name(name.into()).spawn(f);
    Some(thread.unwrap_or_else(|err| panic!("create the {name} thread: {err}")))
}

#[cfg(feature = "shuttle")]
pub(crate) use shuttle_condvar::Condvar;

#[cfg(feature = "shuttle")]
mod shuttle_condvar {
    use parking_lot::MutexGuard;
    use std::time::Duration;

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

        /// As [`wait`](Self::wait): Shuttle's waits don't time out, so that a test whose threads
        /// all wait, some for a time, is reported as a deadlock.
        pub fn wait_for<T>(&self, guard: &mut MutexGuard<'_, T>, _timeout: Duration) {
            self.wait(guard);
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
