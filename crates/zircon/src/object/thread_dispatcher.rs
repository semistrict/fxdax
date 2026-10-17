// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What the kernel knows of threads: why they are blocked.
//!
//! Ported from `ThreadDispatcher::Blocked` and `ThreadDispatcher::AutoBlocked` in
//! `zircon/kernel/object/include/object/thread_dispatcher.h`.
//!
//! Deviations:
//! - Threads are host threads, without thread objects or handles. The reason a thread is blocked is
//!   recorded by host thread id, and read with `blocked_reason` rather than `ZX_INFO_THREAD`.
//! - A thread's exception channel only receives fatal page faults, as `ExceptionReport`s. A thread
//!   in an exception stays blocked for good, rather than until the exception is handled.

use crate::ensure;
use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::ThreadId;
use zx_status::Status;

/// Why a thread is blocked. Only the reasons the kernel records are included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blocked {
    /// Not blocked.
    #[default]
    None,
    /// pager
    Pager,
}

static BLOCKED_REASONS: Mutex<Option<HashMap<ThreadId, Blocked>>> = Mutex::new(None);

fn set_blocked_reason(reason: Blocked) -> Blocked {
    let mut reasons = BLOCKED_REASONS.lock();
    let reasons = reasons.get_or_insert_with(HashMap::new);
    let id = std::thread::current().id();
    let previous = match reason {
        Blocked::None => reasons.remove(&id),
        reason => reasons.insert(id, reason),
    };
    previous.unwrap_or_default()
}

/// Returns why the thread `id` is blocked.
pub fn blocked_reason(id: ThreadId) -> Blocked {
    BLOCKED_REASONS
        .lock()
        .as_ref()
        .and_then(|reasons| reasons.get(&id).copied())
        .unwrap_or_default()
}

/// Records that the current thread is blocked for `reason` while it exists.
pub struct AutoBlocked {
    prev_reason: Blocked,
}

impl AutoBlocked {
    pub fn new(reason: Blocked) -> Self {
        debug_assert_ne!(reason, Blocked::None);
        Self { prev_reason: set_blocked_reason(reason) }
    }
}

impl Drop for AutoBlocked {
    fn drop(&mut self) {
        set_blocked_reason(self.prev_reason);
    }
}

/// A fatal page fault, as the exception report of `ZX_EXCP_FATAL_PAGE_FAULT` describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionReport {
    /// The address that faulted.
    pub fault_address: usize,
    /// The status the fault could not be resolved with, the report's `synth_code`.
    pub synth_code: Status,
}

#[derive(Default)]
struct ExceptionChannelState {
    report: Option<ExceptionReport>,
    terminated: bool,
}

/// Receives the exceptions of a thread, and learns when it terminates.
#[derive(Default)]
pub struct ExceptionChannel {
    state: Mutex<ExceptionChannelState>,
    cond: Condvar,
}

impl ExceptionChannel {
    /// Records that the thread terminated, as the channel's peer closing does.
    pub fn thread_terminated(&self) {
        self.state.lock().terminated = true;
        self.cond.notify_all();
    }

    /// Waits until the thread takes an exception, which is returned, or terminates.
    pub fn wait(&self) -> Option<ExceptionReport> {
        let mut state = self.state.lock();
        while state.report.is_none() && !state.terminated {
            self.cond.wait(&mut state);
        }
        state.report
    }
}

static EXCEPTION_CHANNELS: Mutex<Option<HashMap<ThreadId, Arc<ExceptionChannel>>>> =
    Mutex::new(None);

/// Creates an exception channel for the thread `id`, which it has until it is removed.
pub fn create_exception_channel(id: ThreadId) -> Result<Arc<ExceptionChannel>, Status> {
    let mut channels = EXCEPTION_CHANNELS.lock();
    let channels = channels.get_or_insert_with(HashMap::new);
    ensure!(!channels.contains_key(&id), Status::ALREADY_BOUND);
    let channel = Arc::new(ExceptionChannel::default());
    channels.insert(id, channel.clone());
    Ok(channel)
}

/// Removes the exception channel of the thread `id`.
pub fn remove_exception_channel(id: ThreadId) {
    if let Some(channels) = EXCEPTION_CHANNELS.lock().as_mut() {
        channels.remove(&id);
    }
}

/// Delivers a fatal page fault to the current thread's exception channel, if it has one, and then
/// blocks the thread for good. Returns if it has none.
pub fn deliver_fatal_page_fault(report: ExceptionReport) {
    let channel = EXCEPTION_CHANNELS
        .lock()
        .as_ref()
        .and_then(|channels| channels.get(&std::thread::current().id()).cloned());
    let Some(channel) = channel else { return };
    channel.state.lock().report = Some(report);
    channel.cond.notify_all();
    loop {
        std::thread::park();
    }
}
