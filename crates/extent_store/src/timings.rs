// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: how long this process's stores' flushes and framings took, phase by phase: where the
//! time commits wait to be made durable goes ([`crate::Lags`], TASK-165.9).

use std::sync::Mutex;
use std::time::Instant;

/// The phases of a flush ([`crate::Store::flush`]): waiting for the flush before it; syncing its
/// commits' data files; syncing the directories whose entries they changed; appending their
/// records to the segments' logs; removing the files they let go of; and removing the raw files
/// and logs of the segments framed whose commits are durable.
pub const FLUSH_PHASES: [&str; 6] = ["wait", "data", "dirs", "logs", "gone", "raw"];

/// The phases of a framing ([`crate::Store::framing`]): syncing the extents stored on their own
/// its footer names; reading its raw data and writing its framed file, written back as it goes;
/// syncing the framed file; and syncing the directories of segments.
pub const FRAMING_PHASES: [&str; 4] = ["own", "write", "sync", "dirs"];

/// Something timed, phase by phase, in microseconds: [`FLUSH_PHASES`] or [`FRAMING_PHASES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phases<const N: usize> {
    pub us: [u64; N],
}

impl<const N: usize> Default for Phases<N> {
    fn default() -> Self {
        Self { us: [0; N] }
    }
}

impl<const N: usize> Phases<N> {
    /// The whole's.
    pub fn total_us(&self) -> u64 {
        self.us.iter().sum()
    }

    /// Phase `phase` took from `since` till now; returns now, when the next began.
    pub(crate) fn took(&mut self, phase: usize, since: Instant) -> Instant {
        let now = Instant::now();
        self.us[phase] += (now - since).as_micros() as u64;
        now
    }

    /// The longer of each phase of `self`'s and `other`'s.
    fn max(self, other: Self) -> Self {
        Self { us: std::array::from_fn(|phase| self.us[phase].max(other.us[phase])) }
    }
}

/// What took [`Phases`]: so many, and so many past extentd's turn, 50 ms; the phases of the
/// longest; and the longest of each phase, of any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timed<const N: usize> {
    pub count: u64,
    pub slow: u64,
    pub slowest: Phases<N>,
    pub max: Phases<N>,
}

impl<const N: usize> Default for Timed<N> {
    fn default() -> Self {
        Self::NONE
    }
}

impl<const N: usize> Timed<N> {
    const NONE: Self =
        Self { count: 0, slow: 0, slowest: Phases { us: [0; N] }, max: Phases { us: [0; N] } };

    /// One took `phases`.
    fn record(&mut self, phases: Phases<N>) {
        self.count += 1;
        if phases.total_us() > 50_000 {
            self.slow += 1;
        }
        if phases.total_us() > self.slowest.total_us() {
            self.slowest = phases;
        }
        self.max = self.max.max(phases);
    }
}

/// How long this process's stores' flushes and framings took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub flushes: Timed<6>,
    pub framings: Timed<4>,
}

/// [`Timings`] of this process's stores.
static TIMINGS: Mutex<Timings> =
    Mutex::new(Timings { flushes: Timed::NONE, framings: Timed::NONE });

/// fxdax: how long this process's stores' flushes and framings took ([`Timings`]).
pub fn timings() -> Timings {
    *TIMINGS.lock().unwrap()
}

/// A flush of a store of this process's took `phases`.
pub(crate) fn flushed(phases: Phases<6>) {
    TIMINGS.lock().unwrap().flushes.record(phases);
}

/// A framing of a store of this process's took `phases`.
pub(crate) fn framed(phases: Phases<4>) {
    TIMINGS.lock().unwrap().framings.record(phases);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slowest's phases are kept whole, beside each phase's longest of any.
    #[test]
    fn the_slowest_keeps_its_phases() {
        let mut timed = Timed::<3>::default();
        let all = [[40_000, 1_000, 0], [0, 30_000, 30_000], [0, 0, 2_000]];
        for us in all {
            timed.record(Phases { us });
        }
        assert_eq!((timed.count, timed.slow), (3, 1));
        assert_eq!(timed.slowest, Phases { us: all[1] });
        assert_eq!(timed.max, Phases { us: [40_000, 30_000, 30_000] });
    }
}

/// The indices of [`FLUSH_PHASES`].
pub(crate) mod flush {
    pub const WAIT: usize = 0;
    pub const DATA: usize = 1;
    pub const DIRS: usize = 2;
    pub const LOGS: usize = 3;
    pub const GONE: usize = 4;
    pub const RAW: usize = 5;
}

/// The indices of [`FRAMING_PHASES`].
pub(crate) mod framing {
    pub const OWN: usize = 0;
    pub const WRITE: usize = 1;
    pub const SYNC: usize = 2;
    pub const DIRS: usize = 3;
}
