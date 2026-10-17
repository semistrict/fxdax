// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/pager/test_thread.cc` and
//! `zircon/system/utest/core/pager/test_thread.h`.
//!
//! Deviations:
//! - Threads are host threads. Whether one is blocked on a pager is asked of the kernel directly,
//!   rather than read from `ZX_INFO_THREAD`, and exceptions arrive on the kernel's exception
//!   channel for the host thread.
//! - A thread that crashes stays blocked in its exception, rather than being redirected to exit,
//!   and is not joined.
//! - Threads cannot be suspended, so there are no `Suspend` or `Resume` methods.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, OnceLock};
use std::thread::JoinHandle;
use zircon::object::thread_dispatcher::{
    Blocked, ExceptionChannel, blocked_reason, create_exception_channel, remove_exception_channel,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpectStatus {
    Success,
    Failure,
    Crash,
}

/// Executes the specified function on a test thread.
pub struct TestThread {
    func: Option<Box<dyn FnOnce() -> bool + Send>>,
    thread: Option<JoinHandle<()>>,
    exception_channel: Option<Arc<ExceptionChannel>>,
    success: Arc<AtomicBool>,
    crashed: bool,
}

impl TestThread {
    pub fn new(func: impl FnOnce() -> bool + Send + 'static) -> Self {
        Self {
            func: Some(Box::new(func)),
            thread: None,
            exception_channel: None,
            success: Arc::new(AtomicBool::new(false)),
            crashed: false,
        }
    }

    /// Starts the test thread's execution.
    pub fn start(&mut self) -> bool {
        assert!(self.thread.is_none());
        let func = self.func.take().expect("thread not yet started");
        // Makes sure that everything is set up before starting the actual test function.
        let startup_sync = Arc::new(Barrier::new(2));
        let thread_sync = startup_sync.clone();
        let success = self.success.clone();
        let channel_slot = Arc::new(OnceLock::<Arc<ExceptionChannel>>::new());
        let thread_channel = channel_slot.clone();
        let thread = std::thread::Builder::new().name("test_thread".into()).spawn(move || {
            thread_sync.wait();
            // Tell the exception channel when the thread terminates, as its peer closing does.
            struct Terminated(Arc<ExceptionChannel>);
            impl Drop for Terminated {
                fn drop(&mut self) {
                    self.0.thread_terminated();
                }
            }
            let _terminated = Terminated(thread_channel.get().expect("channel").clone());
            success.store(func(), Ordering::Release);
        });
        let Ok(thread) = thread else { return false };
        let Ok(channel) = create_exception_channel(thread.thread().id()) else { return false };
        assert!(channel_slot.set(channel.clone()).is_ok());
        self.exception_channel = Some(channel);
        self.thread = Some(thread);
        startup_sync.wait();
        true
    }

    /// Block until the test thread successfully terminates.
    pub fn wait(&mut self) -> bool {
        self.wait_for(ExpectStatus::Success, None)
    }

    /// Block until the test thread terminates with a validation error.
    pub fn wait_for_failure(&mut self) -> bool {
        self.wait_for(ExpectStatus::Failure, None)
    }

    /// Block until the test thread crashes due to an access to `crash_addr`. The exception
    /// report's `synth_code` field should be set to `error_status`.
    pub fn wait_for_crash(&mut self, crash_addr: usize, error_status: zx::Status) -> bool {
        self.wait_for(ExpectStatus::Crash, Some((crash_addr, error_status)))
    }

    /// Block until the thread terminates.
    pub fn wait_for_term(&mut self) -> bool {
        let thread = self.thread.take().expect("thread started");
        let result = thread.join().is_ok();
        self.finish();
        result
    }

    fn wait_for(&mut self, expect: ExpectStatus, crash_info: Option<(usize, zx::Status)>) -> bool {
        let channel = self.exception_channel.clone().expect("thread started");
        match channel.wait() {
            None => {
                // Thread has terminated.
                let joined = self.thread.take().unwrap().join().is_ok();
                self.finish();
                let success = joined && self.success.load(Ordering::Acquire);
                (success && expect == ExpectStatus::Success)
                    || (!success && expect == ExpectStatus::Failure)
            }
            Some(report) => {
                self.crashed = true;
                let mut res = expect == ExpectStatus::Crash;
                if let Some((crash_addr, error_status)) = crash_info {
                    res &= crash_addr == report.fault_address;
                    res &= error_status == report.synth_code;
                }
                if !res {
                    // Print debug info if the crash wasn't expected.
                    println!(
                        "\nCrash info:\nfault address {:#x} synth_code {:?}",
                        report.fault_address, report.synth_code
                    );
                }
                res
            }
        }
    }

    fn finish(&mut self) {
        if let Some(thread) = &self.thread {
            remove_exception_channel(thread.thread().id());
        }
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
        // UserPagers need to be destroyed before TestThreads to ensure threads aren't blocked. A
        // thread that crashed stays blocked in its exception, and so is not joined.
        if let Some(thread) = self.thread.take() {
            remove_exception_channel(thread.thread().id());
            if !self.crashed {
                let _ = thread.join();
            }
        }
    }
}
