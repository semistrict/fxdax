// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The kernel's operations of Shuttle's primitives (awslabs/shuttle), for tests that run the
//! protocol under Shuttle's scheduler: the seqcount an atomic, the follower's mutex a semaphore of
//! one permit, SRCU the read sections of each generation, counted. What invalidating a batch's
//! changes means is the test's: what its readers cache.

use crate::Kernel;
use crate::shuttle_trace;
use shuttle::future::batch_semaphore::{BatchSemaphore, Fairness};
use shuttle::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use shuttle::sync::{Condvar, Mutex};
use std::collections::BTreeMap;

pub struct ShuttleKernel<C> {
    sequence: AtomicU32,
    mutex: BatchSemaphore,
    /// How often readers took the mutex: their fallbacks.
    fallbacks: AtomicUsize,
    grace: Mutex<Grace>,
    left: Condvar,
    invalidate: Box<dyn Fn(&C) + Send + Sync>,
}

/// SRCU: the read sections of each generation still open.
#[derive(Default)]
struct Grace {
    generation: u32,
    readers: BTreeMap<u32, usize>,
}

impl<C> ShuttleKernel<C> {
    /// A kernel whose invalidation of a batch's changes is `invalidate`.
    pub fn new(invalidate: impl Fn(&C) + Send + Sync + 'static) -> Self {
        Self {
            sequence: AtomicU32::new(0),
            mutex: BatchSemaphore::new(1, Fairness::StrictlyFair),
            fallbacks: AtomicUsize::new(0),
            grace: Mutex::new(Grace::default()),
            left: Condvar::new(),
            invalidate: Box::new(invalidate),
        }
    }

    /// How often readers took the follower's mutex, as their fallback.
    pub fn fallbacks(&self) -> usize {
        self.fallbacks.load(Ordering::SeqCst)
    }
}

impl<C> Kernel for ShuttleKernel<C> {
    type Changes = C;

    fn sequence(&self) -> u32 {
        let sequence = self.sequence.load(Ordering::SeqCst);
        let begins = if sequence % 2 == 0 { "Begin" } else { "BusyBegin" };
        shuttle_trace::reader_event(begins, String::new);
        sequence
    }

    fn moved(&self, start: u32) -> bool {
        let moved = self.sequence.load(Ordering::SeqCst) != start;
        shuttle_trace::reader_event("Validate", String::new);
        moved
    }

    fn write_begin(&self) {
        self.mutex.acquire_blocking(1).unwrap();
        self.sequence.fetch_add(1, Ordering::SeqCst);
        shuttle_trace::event("StartBatch", String::new);
    }

    fn write_end(&self) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        shuttle_trace::event("EndBatch", String::new);
        self.mutex.release(1);
    }

    fn lock(&self) {
        self.fallbacks.fetch_add(1, Ordering::SeqCst);
        self.mutex.acquire_blocking(1).unwrap();
        shuttle_trace::locked(true);
    }

    fn unlock(&self) {
        shuttle_trace::locked(false);
        self.mutex.release(1);
    }

    fn grace_enter(&self) -> u32 {
        let mut grace = self.grace.lock().unwrap();
        let generation = grace.generation;
        *grace.readers.entry(generation).or_default() += 1;
        generation
    }

    fn grace_leave(&self, generation: u32) {
        let mut grace = self.grace.lock().unwrap();
        let readers = grace.readers.get_mut(&generation).unwrap();
        *readers -= 1;
        if *readers == 0 {
            grace.readers.remove(&generation);
        }
        self.left.notify_all();
    }

    fn grace_wait(&self) {
        let mut grace = self.grace.lock().unwrap();
        let waited = grace.generation;
        grace.generation += 1;
        shuttle_trace::event("Invalidated", String::new);
        let before = |grace: &mut Grace| grace.readers.keys().next().is_some_and(|g| *g <= waited);
        let _grace = self.left.wait_while(grace, before).unwrap();
        shuttle_trace::event("GraceOver", String::new);
    }

    fn invalidate(&self, changes: &C) {
        (self.invalidate)(changes)
    }
}
