//! Synchronization: `alloc`'s `Arc`, `core`'s atomics, and std's locks, cells, condition
//! variable and channels. Panics abort, so locks are never poisoned.
//!
//! The locks spin until they are free, yielding the CPU's pipeline as they do. The kernel's Rust
//! should sleep instead, on the kernel's mutexes and wait queues: milestone 1's.

use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

pub use alloc_crate::sync::{Arc, Weak};
pub use core::sync::atomic;

pub mod mpsc;

/// Spins once, for a lock that is held.
fn relax() {
    core::hint::spin_loop();
}

/// An error that a lock was poisoned, which never happens.
pub struct PoisonError<T>(T);

impl<T> PoisonError<T> {
    pub fn new(guard: T) -> Self {
        Self(guard)
    }

    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn get_ref(&self) -> &T {
        &self.0
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> fmt::Debug for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PoisonError")
    }
}

impl<T> fmt::Display for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("poisoned lock")
    }
}

impl<T> core::error::Error for PoisonError<T> {}

/// Why a lock could not be taken without waiting.
pub enum TryLockError<T> {
    Poisoned(PoisonError<T>),
    WouldBlock,
}

impl<T> fmt::Debug for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryLockError::Poisoned(_) => f.write_str("Poisoned(..)"),
            TryLockError::WouldBlock => f.write_str("WouldBlock"),
        }
    }
}

pub type LockResult<T> = Result<T, PoisonError<T>>;
pub type TryLockResult<T> = Result<T, TryLockError<T>>;

/// A mutual exclusion lock.
pub struct Mutex<T: ?Sized> {
    locked: atomic::AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: The lock gives one thread at a time access to the data.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
// SAFETY: As above.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: atomic::AtomicBool::new(false), data: UnsafeCell::new(value) }
    }

    pub fn into_inner(self) -> LockResult<T> {
        Ok(self.data.into_inner())
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        loop {
            if let Ok(guard) = self.try_lock() {
                return Ok(guard);
            }
            while self.locked.load(Ordering::Relaxed) {
                relax();
            }
        }
    }

    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        match self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => Ok(MutexGuard { mutex: self }),
            Err(_) => Err(TryLockError::WouldBlock),
        }
    }

    pub fn is_poisoned(&self) -> bool {
        false
    }

    pub fn clear_poison(&self) {}

    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        Ok(self.data.get_mut())
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for Mutex<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Ok(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
            Err(_) => f.write_str("Mutex { <locked> }"),
        }
    }
}

/// A `Mutex`, locked.
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
}

// SAFETY: The guard gives access to `T` from whichever thread holds it.
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the lock.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard holds the lock.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// A reader-writer lock.
pub struct RwLock<T: ?Sized> {
    /// The number of readers, or `WRITER`.
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

const WRITER: usize = usize::MAX;

// SAFETY: The lock gives readers shared access, and a writer exclusive access.
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
// SAFETY: As above.
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self { state: AtomicUsize::new(0), data: UnsafeCell::new(value) }
    }

    pub fn into_inner(self) -> LockResult<T> {
        Ok(self.data.into_inner())
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        loop {
            if let Ok(guard) = self.try_read() {
                return Ok(guard);
            }
            relax();
        }
    }

    pub fn try_read(&self) -> TryLockResult<RwLockReadGuard<'_, T>> {
        let state = self.state.load(Ordering::Relaxed);
        if state != WRITER
            && state + 1 != WRITER
            && self
                .state
                .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return Ok(RwLockReadGuard { lock: self });
        }
        Err(TryLockError::WouldBlock)
    }

    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        loop {
            if let Ok(guard) = self.try_write() {
                return Ok(guard);
            }
            relax();
        }
    }

    pub fn try_write(&self) -> TryLockResult<RwLockWriteGuard<'_, T>> {
        match self.state.compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => Ok(RwLockWriteGuard { lock: self }),
            Err(_) => Err(TryLockError::WouldBlock),
        }
    }

    pub fn is_poisoned(&self) -> bool {
        false
    }

    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        Ok(self.data.get_mut())
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_read() {
            Ok(guard) => f.debug_struct("RwLock").field("data", &&*guard).finish(),
            Err(_) => f.write_str("RwLock { <locked> }"),
        }
    }
}

/// An `RwLock`, read.
pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: The guard holds a read lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}

/// An `RwLock`, written.
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the write lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard holds the write lock.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
    }
}

