// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What serving extent stores cost this process, counted since it started, and what its VM
//! processes' changes to their windows cost them, as they told it: the extent daemon's, which it
//! reports to whoever asks ([`crate::extentd::stats`]), for benchmarks to compare.
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
    /// The changes to their DAX windows the extent daemon sent its VM processes, which they made
    /// (`extentd::client`'s stage operations: maps, protections, unmaps), as they told it
    /// (`extentd::wire::STAGE_COUNTS`, TASK-91.16). An unmap and the maps a VM process makes with
    /// it at once (`dax::SecondStage::remap`), and adjacent unmaps joined, count as one.
    pub stage_ops: u64,
    /// The nanoseconds they took the VM processes.
    pub stage_ns: u64,
    /// Commits: `COMMIT` requests, and `WRITE`s' barriers and commits.
    pub commits: u64,
    /// The nanoseconds they took, all of them.
    pub commit_ns: u64,
    /// Of those, writing the open extents' data to the store.
    pub commit_write_ns: u64,
    /// The bytes of open extents' data they wrote.
    pub commit_written_bytes: u64,
    /// Of those nanoseconds, committing the store.
    pub commit_store_ns: u64,
    /// Of those, mapping the extents they sealed read-only in the window.
    pub commit_seal_ns: u64,
    /// Of those, waiting for object storage: for the upload of a durable commit, or for the
    /// uploads to come within the lag an ordered one allows.
    pub commit_wait_ns: u64,
    /// The segments stores closed and framed (`extent_store::segment_closes`).
    pub segment_closes: u64,
    /// The nanoseconds closing them took.
    pub segment_close_ns: u64,
    /// fxdax: the maps of pages the extent daemon sent its VM processes' windows
    /// (`extentd::wire::STAGE_MAP`): one for each run of consecutive pages of the kernel's physical
    /// memory that a window maps, so that more of them for as many bytes, `stage_map_bytes`, say
    /// that memory is more fragmented (TASK-108.1).
    pub stage_maps: u64,
    /// The bytes they mapped.
    pub stage_map_bytes: u64,
    /// fxdax: the VM processes' changes to their windows that took access away from pages they had
    /// installed, as they told the daemon (`extentd::wire::STAGE_COUNTS`): each flushes the host's
    /// TLBs and the hypervisor's, interrupting the guest's vCPUs (TASK-91.16). Memory installed
    /// over theirs, as a map over a range mapped makes it.
    pub stage_overwrites: u64,
    /// Theirs left out, as an unmap, or what a map anew leaves uncovered, makes it.
    pub stage_leave_outs: u64,
    /// Theirs write-protected, as a seal makes it.
    pub stage_write_protects: u64,
    /// fxdax: the extent cache's passes letting go of extents past its bound (TASK-109.2).
    pub cache_trims: u64,
    /// The nanoseconds they took, under the cache's lock.
    pub cache_trim_ns: u64,
    /// fxdax: of `faults`, those VM processes asked for as the guest's stores to pages they had
    /// installed read-only (`extentd::wire::FAULT` without `FAULT_PAGE`); the rest were on pages
    /// they hadn't installed (TASK-111.1).
    pub fault_stores: u64,
    /// Of `faults`, the stores refused, to pages the window maps read-only.
    pub fault_refusals: u64,
    /// Of `faults`, those that waited for the pager to read their pages in.
    pub fault_waits: u64,
    /// fxdax: of `commit_seal_ns`, finding or making the cached extents of the extents sealed
    /// (TASK-112.3).
    pub commit_seal_cached_ns: u64,
    /// Of those, mapping the cached extents read-only where the windows mapped the open ones.
    pub commit_seal_remap_ns: u64,
    /// Of those, handing the open extents' pages to the cached extents.
    pub commit_seal_supply_ns: u64,
    /// Of those, mapping the pages in the windows again.
    pub commit_seal_populate_ns: u64,
    /// fxdax: of the maps of pages the windows made (`stage_maps`), the runs of pages consecutive in
    /// the window and present together that they were made of: as many as the maps, a window maps
    /// short runs as its pages are short of each other; fewer, as the pages are apart in physical
    /// memory (TASK-131.1).
    pub stage_segments: u64,
    /// Of `stage_maps`, those of open extents' pages.
    pub stage_maps_open: u64,
    /// fxdax: the allocations of open extents' pages, each of a run of them not yet in memory
    /// (TASK-131.1).
    pub open_allocs: u64,
    /// Their pages.
    pub open_alloc_pages: u64,
    /// The runs of pages consecutive in physical memory they were: as many as the pages, physical
    /// memory gave them a page at a time.
    pub open_alloc_runs: u64,
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
    Commits,
    CommitNs,
    CommitWriteNs,
    CommitWrittenBytes,
    CommitStoreNs,
    CommitSealNs,
    CommitWaitNs,
    StageMaps,
    StageMapBytes,
    StageOverwrites,
    StageLeaveOuts,
    StageWriteProtects,
    CacheTrims,
    CacheTrimNs,
    FaultStores,
    FaultRefusals,
    FaultWaits,
    CommitSealCachedNs,
    CommitSealRemapNs,
    CommitSealSupplyNs,
    CommitSealPopulateNs,
    StageSegments,
    StageMapsOpen,
    OpenAllocs,
    OpenAllocPages,
    OpenAllocRuns,
}

