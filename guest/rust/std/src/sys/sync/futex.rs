//! Futexes, as std's locks use them, over the kernel's wait queues for addresses
//! (`wait_var_event` and `wake_up_var`, guest/linux/fs/fxfs/rt.c).

use core::sync::atomic::AtomicU32;
use core::time::Duration;

/// An atomic for use as a futex.
pub type Futex = AtomicU32;
/// The underlying type of `Futex`.
pub type Primitive = u32;
/// An atomic for use as a futex that may be smaller: the same.
pub type SmallFutex = AtomicU32;
/// The underlying type of `SmallFutex`.
pub type SmallPrimitive = u32;

unsafe extern "C" {
    fn fxfs_rt_futex_wait(futex: *const u32, expected: u32, timeout_ns: u64) -> bool;
    fn fxfs_rt_futex_wake(futex: *const u32);
}

/// Sleeps until `futex_wake` or `futex_wake_all` wakes it, unless the futex doesn't hold
/// `expected`, or `timeout` passes. Returns false on a timeout, and true otherwise.
pub fn futex_wait(futex: &Futex, expected: u32, timeout: Option<Duration>) -> bool {
    let timeout_ns = timeout.map_or(u64::MAX, |timeout| {
        u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX)
    });
    // SAFETY: The futex is live for the call.
    unsafe { fxfs_rt_futex_wait(futex.as_ptr(), expected, timeout_ns) }
}

/// Wakes the tasks waiting on `futex`: all of them, which std's locks take as spurious wakeups.
/// Returns false, as it doesn't know whether it woke any.
pub fn futex_wake(futex: &Futex) -> bool {
    futex_wake_all(futex);
    false
}

/// Wakes the tasks waiting on `futex`.
pub fn futex_wake_all(futex: &Futex) {
    // SAFETY: The futex is live for the call.
    unsafe { fxfs_rt_futex_wake(futex.as_ptr()) }
}
