// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: how long this process's stores' flushes and framings took, phase by phase: where the
//! time commits wait to be made durable goes ([`crate::Lags`], TASK-165.9).

use crate::durability::FileSyncs;
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
/// longest; the longest of each phase, of any; and each phase's time, summed over all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timed<const N: usize> {
    pub count: u64,
    pub slow: u64,
    pub slowest: Phases<N>,
    pub max: Phases<N>,
    pub sum: Phases<N>,
}

impl<const N: usize> Default for Timed<N> {
    fn default() -> Self {
        Self::NONE
    }
}

impl<const N: usize> Timed<N> {
    const NONE: Self = Self {
        count: 0,
        slow: 0,
        slowest: Phases { us: [0; N] },
        max: Phases { us: [0; N] },
        sum: Phases { us: [0; N] },
    };

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
        self.sum =
            Phases { us: std::array::from_fn(|phase| self.sum.us[phase] + phases.us[phase]) };
    }
}

/// How long this process's stores' flushes and framings took; and, of the files flushes synced,
/// how long the longest sync of an extent's own file, and of a segment's raw file, took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub flushes: Timed<6>,
    pub framings: Timed<4>,
    pub own_sync_max_us: u64,
    pub raw_sync_max_us: u64,
}

/// [`Timings`] of this process's stores.
static TIMINGS: Mutex<Timings> = Mutex::new(Timings {
    flushes: Timed::NONE,
    framings: Timed::NONE,
    own_sync_max_us: 0,
    raw_sync_max_us: 0,
});

/// A flush, as timed: its phases, its commits, and the files it synced.
#[derive(Default)]
pub(crate) struct Flush {
    pub phases: Phases<6>,
    pub commits: usize,
    pub synced: FileSyncs,
}

/// fxdax: how long this process's stores' flushes and framings took ([`Timings`]).
pub fn timings() -> Timings {
    *TIMINGS.lock().unwrap()
}

/// A flush of a store of this process's, `flush`, of branch `branch`, ended: one past extentd's
/// turn said as it happens, what it synced with it, which a daemon killed leaves in its log.
pub(crate) fn flushed(branch: u64, flush: &Flush) {
    let Flush { phases, commits, synced } = flush;
    if phases.total_us() > 50_000 {
        let ms = |us: u64| us as f64 / 1000.0;
        let named = FLUSH_PHASES.iter().zip(phases.us);
        let named: Vec<String> =
            named.map(|(phase, us)| format!("{phase} {:.1}", ms(us))).collect();
        log::warn!(
            "extent store: branch {branch:#x}'s flush of {commits} commits took {:.1} ms ({} ms); \
             synced {} own extents' files, the longest {:.1} ms, and {} raw files, the longest \
             {:.1} ms",
            ms(phases.total_us()),
            named.join(", "),
            synced.files,
            ms(synced.longest_file_us),
            synced.cached,
            ms(synced.longest_cached_us),
        );
    }
    let mut timings = TIMINGS.lock().unwrap();
    timings.flushes.record(*phases);
    timings.own_sync_max_us = timings.own_sync_max_us.max(synced.longest_file_us);
    timings.raw_sync_max_us = timings.raw_sync_max_us.max(synced.longest_cached_us);
}

/// A framing of a store of this process's, of `segment`, took `phases`: one past extentd's turn
/// said as it happens, as a slow flush is ([`flushed`]).
pub(crate) fn framed(segment: impl std::fmt::Debug, phases: Phases<4>) {
    if phases.total_us() > 50_000 {
        let named = FRAMING_PHASES.iter().zip(phases.us);
        let named: Vec<String> =
            named.map(|(phase, us)| format!("{phase} {:.1}", us as f64 / 1000.0)).collect();
        log::warn!(
            "extent store: framing {segment:?} took {:.1} ms ({} ms)",
            phases.total_us() as f64 / 1000.0,
            named.join(", ")
        );
    }
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
        assert_eq!(timed.sum, Phases { us: [40_000, 31_000, 32_000] });
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