/// A condition variable: waiters spin until notified, and may wake spuriously.
#[derive(Default)]
pub struct Condvar {
    notifications: AtomicUsize,
}

/// Whether a wait timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

impl Condvar {
    pub const fn new() -> Self {
        Self { notifications: AtomicUsize::new(0) }
    }

    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        Ok(self
            .wait_timeout(guard, core::time::Duration::MAX)
            .unwrap_or_else(PoisonError::into_inner)
            .0)
    }

    pub fn wait_while<'a, T, F: FnMut(&mut T) -> bool>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> LockResult<MutexGuard<'a, T>> {
        while condition(&mut *guard) {
            guard = self.wait(guard)?;
        }
        Ok(guard)
    }

    pub fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        timeout: core::time::Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let mutex = guard.mutex;
        let seen = self.notifications.load(Ordering::Acquire);
        drop(guard);
        let start = crate::time::Instant::now();
        let mut timed_out = false;
        while self.notifications.load(Ordering::Acquire) == seen {
            if start.elapsed() >= timeout {
                timed_out = true;
                break;
            }
            relax();
        }
        Ok((mutex.lock().unwrap_or_else(PoisonError::into_inner), WaitTimeoutResult(timed_out)))
    }

    pub fn notify_one(&self) {
        self.notifications.fetch_add(1, Ordering::Release);
    }

    pub fn notify_all(&self) {
        self.notifications.fetch_add(1, Ordering::Release);
    }
}

impl fmt::Debug for Condvar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Condvar")
    }
}

const INCOMPLETE: u8 = 0;
const RUNNING: u8 = 1;
const COMPLETE: u8 = 2;

/// Runs an initializer once.
pub struct Once {
    state: AtomicU8,
}

impl Once {
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(INCOMPLETE) }
    }

    pub fn call_once<F: FnOnce()>(&self, f: F) {
        if self.state.load(Ordering::Acquire) == COMPLETE {
            return;
        }
        loop {
            match self.state.compare_exchange(
                INCOMPLETE,
                RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    f();
                    self.state.store(COMPLETE, Ordering::Release);
                    return;
                }
                Err(COMPLETE) => return,
                Err(_) => relax(),
            }
        }
    }

    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

/// A cell written once.
pub struct OnceLock<T> {
    once: Once,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: The value is written once, before any thread reads it.
unsafe impl<T: Send> Send for OnceLock<T> {}
// SAFETY: As above.
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self { once: Once::new(), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    pub fn get(&self) -> Option<&T> {
        // SAFETY: Once complete, the value is written and never written again.
        self.once.is_completed().then(|| unsafe { (*self.value.get()).assume_init_ref() })
    }

    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        self.once.call_once(|| {
            // SAFETY: Only the one call that runs writes the value.
            unsafe { (*self.value.get()).write(f()) };
        });
        self.get().unwrap()
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        let mut value = Some(value);
        self.get_or_init(|| value.take().unwrap());
        match value {
            None => Ok(()),
            Some(value) => Err(value),
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        // SAFETY: Once complete, the value is written.
        self.once.is_completed().then(|| unsafe { self.value.get_mut().assume_init_mut() })
    }

    /// Takes the value out, leaving the cell uninitialized.
    pub fn take(&mut self) -> Option<T> {
        if !self.once.is_completed() {
            return None;
        }
        *self.once.state.get_mut() = INCOMPLETE;
        // SAFETY: The value was written, and the cell no longer claims it.
        Some(unsafe { self.value.get_mut().assume_init_read() })
    }

    pub fn into_inner(mut self) -> Option<T> {
        self.take()
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.once.is_completed() {
            // SAFETY: The value was written, and is dropped once.
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OnceLock").field(&self.get()).finish()
    }
}

/// A value initialized on first use.
pub struct LazyLock<T, F = fn() -> T> {
    cell: OnceLock<T>,
    init: UnsafeCell<Option<F>>,
}

// SAFETY: `init` is taken once, by the thread that initializes the cell.
unsafe impl<T: Send + Sync, F: Send> Sync for LazyLock<T, F> {}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    pub const fn new(init: F) -> Self {
        Self { cell: OnceLock::new(), init: UnsafeCell::new(Some(init)) }
    }

    pub fn force(this: &Self) -> &T {
        this.cell.get_or_init(|| {
            // SAFETY: Only the one initializing call takes `init`.
            let init = unsafe { (*this.init.get()).take() };
            init.expect("LazyLock initialized twice")()
        })
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;
    fn deref(&self) -> &T {
        Self::force(self)
    }
}
