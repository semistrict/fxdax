// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What serving extent stores cost this process, counted since it started: the extent daemon's,
//! which it reports to whoever asks ([`crate::extentd::stats`]), for benchmarks to compare.
//! Counters only grow; a measurement is the difference of two snapshots ([`Stats::since`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A snapshot of the counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// `MAP` requests.
    pub maps: u64,
    /// The nanoseconds `MAP` requests took.
    pub map_ns: u64,
    /// Guest faults on DAX windows resolved.
    pub faults: u64,
    /// The nanoseconds the faults took.
    pub fault_ns: u64,
    /// The pager's reads of sealed extents' data into the extent cache.
    pub page_ins: u64,
    /// The bytes page-ins read from the local store.
    pub page_in_bytes: u64,
    /// The bytes page-ins fetched from object storage.
    pub fetched_bytes: u64,
    /// `READ` requests.
    pub reads: u64,
    /// The bytes `READ` requests read.
    pub read_bytes: u64,
    /// Of those, the bytes read straight from the local store or object storage: of open extents,
    /// and followers'. Sealed extents' are read through the extent cache, whose page-ins count what
    /// it reads of them (TASK-32).
    pub read_stored_bytes: u64,
    /// Units of boot profiles read into the extent cache ahead of forks, once read.
    pub prefetches: u64,
    /// The parts of extents stored compressed decompressed (`extent_store::unpacks`).
    pub unpacks: u64,
    /// The changes to a DAX window the extent daemon sent this process, a VMM's, which it made
    /// (`extentd::client`'s stage operations: maps, protections, unmaps).
    pub stage_ops: u64,
    /// The nanoseconds they took.
    pub stage_ns: u64,
}

/// The counters, in the order of [`Stats`]' fields.
#[derive(Clone, Copy)]
pub(crate) enum Counter {
    Maps,
    MapNs,
    Faults,
    FaultNs,
    PageIns,
    PageInBytes,
    FetchedBytes,
    Reads,
    ReadBytes,
    ReadStoredBytes,
    Prefetches,
    StageOps,
    StageNs,
}

/// The counters [`Counter`] names, which this module keeps.
const COUNTED: usize = 13;

/// The counters a snapshot holds: those it keeps, and the extent store's.
const COUNTERS: usize = COUNTED + 1;

static COUNTS: [AtomicU64; COUNTED] = [const { AtomicU64::new(0) }; COUNTED];

/// Adds `value` to `counter`.
pub(crate) fn add(counter: Counter, value: u64) {
    COUNTS[counter as usize].fetch_add(value, Ordering::Relaxed);
}

/// Counts an operation in `count`, and the nanoseconds since `start` in `ns`.
pub(crate) fn count_since(count: Counter, ns: Counter, start: Instant) {
    add(count, 1);
    add(ns, start.elapsed().as_nanos() as u64);
}

/// The counters as of now.
pub fn stats() -> Stats {
    let count = |counter: Counter| COUNTS[counter as usize].load(Ordering::Relaxed);
    Stats {
        maps: count(Counter::Maps),
        map_ns: count(Counter::MapNs),
        faults: count(Counter::Faults),
        fault_ns: count(Counter::FaultNs),
        page_ins: count(Counter::PageIns),
        page_in_bytes: count(Counter::PageInBytes),
        fetched_bytes: count(Counter::FetchedBytes),
        reads: count(Counter::Reads),
        read_bytes: count(Counter::ReadBytes),
        read_stored_bytes: count(Counter::ReadStoredBytes),
        prefetches: count(Counter::Prefetches),
        unpacks: extent_store::unpacks(),
        stage_ops: count(Counter::StageOps),
        stage_ns: count(Counter::StageNs),
    }
}

impl Stats {
    /// The counters, in their fields' order: how the extent daemon sends them.
    pub fn words(&self) -> [u64; COUNTERS] {
        [
            self.maps,
            self.map_ns,
            self.faults,
            self.fault_ns,
            self.page_ins,
            self.page_in_bytes,
            self.fetched_bytes,
            self.reads,
            self.read_bytes,
            self.read_stored_bytes,
            self.prefetches,
            self.unpacks,
            self.stage_ops,
            self.stage_ns,
        ]
    }

    /// The counters `words` holds, in their fields' order; those it lacks are 0.
    pub fn of_words(words: &[u64]) -> Self {
        let word = |at: usize| words.get(at).copied().unwrap_or(0);
        Self {
            maps: word(0),
            map_ns: word(1),
            faults: word(2),
            fault_ns: word(3),
            page_ins: word(4),
            page_in_bytes: word(5),
            fetched_bytes: word(6),
            reads: word(7),
            read_bytes: word(8),
            read_stored_bytes: word(9),
            prefetches: word(10),
            unpacks: word(11),
            stage_ops: word(12),
            stage_ns: word(13),
        }
    }

    /// What was counted between `earlier` and this.
    pub fn since(&self, earlier: &Self) -> Self {
        let (now, then) = (self.words(), earlier.words());
        let words: [u64; COUNTERS] = std::array::from_fn(|at| now[at].saturating_sub(then[at]));
        Self::of_words(&words)
    }

    /// The counters by the names benchmarks report them by.
    pub fn named(&self) -> [(&'static str, u64); COUNTERS] {
        let words = self.words();
        let names = [
            "maps",
            "map-ns",
            "faults",
            "fault-ns",
            "page-ins",
            "page-in-bytes",
            "fetched-bytes",
            "reads",
            "read-bytes",
            "read-stored-bytes",
            "prefetches",
            "unpacks",
            "stage-ops",
            "stage-ns",
        ];
        std::array::from_fn(|at| (names[at], words[at]))
    }
}
