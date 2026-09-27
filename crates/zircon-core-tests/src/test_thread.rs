// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/pager/test_thread.cc` and
//! `zircon/system/utest/core/pager/test_thread.h`.
//!
//! Deviations:
//! - Threads are host threads. Whether one is blocked on a pager is asked of the kernel directly,
//!   rather than read from `ZX_INFO_THREAD`.
//! - Threads cannot be suspended, and a fault the kernel cannot resolve is fatal to the process,
//!   so there are no `Suspend`, `Resume` or `WaitForCrash` methods.

use std::sync::{Arc, Barrier};
use std::thread::JoinHandle;
use zircon::object::thread_dispatcher::{Blocked, blocked_reason};

/// Executes the specified function on a test thread.
pub struct TestThread {
    func: Option<Box<dyn FnOnce() -> bool + Send>>,
    thread: Option<JoinHandle<bool>>,
}

impl TestThread {
    pub fn new(func: impl FnOnce() -> bool + Send + 'static) -> Self {
        Self { func: Some(Box::new(func)), thread: None }
    }

    /// Starts the test thread's execution.
    pub fn start(&mut self) -> bool {
        assert!(self.thread.is_none());
        let func = self.func.take().expect("thread not yet started");
        // Makes sure that everything is set up before starting the actual test function.
        let startup_sync = Arc::new(Barrier::new(2));
        let thread_sync = startup_sync.clone();
        let thread = std::thread::Builder::new().name("test_thread".into()).spawn(move || {
            thread_sync.wait();
            func()
        });
        let Ok(thread) = thread else { return false };
        self.thread = Some(thread);
        startup_sync.wait();
        true
    }

    fn join(&mut self) -> Option<bool> {
        self.thread.take().expect("thread started").join().ok()
    }

    /// Block until the test thread successfully terminates.
    pub fn wait(&mut self) -> bool {
        self.join() == Some(true)
    }

    /// Block until the test thread terminates with a validation error.
    pub fn wait_for_failure(&mut self) -> bool {
        self.join() == Some(false)
    }

    /// Block until the thread terminates.
    pub fn wait_for_term(&mut self) -> bool {
        self.join().is_some()
    }

    /// Block until the test thread is blocked against a pager.
    pub fn wait_for_blocked(&self) -> bool {
        let thread = self.thread.as_ref().expect("thread started");
        loop {
            if thread.is_finished() {
                return false;
            }
            if blocked_reason(thread.thread().id()) == Blocked::Pager {
                return true;
            }
            // There's no signal to wait on, so just poll
            zx::MonotonicInstant::after(zx::MonotonicDuration::from_micros(100)).sleep();
        }
    }
}

impl Drop for TestThread {
    fn drop(&mut self) {
        // UserPagers need to be destroyed before TestThreads to ensure threads aren't blocked
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