/// The counters [`Counter`] names, which this module keeps.
const COUNTED: usize = 39;

/// The counters a snapshot holds: those it keeps, and the extent store's.
const COUNTERS: usize = COUNTED + 3;

static COUNTS: [AtomicU64; COUNTED] = [const { AtomicU64::new(0) }; COUNTED];

/// Adds `value` to `counter`.
pub(crate) fn add(counter: Counter, value: u64) {
    COUNTS[counter as usize].fetch_add(value, Ordering::Relaxed);
}

/// Counts an operation in `count`, and the nanoseconds since `start` in `ns`.
pub(crate) fn count_since(count: Counter, ns: Counter, start: Instant) {
    add(count, 1);
    add_since(ns, start);
}

/// Adds the nanoseconds since `start` to `ns`.
pub(crate) fn add_since(ns: Counter, start: Instant) {
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
        commits: count(Counter::Commits),
        commit_ns: count(Counter::CommitNs),
        commit_write_ns: count(Counter::CommitWriteNs),
        commit_written_bytes: count(Counter::CommitWrittenBytes),
        commit_store_ns: count(Counter::CommitStoreNs),
        commit_seal_ns: count(Counter::CommitSealNs),
        commit_wait_ns: count(Counter::CommitWaitNs),
        segment_closes: extent_store::segment_closes().0,
        segment_close_ns: extent_store::segment_closes().1,
        stage_maps: count(Counter::StageMaps),
        stage_map_bytes: count(Counter::StageMapBytes),
        stage_overwrites: count(Counter::StageOverwrites),
        stage_leave_outs: count(Counter::StageLeaveOuts),
        stage_write_protects: count(Counter::StageWriteProtects),
        cache_trims: count(Counter::CacheTrims),
        cache_trim_ns: count(Counter::CacheTrimNs),
        fault_stores: count(Counter::FaultStores),
        fault_refusals: count(Counter::FaultRefusals),
        fault_waits: count(Counter::FaultWaits),
        commit_seal_cached_ns: count(Counter::CommitSealCachedNs),
        commit_seal_remap_ns: count(Counter::CommitSealRemapNs),
        commit_seal_supply_ns: count(Counter::CommitSealSupplyNs),
        commit_seal_populate_ns: count(Counter::CommitSealPopulateNs),
        stage_segments: count(Counter::StageSegments),
        stage_maps_open: count(Counter::StageMapsOpen),
        open_allocs: count(Counter::OpenAllocs),
        open_alloc_pages: count(Counter::OpenAllocPages),
        open_alloc_runs: count(Counter::OpenAllocRuns),
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
            self.commits,
            self.commit_ns,
            self.commit_write_ns,
            self.commit_written_bytes,
            self.commit_store_ns,
            self.commit_seal_ns,
            self.commit_wait_ns,
            self.segment_closes,
            self.segment_close_ns,
            self.stage_maps,
            self.stage_map_bytes,
            self.stage_overwrites,
            self.stage_leave_outs,
            self.stage_write_protects,
            self.cache_trims,
            self.cache_trim_ns,
            self.fault_stores,
            self.fault_refusals,
            self.fault_waits,
            self.commit_seal_cached_ns,
            self.commit_seal_remap_ns,
            self.commit_seal_supply_ns,
            self.commit_seal_populate_ns,
            self.stage_segments,
            self.stage_maps_open,
            self.open_allocs,
            self.open_alloc_pages,
            self.open_alloc_runs,
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
            commits: word(14),
            commit_ns: word(15),
            commit_write_ns: word(16),
            commit_written_bytes: word(17),
            commit_store_ns: word(18),
            commit_seal_ns: word(19),
            commit_wait_ns: word(20),
            segment_closes: word(21),
            segment_close_ns: word(22),
            stage_maps: word(23),
            stage_map_bytes: word(24),
            stage_overwrites: word(25),
            stage_leave_outs: word(26),
            stage_write_protects: word(27),
            cache_trims: word(28),
            cache_trim_ns: word(29),
            fault_stores: word(30),
            fault_refusals: word(31),
            fault_waits: word(32),
            commit_seal_cached_ns: word(33),
            commit_seal_remap_ns: word(34),
            commit_seal_supply_ns: word(35),
            commit_seal_populate_ns: word(36),
            stage_segments: word(37),
            stage_maps_open: word(38),
            open_allocs: word(39),
            open_alloc_pages: word(40),
            open_alloc_runs: word(41),
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
            "commits",
            "commit-ns",
            "commit-write-ns",
            "commit-written-bytes",
            "commit-store-ns",
            "commit-seal-ns",
            "commit-wait-ns",
            "segment-closes",
            "segment-close-ns",
            "stage-maps",
            "stage-map-bytes",
            "stage-overwrites",
            "stage-leave-outs",
            "stage-write-protects",
            "cache-trims",
            "cache-trim-ns",
            "fault-stores",
            "fault-refusals",
            "fault-waits",
            "commit-seal-cached-ns",
            "commit-seal-remap-ns",
            "commit-seal-supply-ns",
            "commit-seal-populate-ns",
            "stage-segments",
            "stage-maps-open",
            "open-allocs",
            "open-alloc-pages",
            "open-alloc-runs",
        ];
        std::array::from_fn(|at| (names[at], words[at]))
    }
}
