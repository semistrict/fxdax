// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a store does once a panic went through it: it is fenced (TASK-153.16). It serves
//! nothing more, each request failing with `EIO`, rather than panicking at its locks' poison, as
//! every request after the first did in TASK-104, TASK-105 and TASK-150.20; and nothing reads what
//! the panic left half done. Its guest's filesystem meets `EIO` and stops, its VM with it. The
//! connection's end closes it as a writer's that went with writes uncommitted, and the next to
//! open it has it as of its last commit: a store is usable again opened anew, as after a crash.
//!
//! A store's locks are [`StoreMutex`]es and [`StoreRwLock`]s, locked only through its [`Fence`]:
//! `EIO` once it is fenced, and a guard ([`Held`]) dropped as a panic unwinds fences it. So no lock
//! of a store's is held across a panic without fencing it, and none is locked poisoned. What
//! panics beneath a store, past its locks, the extent daemon fences it for, as it answers the
//! request that panicked.
//!
//! A lock shared beyond one store, which no fence contains, as the process's cache of extents, a
//! window's, a connection's, or `extent_store`'s store's, which its followers share with its writer,
//! is an `extent_store::shared::SharedLock`: a panic holding one stops the daemon (TASK-149.24).

use crate::error::{Errno, Result};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{
    LockResult, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

/// Whether a store is fenced, and why: by the first panic to go through it.
pub(crate) struct Fence {
    /// The store's path, which its log names.
    store: PathBuf,
    why: OnceLock<String>,
}

impl Fence {
    /// The fence of the store at `store`, not fenced.
    pub(crate) fn new(store: &Path) -> Self {
        Self { store: store.to_owned(), why: OnceLock::new() }
    }

    /// Fences the store, as `why` says, unless it is already: logged once.
    pub(crate) fn fence(&self, why: impl FnOnce() -> String) {
        let mut said = None;
        self.why.get_or_init(|| said.insert(why()).clone());
        if let Some(why) = said {
            log::error!(
                "fxdax: the store at {} is fenced, {why}: it serves nothing more, each request \
                 failing with EIO, until it is opened anew, as of its last commit",
                self.store.display()
            );
        }
    }

    /// Whether the store is fenced.
    pub(crate) fn is_fenced(&self) -> bool {
        self.why.get().is_some()
    }

    /// `EIO` if the store is fenced.
    pub(crate) fn check(&self) -> Result<()> {
        if self.is_fenced() { Err(Errno::EIO) } else { Ok(()) }
    }

    /// `locked`, the guard of the store's lock called `lock`, held: `EIO`, the lock let go of, if
    /// the store is fenced, as it may have been while this waited for it; or if the lock is
    /// poisoned, which fences it, though no lock of a store's is poisoned unfenced.
    fn held<G>(&self, lock: &'static str, locked: LockResult<G>) -> Result<Held<'_, G>> {
        let guard = locked.map_err(|_| {
            self.fence(|| format!("its {lock} lock found poisoned"));
            Errno::EIO
        })?;
        self.check()?;
        let held = Held { guard, fence: self, lock };
        #[cfg(test)]
        tests::panic_if_asked(lock);
        Ok(held)
    }
}

/// A guard of a store's lock: which fences the store if a panic drops it, holding the lock.
pub(crate) struct Held<'a, G> {
    guard: G,
    fence: &'a Fence,
    lock: &'static str,
}

impl<G: Deref> Deref for Held<'_, G> {
    type Target = G::Target;

    fn deref(&self) -> &G::Target {
        &self.guard
    }
}

impl<G: DerefMut> DerefMut for Held<'_, G> {
    fn deref_mut(&mut self) -> &mut G::Target {
        &mut self.guard
    }
}

// Before the guard is dropped, which lets the lock go: whoever takes it next finds the store fenced.
impl<G> Drop for Held<'_, G> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.fence.fence(|| format!("a panic holding its {} lock", self.lock));
        }
    }
}

/// A mutex of a store's, locked only through its fence ([`StoreMutex::lock`]).
pub(crate) struct StoreMutex<T> {
    /// What the store's log calls it.
    name: &'static str,
    lock: Mutex<T>,
}

impl<T> StoreMutex<T> {
    pub(crate) const fn new(name: &'static str, value: T) -> Self {
        Self { name, lock: Mutex::new(value) }
    }

