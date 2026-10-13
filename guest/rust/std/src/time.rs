//! Time: `core`'s `Duration`, and the kernel's monotonic and real-time clocks.

use crate::sys;
use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};
pub use core::time::*;

/// A point on the monotonic clock: the time since the kernel booted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Instant(Duration);

impl Instant {
    pub fn now() -> Self {
        // SAFETY: Reads the clock.
        Self(Duration::from_nanos(unsafe { sys::fxfs_rt_monotonic_ns() }))
    }

    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }

    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.checked_sub(earlier.0)
    }

    pub fn elapsed(&self) -> Duration {
        Instant::now().duration_since(*self)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add(duration).map(Instant)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub(duration).map(Instant)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, other: Duration) -> Instant {
        self.checked_add(other).expect("overflow adding a duration to an instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;
    fn sub(self, other: Duration) -> Instant {
        self.checked_sub(other).expect("overflow subtracting a duration from an instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

/// A point on the real-time clock.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::ZERO);

/// An error that a time was later than another.
#[derive(Clone, Debug)]
pub struct SystemTimeError(Duration);

impl SystemTimeError {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl fmt::Display for SystemTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("second time provided was later than self")
    }
}

impl core::error::Error for SystemTimeError {}

impl SystemTime {
    pub const UNIX_EPOCH: SystemTime = UNIX_EPOCH;

    pub fn now() -> Self {
        // SAFETY: Reads the clock.
        Self(Duration::from_nanos(unsafe { sys::fxfs_rt_realtime_ns() }))
    }

    pub fn duration_since(&self, earlier: SystemTime) -> Result<Duration, SystemTimeError> {
        self.0.checked_sub(earlier.0).ok_or_else(|| SystemTimeError(earlier.0 - self.0))
    }

    pub fn elapsed(&self) -> Result<Duration, SystemTimeError> {
        SystemTime::now().duration_since(*self)
    }

    pub fn checked_add(&self, duration: Duration) -> Option<SystemTime> {
        self.0.checked_add(duration).map(SystemTime)
    }

    pub fn checked_sub(&self, duration: Duration) -> Option<SystemTime> {
        self.0.checked_sub(duration).map(SystemTime)
    }
}

impl Add<Duration> for SystemTime {
    type Output = SystemTime;
    fn add(self, other: Duration) -> SystemTime {
        self.checked_add(other).expect("overflow adding a duration to a time")
    }
}

impl Sub<Duration> for SystemTime {
    type Output = SystemTime;
    fn sub(self, other: Duration) -> SystemTime {
        self.checked_sub(other).expect("overflow subtracting a duration from a time")
    }
}
