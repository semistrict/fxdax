//! Events threads can block on.
//!
//! Ported from `zircon/kernel/kernel/event.rs` and
//! `zircon/kernel/include/kernel/event.h`.
//!
//! An `Event` stays signaled, releasing every waiter, until it is unsignaled.
//! An `AutounsignalEvent` releases a single waiter per signal. Both carry the
//! status passed to `signal_result` back to waiters.
//!
//! Deviation: waits cannot be interrupted by thread suspension or killing,
//! so they never return `ZX_ERR_INTERNAL_INTR_RETRY` or `_KILLED`.

use super::deadline::Deadline;
use parking_lot::{Condvar, Mutex};
use zx_status::Status;

struct State {
    signaled: bool,
    result: Result<(), Status>,
}

/// An event that stays signaled until it is unsignaled.
pub struct Event {
    state: Mutex<State>,
    cond: Condvar,
    autounsignal: bool,
}

impl Event {
    pub fn new(initial: bool) -> Self {
        Self::with_mode(initial, false)
    }

    /// Creates an event that releases a single waiter per signal, as an `AutounsignalEvent` does,
    /// for sharing where an `Event` is expected.
    pub fn new_autounsignal(initial: bool) -> Self {
        Self::with_mode(initial, true)
    }

    fn with_mode(initial: bool, autounsignal: bool) -> Self {
        Self {
            state: Mutex::new(State { signaled: initial, result: Ok(()) }),
            cond: Condvar::new(),
            autounsignal,
        }
    }

    /// Waits for the event to be signaled or for `deadline` to pass.
    pub fn wait(&self, deadline: &Deadline) -> Result<(), Status> {
        let mut state = self.state.lock();
        while !state.signaled {
            match deadline.remaining() {
                None => self.cond.wait(&mut state),
                Some(remaining) if remaining.is_zero() => return Err(Status::TIMED_OUT),
                Some(remaining) => {
                    self.cond.wait_for(&mut state, remaining);
                }
            }
        }
        let result = state.result;
        if self.autounsignal {
            state.signaled = false;
        }
        result
    }

    pub fn wait_infinite(&self) -> Result<(), Status> {
        self.wait(&Deadline::infinite())
    }

    /// Signals the event with `ZX_OK`.
    pub fn signal(&self) {
        self.signal_result(Ok(()));
    }

    /// Signals the event, waking waiters with `result`.
    pub fn signal_result(&self, result: Result<(), Status>) {
        let mut state = self.state.lock();
        state.signaled = true;
        state.result = result;
        if self.autounsignal {
            self.cond.notify_one();
        } else {
            self.cond.notify_all();
        }
    }

    /// Returns the event to the unsignaled state.
    pub fn unsignal(&self) {
        self.state.lock().signaled = false;
    }

    pub fn is_signaled(&self) -> bool {
        self.state.lock().signaled
    }
}

/// An event where each signal releases at most one waiter, after which the
/// event is unsignaled again.
pub struct AutounsignalEvent(Event);

impl AutounsignalEvent {
    pub fn new(initial: bool) -> Self {
        Self(Event::with_mode(initial, true))
    }

    pub fn as_event(&self) -> &Event {
        &self.0
    }
}

impl std::ops::Deref for AutounsignalEvent {
    type Target = Event;

    fn deref(&self) -> &Event {
        &self.0
    }
}
