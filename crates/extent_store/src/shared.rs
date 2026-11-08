// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: locks shared beyond one store, whose poison no fence contains (TASK-149.24). A store's
//! own locks fence the store a panic holding one went through (fxdax's `fence`): it fails each
//! request after with `EIO`, the others serve on. A lock shared beyond it, as the process's cache
//! of extents, a store's state, which its followers share with its writer, or a window's, which its
//! connection and the guest's faults share, would be found poisoned by every user after, one panic
//! each, or read half changed. So the extent daemon stops instead: a panic on a thread holding one
//! of these aborts the process, at the panic, before anything unwinds, once [`abort_on_panics`]
//! asks, as the daemon does as it starts. Its VMs' stores open anew as of their last commits, as
//! after a crash, once systemd has it running again.
//!
//! Such a lock is a [`SharedLock`], locked only through it ([`SharedLock::with`]), which has its
//! guard a [`Shared`]: the thread's locks held are what the panic hook looks at. Elsewhere, as in
//! tests, a panic holding one unwinds as any panic does.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::{LockResult, Once};

thread_local! {
    /// The shared locks this thread holds, by name, as it took them.
    static HELD: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// The shared locks the calling thread holds, by name.
pub fn held() -> Vec<&'static str> {
    HELD.with(|held| held.borrow().clone())
}

/// What the process says as it aborts, before the locks held, in parentheses, by name.
pub const ABORTING: &str = "a panic holding the process's shared locks";

/// Whether `said`, a process's output, says it aborted for a panic holding `lock`, among others.
pub fn aborted_holding(said: &str, lock: &str) -> bool {
    said.lines().any(|line| {
        line.split_once(&format!("{ABORTING} ("))
            .and_then(|(_, locks)| locks.strip_suffix("): aborting"))
            .is_some_and(|locks| locks.split(", ").any(|held| held == lock))
    })
}

/// Has a panic on a thread holding a shared lock abort the process, once the panic is said, as the
/// hook before this one says it: the process's log names the locks, and nothing unwinds past them.
/// Once a process; the extent daemon asks as it starts.
pub fn abort_on_panics() {
    static HOOKED: Once = Once::new();
    HOOKED.call_once(|| {
        let said = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let held = held();
            said(info);
            if !held.is_empty() {
                let locks = held.join(", ");
                log::error!(
                    "fxdax: {ABORTING} ({locks}), shared beyond one store: aborting, as no fence \
                     contains it (TASK-149.24)"
                );
                // As the log may not reach stderr, nor be flushed before the abort.
                eprintln!("fxdax: {ABORTING} ({locks}): aborting");
                std::process::abort();
            }
        }));
    });
}

/// A lock shared beyond one store (see the module docs): `L`, a mutex, locked only through
/// [`SharedLock::with`].
pub struct SharedLock<L> {
    /// What the log calls it.
    name: &'static str,
    lock: L,
}

impl<L> SharedLock<L> {
    pub const fn new(name: &'static str, lock: L) -> Self {
        Self { name, lock }
    }

    /// The lock itself, which `&mut` has alone: as a drop takes what it guards, locking nothing.
    pub fn get_mut(&mut self) -> &mut L {
        &mut self.lock
    }

    /// The lock, locked as `lock` locks it, such as `Mutex::lock`: held, it is the thread's. A lock
    /// found poisoned, which none is in the extent daemon, panics.
    pub fn with<'a, G>(&'a self, lock: impl FnOnce(&'a L) -> LockResult<G>) -> Shared<G> {
        let name = self.name;
        let guard = lock(&self.lock).unwrap_or_else(|_| {
            panic!("the process's {name} lock, shared beyond one store, is poisoned")
        });
        Shared::new(name, guard)
    }
}

/// The guard of a [`SharedLock`], `G`, held by the thread that locked it: not sent to another.
pub struct Shared<G> {
    /// None only as it is let go of ([`Shared::unlocked`]).
    guard: Option<G>,
    name: &'static str,
    _thread: PhantomData<*const ()>,
}

impl<G> Shared<G> {
    fn new(name: &'static str, guard: G) -> Self {
        HELD.with(|held| held.borrow_mut().push(name));
        let shared = Self { guard: Some(guard), name, _thread: PhantomData };
        #[cfg(any(test, feature = "testing"))]
        testing::panic_if_asked(name);
        shared
    }

    /// Lets the lock go while `unlocked` runs with its guard, as a condition variable's wait does,
    /// returning it, and holds it again.
    pub fn unlocked(mut self, unlocked: impl FnOnce(G) -> G) -> Self {
        let guard = self.guard.take().expect("the lock is held");
        let name = self.name;
        drop(self);
        Self::new(name, unlocked(guard))
    }
}