    /// Locks it, for the store `fence` fences: `EIO` if it is fenced.
    pub(crate) fn lock<'a>(&'a self, fence: &'a Fence) -> Result<Held<'a, MutexGuard<'a, T>>> {
        fence.held(self.name, self.lock.lock())
    }
}

/// A reader-writer lock of a store's, locked only through its fence, as a [`StoreMutex`] is.
pub(crate) struct StoreRwLock<T> {
    /// What the store's log calls it.
    name: &'static str,
    lock: RwLock<T>,
}

impl<T> StoreRwLock<T> {
    pub(crate) const fn new(name: &'static str, value: T) -> Self {
        Self { name, lock: RwLock::new(value) }
    }

    /// Locks it shared, for the store `fence` fences: `EIO` if it is fenced. A panic holding it
    /// shared fences the store too, which poisons no reader-writer lock.
    pub(crate) fn read<'a>(&'a self, fence: &'a Fence) -> Result<Held<'a, RwLockReadGuard<'a, T>>> {
        fence.held(self.name, self.lock.read())
    }

    /// Locks it alone, for the store `fence` fences: `EIO` if it is fenced.
    pub(crate) fn write<'a>(
        &'a self,
        fence: &'a Fence,
    ) -> Result<Held<'a, RwLockWriteGuard<'a, T>>> {
        fence.held(self.name, self.lock.write())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The store's lock a test asks to be panicked holding, once: as it is next taken.
    static PANIC_HOLDING: Mutex<Option<&'static str>> = Mutex::new(None);

    /// Panics holding the store's lock called `lock` as it is next taken, by any store: a bug's
    /// panic under it, as a lazily made resource's `expect` was in TASK-104 and TASK-150.20.
    pub(crate) fn panic_holding(lock: &'static str) {
        *PANIC_HOLDING.lock().unwrap() = Some(lock);
    }

    pub(super) fn panic_if_asked(lock: &'static str) {
        let mut asked = PANIC_HOLDING.lock().unwrap();
        if *asked == Some(lock) {
            *asked = None;
            drop(asked);
            panic!("a panic holding a store's {lock} lock, as a test asks");
        }
    }

    /// A panic holding a store's lock fences the store: that lock and every other of the store's
    /// refuse with `EIO` from then on, once let go of, rather than panic at its poison or let the
    /// store be read past it; another store's are untouched.
    #[test]
    fn a_panic_holding_a_lock_fences_its_store() {
        let (fence, other) = (Fence::new(Path::new("store")), Fence::new(Path::new("other")));
        let (lock, rw) = (StoreMutex::new("first", 1), StoreRwLock::new("second", 2));
        let panicked = std::panic::catch_unwind(|| {
            let mut held = lock.lock(&fence).unwrap();
            *held += 1;
            panic!("a bug, holding the lock");
        });
        assert!(panicked.is_err());
        assert!(fence.is_fenced());
        assert_eq!(lock.lock(&fence).err(), Some(Errno::EIO));
        assert_eq!(rw.read(&fence).err(), Some(Errno::EIO));
        assert_eq!(rw.write(&fence).err(), Some(Errno::EIO));
        assert_eq!(fence.check(), Err(Errno::EIO));
        let unfenced = StoreMutex::new("third", 3);
        assert_eq!(*unfenced.lock(&other).unwrap(), 3);
        assert_eq!(other.check(), Ok(()));
    }

    /// A panic holding a store's reader-writer lock shared, which poisons no lock, fences the store
    /// too.
    #[test]
    fn a_panic_holding_a_lock_shared_fences_its_store() {
        let fence = Fence::new(Path::new("store"));
        let rw = StoreRwLock::new("shared", ());
        let panicked = std::panic::catch_unwind(|| {
            let _held = rw.read(&fence).unwrap();
            panic!("a bug, holding the lock shared");
        });
        assert!(panicked.is_err());
        assert_eq!(rw.write(&fence).err(), Some(Errno::EIO));
    }

    /// A store's lock taken without a panic, and let go of, leaves it unfenced, however often.
    #[test]
    fn locks_let_go_of_leave_their_store_unfenced() {
        let fence = Fence::new(Path::new("store"));
        let lock = StoreMutex::new("lock", 0);
        for _ in 0..3 {
            *lock.lock(&fence).unwrap() += 1;
        }
        assert_eq!(*lock.lock(&fence).unwrap(), 3);
        assert!(!fence.is_fenced());
    }
}
