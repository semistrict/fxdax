// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! One-shot timers that call back at a deadline.
//!
//! Ported from the interface of `Timer` in `zircon/kernel/kernel/timer.cc`.
//!
//! Deviations:
//! - Its condition variable is `crate::sync`'s: parking_lot's, or one over Shuttle's primitives
//!   with the `shuttle` feature.
//! - Its threads start through `crate::sync::spawn`, which with the `shuttle` feature starts
//!   none, as time doesn't pass under Shuttle.
//! - Callbacks run on a host thread started for each armed timer, rather than from the timer
//!   interrupt, and are passed nothing.
//! - Only one-shot timers, `set_oneshot` and `cancel`.
//! - [`wake_at`] wakes a future's task at a deadline, for futures waiting in place of threads (see
//!   `kernel::wakers`). One host thread serves every such deadline.

use super::deadline::{Deadline, current_mono_time};
use super::event::Event;
use crate::sync::Condvar;
use parking_lot::Mutex;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, OnceLock};
use std::task::Waker;
use std::time::Duration;
use zx_status::Status;
use zx_types::zx_instant_mono_t;

struct Armed {
    generation: u64,
    /// Signaled to wake the timer's thread when it is canceled.
    cancel: Arc<Event>,
}

struct TimerState {
    armed: Option<Armed>,
    generation: u64,
}

/// A timer that calls back once its deadline passes, unless it is canceled first.
pub struct Timer {
    /// Held while the callback runs, so that `cancel` waits for a running callback.
    state: Arc<Mutex<TimerState>>,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(TimerState { armed: None, generation: 0 })) }
    }

    /// Calls `callback` once at `deadline`. The timer must not already be armed.
    pub fn set_oneshot(
        &self,
        deadline: zx_instant_mono_t,
        callback: impl FnOnce() + Send + 'static,
    ) {
        let mut state = self.state.lock();
        assert!(state.armed.is_none(), "timer already armed");
        state.generation += 1;
        let generation = state.generation;
        let cancel = Arc::new(Event::new(false));
        state.armed = Some(Armed { generation, cancel: cancel.clone() });
        let timer_state = self.state.clone();
        crate::sync::spawn("timer", move || {
            if cancel.wait(&Deadline::no_slack(deadline)) != Err(Status::TIMED_OUT) {
                // Canceled.
                return;
            }
            let mut state = timer_state.lock();
            if state.armed.as_ref().is_some_and(|armed| armed.generation == generation) {
                state.armed = None;
                callback();
            }
        });
    }

    /// Cancels the timer, returning whether it was armed and had not yet called back. Once this
    /// returns the callback is not running.
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock();
        match state.armed.take() {
            Some(armed) => {
                armed.cancel.signal();
                true
            }
            None => false,
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The deadlines of futures' wakeups, served by one thread.
struct Wakeups {
    queue: Mutex<WakeupQueue>,
    /// Notified when an earlier deadline is queued.
    changed: Condvar,
}

#[derive(Default)]
struct WakeupQueue {
    /// Deadlines and the ids of their wakeups, soonest first. A dropped wakeup's entry stays
    /// until it is due, and is then skipped.
    deadlines: BinaryHeap<Reverse<(zx_instant_mono_t, u64)>>,
    wakers: HashMap<u64, Waker>,
    next_id: u64,
}

impl Wakeups {
    fn get() -> &'static Wakeups {
        static WAKEUPS: OnceLock<Wakeups> = OnceLock::new();
        WAKEUPS.get_or_init(|| {
            crate::sync::spawn("wakeups", || Wakeups::get().serve());
            Wakeups { queue: Mutex::new(WakeupQueue::default()), changed: Condvar::new() }
        })
    }

    /// Wakes each waker at its deadline.
    fn serve(&self) -> ! {
        let mut queue = self.queue.lock();
        loop {
            let now = current_mono_time();
            let mut due = Vec::new();
            while let Some(&Reverse((when, id))) = queue.deadlines.peek()
                && when <= now
            {
                queue.deadlines.pop();
                due.extend(queue.wakers.remove(&id));
            }
            if !due.is_empty() {
                // Wake them unlocked, then look again: deadlines may have been queued meanwhile.
                parking_lot::MutexGuard::unlocked(&mut queue, || {
                    due.into_iter().for_each(Waker::wake)
                });
                continue;
            }
            match queue.deadlines.peek() {
                Some(&Reverse((when, _))) => {
                    self.changed.wait_for(&mut queue, Duration::from_nanos((when - now) as u64));
                }
                None => self.changed.wait(&mut queue),
            }
        }
    }
}

