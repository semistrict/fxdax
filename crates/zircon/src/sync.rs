// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: what the kernel synchronizes with beyond its locks. Its locks are parking_lot's,
//! or, with the `shuttle` feature, Shuttle's behind parking_lot's API (shuttle-parking_lot), which
//! has no condition variable: so the kernel's is this module's, parking_lot's or one over
//! Shuttle's primitives (TASK-44.3).
//!
//! State a lock guards that is not inside it, as Zircon's fields guarded by another object's lock
//! (`TA_GUARDED`) are, is a [`LockedBy`], which only the lock's holder reaches: what Zircon's
//! thread safety analysis checks as it compiles, this checks as it runs.

#[cfg(not(feature = "shuttle"))]
pub(crate) use parking_lot::Condvar;
use parking_lot::Mutex;
use std::cell::UnsafeCell;

/// A lock as the state it guards outside it knows it ([`LockedBy`]): by the address of the state
/// inside it, which only the lock's holder can borrow.
pub(crate) struct LockId<L>(*const L);

impl<L> LockId<L> {
    pub(crate) fn of(lock: &Mutex<L>) -> Self {
        // Values of a zero-sized type may all be at one address, which would tell no lock from
        // another.
        const { assert!(size_of::<L>() != 0, "a lock guarding no state") };
        Self(lock.data_ptr())
    }

    /// Asserts that `locked` is the state inside this lock, so that whoever lends it holds it.
    fn check(&self, locked: &L) {
        assert!(std::ptr::eq(self.0, locked), "guarded by another lock");
    }
}

impl<L> Clone for LockId<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for LockId<L> {}

/// A value a lock guards outside it, as Zircon's fields guarded by another object's lock are: only
/// the lock's holder reaches it, by the state inside the lock, which it borrows shared to read the
/// value and exclusively to change it.
pub(crate) struct LockedBy<T, L> {
    lock: LockId<L>,
    value: UnsafeCell<T>,
}

// SAFETY: The value is the cell's, which hands it out only to the lock's holder, as a `Mutex`
// hands out what it holds.
unsafe impl<T: Send, L> Send for LockedBy<T, L> {}
// SAFETY: Only the lock's holder reaches the value: shared, by as many as it shares the lock's
// state with, or exclusively, while it borrows the lock's state exclusively.
unsafe impl<T: Send + Sync, L> Sync for LockedBy<T, L> {}

impl<T, L> LockedBy<T, L> {
    /// `value`, which `lock` guards.
    pub(crate) fn new(lock: LockId<L>, value: T) -> Self {
        Self { lock, value: UnsafeCell::new(value) }
    }

    /// The value, to the lock's holder, whose state inside the lock `locked` is.
    pub(crate) fn get<'a>(&'a self, locked: &'a L) -> &'a T {
        self.lock.check(locked);
        // SAFETY: Whoever lends `locked` holds the lock, and while it is lent shared, neither it nor
        // anyone else borrows the value exclusively.
        unsafe { &*self.value.get() }
    }

    /// The value, to change, to the lock's holder, whose state inside the lock `locked` is.
    pub(crate) fn get_mut<'a>(&'a self, locked: &'a mut L) -> &'a mut T {
        self.lock.check(locked);
        // SAFETY: Whoever lends `locked` holds the lock, and while it is lent exclusively, no one
        // else borrows the value.
        unsafe { &mut *self.value.get() }
    }
}

/// Starts the kernel's thread `name`, running `f`: one of its services that act as time passes,
/// which are timers and their wakeups, the scanner, the page queues' aging, eviction and the
/// memory watchdog, or as allocations ask, which is the arenas' relief. Under Shuttle time doesn't
/// pass, as its timed waits don't time out, and none starts: the kernel runs without them, and its
/// tests' threads are Shuttle's alone. Fails as the host fails to start it.
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

/// Starts the kernel's thread `name`, running `f`, as [`try_spawn`] does: one the kernel cannot
/// run without.
pub(crate) fn spawn<F, T>(name: &str, f: F) -> Option<std::thread::JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    try_spawn(name, f).unwrap_or_else(|err| panic!("create the {name} thread: {err}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A value is reached with the state of the lock guarding it: shared to read it, exclusively
    /// to change it.
    #[test]
    fn locked_values_are_reached_with_their_locks() {
        let lock = Mutex::new(0u32);
        let value = LockedBy::new(LockId::of(&lock), 5);
        assert_eq!(*value.get(&lock.lock()), 5);
        *value.get_mut(&mut lock.lock()) = 6;
        assert_eq!(*value.get(&lock.lock()), 6);
    }

    /// Another lock's state reaches nothing.
    #[test]
    #[should_panic(expected = "guarded by another lock")]
    fn locked_values_are_not_reached_with_another_lock() {
        let (lock, other) = (Mutex::new(0u32), Mutex::new(0u32));
        let value = LockedBy::new(LockId::of(&lock), 5);
        value.get(&other.lock());
    }
}
