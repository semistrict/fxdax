// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The kernel's operations of Shuttle's primitives (awslabs/shuttle), for tests that run the
//! protocol under Shuttle's scheduler: the fill barrier an rwsem of a mutex and a condvar, whose
//! writer waiting goes before readers coming, as the kernel's does. What dropping a batch's
//! changes means is the test's: what its readers cache.

use crate::Kernel;
use crate::shuttle_trace;
use shuttle::sync::{Condvar, Mutex};

pub struct ShuttleKernel<C> {
    barrier: Mutex<Barrier>,
    changed: Condvar,
    drop_cached: Box<dyn Fn(&C) + Send + Sync>,
}

/// The fill barrier: the readers holding it, whether a batch holds it, and how many wait to.
#[derive(Default)]
struct Barrier {
    readers: usize,
    batch: bool,
    waiting: usize,
}

impl<C> ShuttleKernel<C> {
    /// A kernel whose dropping of what a batch changes is `drop_cached`.
    pub fn new(drop_cached: impl Fn(&C) + Send + Sync + 'static) -> Self {
        Self {
            barrier: Mutex::new(Barrier::default()),
            changed: Condvar::new(),
            drop_cached: Box::new(drop_cached),
        }
    }
}

impl<C> Kernel for ShuttleKernel<C> {
    type Changes = C;

    fn fill_begin(&self) {
        let barrier = self.barrier.lock().unwrap();
        let mut barrier =
            self.changed.wait_while(barrier, |b| b.batch || b.waiting > 0).unwrap();
        barrier.readers += 1;
        // The operation the reader said it starts, as it takes the barrier.
        shuttle_trace::reader_started();
    }

    fn fill_end(&self) {
        let mut barrier = self.barrier.lock().unwrap();
        barrier.readers -= 1;
        self.changed.notify_all();
    }

    fn barrier_take(&self) {
        let mut barrier = self.barrier.lock().unwrap();
        barrier.waiting += 1;
        shuttle_trace::event("WaitBarrier", String::new);
        let mut barrier =
            self.changed.wait_while(barrier, |b| b.batch || b.readers > 0).unwrap();
        barrier.waiting -= 1;
        barrier.batch = true;
        shuttle_trace::event("StartBatch", String::new);
    }

    fn barrier_release(&self) {
        let mut barrier = self.barrier.lock().unwrap();
        barrier.batch = false;
        shuttle_trace::event("EndBatch", String::new);
        self.changed.notify_all();
    }

    fn drop_cached(&self, changes: &C) {
        (self.drop_cached)(changes);
        shuttle_trace::event("Dropped", String::new);
    }

    /// The Shuttle tests' files have no attributes.
    fn settle(&self, _: &C) {}
}