/// Wakes `waker` once `deadline` passes, unless the returned wakeup is dropped first. An infinite
/// deadline never wakes.
pub fn wake_at(deadline: Deadline, waker: &Waker) -> Wakeup {
    let wakeups = Wakeups::get();
    let mut queue = wakeups.queue.lock();
    let id = queue.next_id;
    queue.next_id += 1;
    if deadline.remaining().is_some() {
        let soonest =
            queue.deadlines.peek().is_none_or(|&Reverse((when, _))| deadline.when() < when);
        queue.deadlines.push(Reverse((deadline.when(), id)));
        queue.wakers.insert(id, waker.clone());
        if soonest {
            wakeups.changed.notify_one();
        }
    }
    Wakeup { id }
}

/// A pending wakeup from [`wake_at`], which is canceled when this is dropped.
pub struct Wakeup {
    id: u64,
}

impl Wakeup {
    /// Wakes `waker` at the deadline instead, if it has not passed.
    pub fn set_waker(&self, waker: &Waker) {
        let mut queue = Wakeups::get().queue.lock();
        if let Some(registered) = queue.wakers.get_mut(&self.id)
            && !registered.will_wake(waker)
        {
            registered.clone_from(waker);
        }
    }
}

impl Drop for Wakeup {
    fn drop(&mut self) {
        Wakeups::get().queue.lock().wakers.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::deadline::current_mono_time;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn fires_once_at_deadline() {
        let timer = Timer::new();
        let fired = Arc::new(Event::new(false));
        let count = Arc::new(AtomicU32::new(0));
        let (f, c) = (fired.clone(), count.clone());
        let start = current_mono_time();
        timer.set_oneshot(start + 10_000_000, move || {
            c.fetch_add(1, Ordering::SeqCst);
            f.signal();
        });
        assert_eq!(fired.wait(&Deadline::infinite()), Ok(()));
        assert!(current_mono_time() >= start + 10_000_000);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // It has fired, so there is nothing to cancel.
        assert!(!timer.cancel());
    }

    struct SignalingWaker(Event);

    impl std::task::Wake for SignalingWaker {
        fn wake(self: Arc<Self>) {
            self.0.signal();
        }
    }

    #[test]
    fn wakeups_wake_at_their_deadlines() {
        let (soon, late) = (
            Arc::new(SignalingWaker(Event::new(false))),
            Arc::new(SignalingWaker(Event::new(false))),
        );
        let start = current_mono_time();
        let _late = wake_at(Deadline::after_mono(10_000_000_000), &Waker::from(late.clone()));
        let _soon = wake_at(Deadline::after_mono(10_000_000), &Waker::from(soon.clone()));
        assert_eq!(soon.0.wait(&Deadline::infinite()), Ok(()));
        assert!(current_mono_time() >= start + 10_000_000);
        assert!(!late.0.is_signaled());
    }

    #[test]
    fn dropped_wakeups_do_not_wake() {
        let dropped = Arc::new(SignalingWaker(Event::new(false)));
        let replaced = Arc::new(SignalingWaker(Event::new(false)));
        let current = Arc::new(SignalingWaker(Event::new(false)));
        drop(wake_at(Deadline::after_mono(1_000_000), &Waker::from(dropped.clone())));
        let wakeup = wake_at(Deadline::after_mono(1_000_000), &Waker::from(replaced.clone()));
        wakeup.set_waker(&Waker::from(current.clone()));
        assert_eq!(current.0.wait(&Deadline::infinite()), Ok(()));
        // Both were due at about the same time, so the dropped one would have woken by now.
        assert!(!dropped.0.is_signaled());
        assert!(!replaced.0.is_signaled());
        // An infinite deadline never wakes.
        drop(wake_at(Deadline::infinite(), &Waker::from(dropped.clone())));
    }

    #[test]
    fn cancel_prevents_callback() {
        let timer = Timer::new();
        let count = Arc::new(AtomicU32::new(0));
        let c = count.clone();
        timer.set_oneshot(current_mono_time() + 50_000_000, move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert!(timer.cancel());
        assert!(!timer.cancel());
        // Re-arming after a cancel is allowed.
        let fired = Arc::new(Event::new(false));
        let f = fired.clone();
        timer.set_oneshot(current_mono_time(), move || f.signal());
        assert_eq!(fired.wait(&Deadline::infinite()), Ok(()));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
