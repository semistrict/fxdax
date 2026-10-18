// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Events threads can block on.
//!
//! Ported from `zircon/kernel/kernel/event.rs` and
//! `zircon/kernel/include/kernel/event.h`.
//!
//! An `Event` stays signaled, releasing every waiter, until it is unsignaled.
//! An `AutounsignalEvent` releases a single waiter per signal. Both carry the
//! status passed to `signal_result` back to waiters.
//!
//! Deviations:
//! - Its condition variable is `crate::sync`'s: parking_lot's, or one over Shuttle's primitives
//!   with the `shuttle` feature.
//! - Waits cannot be interrupted by thread suspension or killing, so they never return
//!   `ZX_ERR_INTERNAL_INTR_RETRY` or `_KILLED`.
//! - A future may wait in place of a thread, with [`Event::wait_async`] (see `kernel::wakers`).

use super::deadline::Deadline;
use super::timer::{Wakeup, wake_at};
use super::wakers::Wakers;
use crate::sync::Condvar;
use parking_lot::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use zx_status::Status;

struct State {
    signaled: bool,
    result: Result<(), Status>,
    /// The futures waiting, alongside the threads waiting on `cond`.
    wakers: Wakers,
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
            state: Mutex::new(State { signaled: initial, result: Ok(()), wakers: Wakers::new() }),
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

    /// Waits for the event to be signaled or for `deadline` to pass, as [`Event::wait`] does, as a
    /// future rather than by blocking the calling thread.
    pub fn wait_async(&self, deadline: Deadline) -> EventWait<'_> {
        EventWait { event: self, deadline, key: None, wakeup: None }
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
        // Futures race for an autounsignal event's signal, and those that lose wait again.
        let wake = state.wakers.take();
        drop(state);
        drop(wake);
    }

    /// Returns the event to the unsignaled state.
    pub fn unsignal(&self) {
        self.state.lock().signaled = false;
    }

    pub fn is_signaled(&self) -> bool {
        self.state.lock().signaled
    }
}

/// A future waiting for an [`Event`]. See [`Event::wait_async`].
pub struct EventWait<'a> {
    event: &'a Event,
    deadline: Deadline,
    /// The key of this future's waker in the event's wakers, once registered.
    key: Option<u64>,
    /// The wakeup at the deadline, once armed.
    wakeup: Option<Wakeup>,
}

impl Future for EventWait<'_> {
    type Output = Result<(), Status>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.event.state.lock();
        if state.signaled {
            state.wakers.remove(&mut this.key);
            if this.event.autounsignal {
                state.signaled = false;
            }
            return Poll::Ready(state.result);
        }
        if this.deadline.remaining().is_some_and(|remaining| remaining.is_zero()) {
            state.wakers.remove(&mut this.key);
            return Poll::Ready(Err(Status::TIMED_OUT));
        }
        state.wakers.register(&mut this.key, cx.waker());
        drop(state);
        match &this.wakeup {
            Some(wakeup) => wakeup.set_waker(cx.waker()),
            None => this.wakeup = Some(wake_at(this.deadline, cx.waker())),
        }
        Poll::Pending
    }
}

impl Drop for EventWait<'_> {
    fn drop(&mut self) {
        if self.key.is_some() {
            self.event.state.lock().wakers.remove(&mut self.key);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use futures::task::noop_waker_ref;
    use std::sync::Arc;

    fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
        future.poll_unpin(&mut Context::from_waker(noop_waker_ref()))
    }

    #[test]
    fn async_wait_completes_once_signaled() {
        let event = Event::new(false);
        let mut wait = event.wait_async(Deadline::infinite());
        assert!(poll_once(&mut wait).is_pending());
        event.signal_result(Err(Status::BAD_STATE));
        assert_eq!(poll_once(&mut wait), Poll::Ready(Err(Status::BAD_STATE)));
        // The event stays signaled.
        assert_eq!(
            poll_once(&mut event.wait_async(Deadline::infinite())),
            Poll::Ready(Err(Status::BAD_STATE))
        );
    }

    #[test]
    fn async_wait_wakes_its_task_from_another_thread() {
        let event = Arc::new(Event::new(false));
        let signaler = event.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            signaler.signal();
        });
        assert_eq!(futures::executor::block_on(event.wait_async(Deadline::infinite())), Ok(()));
        thread.join().unwrap();
    }

    #[test]
    fn async_wait_times_out() {
        let event = Event::new(false);
        let result = futures::executor::block_on(event.wait_async(Deadline::after_mono(5_000_000)));
        assert_eq!(result, Err(Status::TIMED_OUT));
    }

    #[test]
    fn autounsignal_releases_one_async_waiter_per_signal() {
        let event = AutounsignalEvent::new(false);
        let (mut first, mut second) =
            (event.wait_async(Deadline::infinite()), event.wait_async(Deadline::infinite()));
        assert!(poll_once(&mut first).is_pending());
        assert!(poll_once(&mut second).is_pending());
        event.signal();
        assert_eq!(poll_once(&mut first), Poll::Ready(Ok(())));
        assert!(poll_once(&mut second).is_pending());
        event.signal();
        assert_eq!(poll_once(&mut second), Poll::Ready(Ok(())));
    }

    #[test]
    fn dropped_async_waits_unregister() {
        let event = Event::new(false);
        let mut wait = event.wait_async(Deadline::infinite());
        assert!(poll_once(&mut wait).is_pending());
        drop(wait);
        assert!(event.state.lock().wakers.is_empty());
    }
}
