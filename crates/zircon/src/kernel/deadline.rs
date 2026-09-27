//! Monotonic time and deadlines.
//!
//! Ported from `zircon/kernel/kernel/deadline.rs`.
//!
//! Deviation: timer slack is not modeled, since host threads wait on
//! condition variables rather than kernel timers.

use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zx_types::{ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST, zx_duration_mono_t, zx_instant_mono_t};

/// Returns the monotonic time in nanoseconds since the kernel first read it.
pub fn current_mono_time() -> zx_instant_mono_t {
    static BOOT: OnceLock<Instant> = OnceLock::new();
    let elapsed = BOOT.get_or_init(Instant::now).elapsed();
    // Offset by one so that a freshly read time is never ZX_TIME_INFINITE_PAST.
    i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX - 1) + 1
}

/// A point in monotonic time by which something should happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    when: zx_instant_mono_t,
}

impl Deadline {
    pub const fn no_slack(when: zx_instant_mono_t) -> Self {
        Self { when }
    }

    pub const fn infinite() -> Self {
        Self { when: ZX_TIME_INFINITE }
    }

    pub const fn infinite_past() -> Self {
        Self { when: ZX_TIME_INFINITE_PAST }
    }

    pub fn after_mono(after: zx_duration_mono_t) -> Self {
        Self { when: current_mono_time().saturating_add(after) }
    }

    pub const fn when(&self) -> zx_instant_mono_t {
        self.when
    }

    /// Returns how long remains until the deadline, or `None` if it is infinite.
    pub fn remaining(&self) -> Option<Duration> {
        if self.when == ZX_TIME_INFINITE {
            return None;
        }
        let remaining = self.when.saturating_sub(current_mono_time());
        Some(Duration::from_nanos(u64::try_from(remaining).unwrap_or(0)))
    }
}
