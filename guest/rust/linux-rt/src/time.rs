//! The monotonic clock, and timers that wake futures.

use crate::ffi;
use alloc::sync::Arc;
use core::ffi::c_void;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
use core::time::Duration;
use futures_util::task::AtomicWaker;

/// The time since the kernel booted.
pub fn monotonic() -> Duration {
    // SAFETY: Reads the clock.
    Duration::from_nanos(unsafe { ffi::fxfs_rt_monotonic_ns() })
}

/// A future that is ready once a duration has passed.
pub struct Timer {
    timer: NonNull<ffi::Timer>,
    /// What the timer's callback shares, a reference to which it holds.
    shared: Arc<Shared>,
}

// SAFETY: The kernel's timers may be started and freed from any thread.
unsafe impl Send for Timer {}
// SAFETY: As above; `&Timer` gives access only to `shared`, which is `Sync`.
unsafe impl Sync for Timer {}

struct Shared {
    fired: AtomicBool,
    waker: AtomicWaker,
}

impl Timer {
    /// A timer that is ready `after` from now.
    pub fn new(after: Duration) -> Self {
        let shared = Arc::new(Shared { fired: AtomicBool::new(false), waker: AtomicWaker::new() });
        let arg = Arc::into_raw(shared.clone()).cast_mut().cast();
        // SAFETY: `fire` may be called with `arg` until `drop` frees the timer, and `drop` then
        // releases the reference `arg` holds.
        let timer = unsafe { ffi::fxfs_rt_timer_new(fire, arg) };
        let timer = NonNull::new(timer).expect("no memory for a timer");
        let ns = u64::try_from(after.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: The timer is live.
        unsafe { ffi::fxfs_rt_timer_start(timer.as_ptr(), ns) };
        Self { timer, shared }
    }
}

/// Called in a softirq when the timer's time has passed.
extern "C" fn fire(shared: *mut c_void) {
    // SAFETY: The timer holds a reference to `shared` while it may fire.
    let shared = unsafe { &*shared.cast::<Shared>() };
    shared.fired.store(true, Ordering::Release);
    shared.waker.wake();
}

impl Future for Timer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.shared.waker.register(cx.waker());
        if self.shared.fired.load(Ordering::Acquire) { Poll::Ready(()) } else { Poll::Pending }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: Cancels the timer and waits for its callback, after which nothing uses the
        // reference it held, which is released.
        unsafe {
            ffi::fxfs_rt_timer_free(self.timer.as_ptr());
            drop(Arc::from_raw(Arc::as_ptr(&self.shared)));
        }
    }
}
