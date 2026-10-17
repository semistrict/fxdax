// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: raw locks for the guest kernel (TASK-26.6), whose std's are over the kernel's futexes.

use lock_api::GuardSend;
use std::os::kernel::sync;

/// A raw mutex: std's, which sleeps on a futex while the mutex is locked.
pub struct RawMutex(sync::Mutex);

// SAFETY: std's mutex is a mutex: one holder at a time, until it unlocks.
unsafe impl lock_api::RawMutex for RawMutex {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self(sync::Mutex::new());

    type GuardMarker = GuardSend;

    fn lock(&self) {
        self.0.lock();
    }

    fn try_lock(&self) -> bool {
        self.0.try_lock()
    }

    unsafe fn unlock(&self) {
        // SAFETY: As the caller promises, the mutex is locked in this context.
        unsafe { self.0.unlock() }
    }
}

/// A raw reader-writer lock: std's, which sleeps on futexes while the lock is held.
pub struct RawRwLock(sync::RwLock);

// SAFETY: std's lock is a reader-writer lock: readers or one writer at a time.
unsafe impl lock_api::RawRwLock for RawRwLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self(sync::RwLock::new());

    type GuardMarker = GuardSend;

    fn lock_shared(&self) {
        self.0.read();
    }

    fn try_lock_shared(&self) -> bool {
        self.0.try_read()
    }

    unsafe fn unlock_shared(&self) {
        // SAFETY: As the caller promises, the lock is read-locked in this context.
        unsafe { self.0.read_unlock() }
    }

    fn lock_exclusive(&self) {
        self.0.write();
    }

    fn try_lock_exclusive(&self) -> bool {
        self.0.try_write()
    }

    unsafe fn unlock_exclusive(&self) {
        // SAFETY: As the caller promises, the lock is write-locked in this context.
        unsafe { self.0.write_unlock() }
    }
}
