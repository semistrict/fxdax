// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: how long this process's stores' flushes took, phase by phase, and their framings: where
//! the time commits wait to be made durable goes ([`crate::Lags`], TASK-165.9).

use std::sync::Mutex;
use std::time::Duration;

/// The phases of a flush ([`crate::Store::flush`]), in microseconds: waiting for the flush before
/// it; syncing its commits' data and directories; appending their records to the segments' logs;
/// removing the files they let go of; and removing the raw files and logs of the segments framed
/// whose commits are durable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushPhases {
    pub wait_us: u64,
    pub sync_us: u64,
    pub logs_us: u64,
    pub gone_us: u64,
    pub raw_us: u64,
}

impl FlushPhases {
    /// The whole flush's.
    pub fn total_us(&self) -> u64 {
        self.wait_us + self.sync_us + self.logs_us + self.gone_us + self.raw_us
    }

    /// Each phase, by its name.
    pub fn named(&self) -> [(&'static str, u64); 5] {
        [
            ("wait", self.wait_us),
            ("sync", self.sync_us),
            ("logs", self.logs_us),
            ("gone", self.gone_us),
            ("raw", self.raw_us),
        ]
    }

    /// The longer of each phase of `self`'s and `other`'s.
    fn max(self, other: Self) -> Self {
        Self {
            wait_us: self.wait_us.max(other.wait_us),
            sync_us: self.sync_us.max(other.sync_us),
            logs_us: self.logs_us.max(other.logs_us),
            gone_us: self.gone_us.max(other.gone_us),
            raw_us: self.raw_us.max(other.raw_us),
        }
    }
}

/// How long this process's stores' flushes and framings took: so many flushes, and so many past
/// extentd's turn, 50 ms; the phases of the longest; the longest of each phase, of any flush; and
/// so many framings, and the longest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub flushes: u64,
    pub slow_flushes: u64,
    pub slowest: FlushPhases,
    pub phase_max: FlushPhases,
    pub framings: u64,
    pub framing_max_us: u64,
}

impl Timings {
    /// A flush took `phases`.
    fn flushed(&mut self, phases: FlushPhases) {
        self.flushes += 1;
        if phases.total_us() > 50_000 {
            self.slow_flushes += 1;
        }
        if phases.total_us() > self.slowest.total_us() {
            self.slowest = phases;
        }
        self.phase_max = self.phase_max.max(phases);
    }

    /// A framing took `took`.
    fn framed(&mut self, took: Duration) {
        self.framings += 1;
        self.framing_max_us = self.framing_max_us.max(us(took));
    }
}

const NONE: FlushPhases = FlushPhases { wait_us: 0, sync_us: 0, logs_us: 0, gone_us: 0, raw_us: 0 };

/// [`Timings`] of this process's stores.
static TIMINGS: Mutex<Timings> = Mutex::new(Timings {
    flushes: 0,
    slow_flushes: 0,
    slowest: NONE,
    phase_max: NONE,
    framings: 0,
    framing_max_us: 0,
});

/// fxdax: how long this process's stores' flushes and framings took ([`Timings`]).
pub fn timings() -> Timings {
    *TIMINGS.lock().unwrap()
}

/// `took`, in microseconds.
pub(crate) fn us(took: Duration) -> u64 {
    took.as_micros() as u64
}

/// A flush of a store of this process's took `phases`.
pub(crate) fn flushed(phases: FlushPhases) {
    TIMINGS.lock().unwrap().flushed(phases);
}

/// A framing of a store of this process's took `took`.
pub(crate) fn framed(took: Duration) {
    TIMINGS.lock().unwrap().framed(took);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slowest flush's phases are kept whole, beside each phase's longest of any flush; and
    /// the longest framing.
    #[test]
    fn the_slowest_flush_keeps_its_phases() {
        let mut timings = Timings::default();
        let flushes = [
            FlushPhases { wait_us: 40_000, sync_us: 1_000, ..NONE },
            FlushPhases { sync_us: 30_000, logs_us: 30_000, ..NONE },
            FlushPhases { raw_us: 2_000, ..NONE },
        ];
        for phases in flushes {
            timings.flushed(phases);
        }
        assert_eq!((timings.flushes, timings.slow_flushes), (3, 1));
        assert_eq!(timings.slowest, flushes[1]);
        let longest = FlushPhases {
            wait_us: 40_000,
            sync_us: 30_000,
            logs_us: 30_000,
            gone_us: 0,
            raw_us: 2_000,
        };
        assert_eq!(timings.phase_max, longest);
        for ms in [3, 9, 4] {
            timings.framed(Duration::from_millis(ms));
        }
        assert_eq!((timings.framings, timings.framing_max_us), (3, 9_000));
    }
}