/// What the lock guards, as its guard has it.
impl<G: Deref> Deref for Shared<G> {
    type Target = G::Target;

    fn deref(&self) -> &G::Target {
        self.guard.as_ref().expect("the lock is held")
    }
}

impl<G: DerefMut> DerefMut for Shared<G> {
    fn deref_mut(&mut self) -> &mut G::Target {
        self.guard.as_mut().expect("the lock is held")
    }
}

impl<G> Drop for Shared<G> {
    fn drop(&mut self) {
        // The guard first: the lock is let go of before the thread says it holds it no more.
        drop(self.guard.take());
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|name| *name == self.name) {
                held.remove(at);
            }
        });
    }
}

/// Tests' panics holding a shared lock.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use std::sync::Mutex;

    /// The shared lock a test asks to be panicked holding, once: as it is next taken.
    static PANIC_HOLDING: Mutex<Option<&'static str>> = Mutex::new(None);

    /// Panics holding the shared lock called `lock` as it is next taken, by any thread: a bug's
    /// panic under it.
    pub fn panic_holding(lock: &'static str) {
        *PANIC_HOLDING.lock().unwrap() = Some(lock);
    }

    pub(super) fn panic_if_asked(lock: &'static str) {
        let mut asked = PANIC_HOLDING.lock().unwrap();
        if *asked == Some(lock) {
            *asked = None;
            drop(asked);
            panic!("a panic holding the process's {lock} lock, as a test asks");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::{Condvar, Mutex};

    /// What [`child_panics`] does, as its parent says: `holding`, panic holding a shared lock;
    /// `let-go`, panic having let it go.
    const CHILD: &str = "EXTENT_STORE_SHARED_CHILD";

    /// Run by [`a_panic_holding_a_shared_lock_aborts`], in a process of its own: aborting on panics,
    /// panics as [`CHILD`] says, and catches the panic, which it says it did.
    #[test]
    #[ignore = "run by a_panic_holding_a_shared_lock_aborts, in a process of its own"]
    fn child_panics() {
        let Ok(what) = std::env::var(CHILD) else { return };
        abort_on_panics();
        let lock = SharedLock::new("test", Mutex::new(0));
        let caught = std::panic::catch_unwind(|| {
            let mut held = lock.with(Mutex::lock);
            *held += 1;
            if what == "let-go" {
                drop(held);
            }
            panic!("a bug");
        });
        assert!(caught.is_err());
        println!("CAUGHT");
    }

    /// The child's status and its output, stdout then stderr, doing `what`.
    fn child(what: &str) -> (std::process::ExitStatus, String) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shared::tests::child_panics", "--ignored", "--nocapture"])
            .env(CHILD, what)
            .output()
            .unwrap();
        let said =
            String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
        (output.status, said.into_owned())
    }

    /// A panic holding a shared lock aborts the process, once it asks, at the panic, saying which
    /// lock: nothing unwinds to the catch. A panic having let it go unwinds as any does.
    #[test]
    fn a_panic_holding_a_shared_lock_aborts() {
        let (status, said) = child("holding");
        assert_eq!(status.signal(), Some(libc::SIGABRT), "{said}");
        assert!(aborted_holding(&said, "test"), "{said}");
        assert!(!said.contains("CAUGHT"), "{said}");
        let (status, said) = child("let-go");
        assert!(status.success(), "{said}");
        assert!(said.contains("CAUGHT"), "{said}");
        assert!(!said.contains("aborting"), "{said}");
    }

    /// A thread holds the shared locks it took, until it lets them go: a condition variable's wait
    /// lets go of its lock meanwhile; another thread's are its own.
    #[test]
    fn threads_hold_what_they_took() {
        let (first, second) = (
            SharedLock::new("first", Mutex::new(())),
            SharedLock::new("second", Mutex::new(false)),
        );
        let condvar = Condvar::new();
        let held = first.with(Mutex::lock);
        let mut ready = second.with(Mutex::lock);
        assert_eq!(super::held(), ["first", "second"]);
        let ready = std::thread::scope(|scope| {
            scope.spawn(|| {
                assert_eq!(super::held(), Vec::<&str>::new());
                *second.with(Mutex::lock) = true;
                condvar.notify_all();
            });
            while !*ready {
                ready = ready.unlocked(|guard| {
                    assert_eq!(super::held(), ["first"]);
                    condvar.wait(guard).unwrap()
                });
            }
            ready
        });
        assert_eq!(super::held(), ["first", "second"]);
        drop(held);
        assert_eq!(super::held(), ["second"]);
        drop(ready);
        assert_eq!(super::held(), Vec::<&str>::new());
    }
}
