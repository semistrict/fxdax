//! One-shot timers that call back at a deadline.
//!
//! Ported from the interface of `Timer` in `zircon/kernel/kernel/timer.cc`.
//!
//! Deviations:
//! - Callbacks run on a host thread started for each armed timer, rather than from the timer
//!   interrupt, and are passed nothing.
//! - Only one-shot timers, `set_oneshot` and `cancel`.

use super::deadline::Deadline;
use super::event::Event;
use parking_lot::Mutex;
use std::sync::Arc;
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
        std::thread::Builder::new()
            .name("timer".into())
            .spawn(move || {
                if cancel.wait(&Deadline::no_slack(deadline)) != Err(Status::TIMED_OUT) {
                    // Canceled.
                    return;
                }
                let mut state = timer_state.lock();
                if state.armed.as_ref().is_some_and(|armed| armed.generation == generation) {
                    state.armed = None;
                    callback();
                }
            })
            .expect("create timer thread");
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
