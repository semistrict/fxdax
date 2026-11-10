// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what the daemon's process holds and has spent, as its kernel counts it, against what its
//! design bounds it by (TASK-330.2): its anonymous memory, what a host's OOM killer weighs it by
//! (TASK-317: 5-6 GB under an upload backlog, past any budget, until the killer took it, and every
//! VM it served), sampled as its stats are, every [`super::stores`]' poll, and its most; its shared
//! memory, the extent cache's pages, which `MEMORY_BYTES` bounds; and its CPU time. The stats
//! line's `process:` says them ([`line`]), and the metrics.
//!
//! Its anonymous memory's budget ([`AnonBudget`]) is the sum of what the daemon holds by design,
//! each part bounded where it is held (doc-7, Memory): its uploads', its framings', its fetches',
//! its tenants' rooms' and its own. A sample past it is memory none of those holds: a copy no
//! budget counts (TASK-317's per-try copy of each upload), what the allocator kept of what was
//! freed (TASK-317's glibc arenas), a leak. Each sample is weighed against the budget as of then,
//! [`over-budget`](Sample::over_budget) counting those past it; a soak's or a test's assertion
//! that none were is what holds the daemon to its design.

use std::sync::Mutex;

/// What the daemon holds and spent as of its last sample, and the most of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// Its CPU time, user and system, in nanoseconds.
    pub cpu_ns: u64,
    /// Its anonymous memory resident, in bytes (`RssAnon`).
    pub anon: u64,
    /// The most of it sampled.
    pub max_anon: u64,
    /// Its shared memory resident, the extent cache's pages that it maps (`RssShmem`).
    pub shmem: u64,
    /// The most of it sampled.
    pub max_shmem: u64,
    /// Its anonymous memory's budget as of the sample ([`AnonBudget`]).
    pub anon_budget: u64,
    /// The samples whose anonymous memory was past their budget.
    pub over_budget: u64,
    /// By how much, in bytes, at most.
    pub max_over_budget: u64,
}

/// The parts of the daemon's anonymous memory, each bounded where it is held: what its design
/// says the daemon holds (doc-7, Memory). Not its extent cache's, which is shared memory
/// (`physmem`'s memfd), bounded by `MEMORY_BYTES`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnonBudget {
    /// Its uploads': its upload memory, `FXDAX_EXTENTD_UPLOAD_MEMORY` (`crate::remote::UPLOADS`),
    /// every store's PUTs holding their objects within it, once each however often each is tried.
    pub uploads: u64,
    /// Its framings': [`FRAMING`] for each store written at once, the most so far.
    pub framings: u64,
    /// Its fetches': [`FETCHES`].
    pub fetches: u64,
    /// Its tenants' rooms': the most their requests' stream data and what the daemon queued for
    /// their VM processes held at once, so far (`super::memory::InFlight`), each tenant's within
    /// its room, as the protocol bounds it (TASK-149.35).
    pub rooms: u64,
    /// Its own: [`OWN`].
    pub own: u64,
}

impl AnonBudget {
    /// Its parts together: the budget.
    pub fn total(&self) -> u64 {
        self.uploads + self.framings + self.fetches + self.rooms + self.own
    }
}

/// What a store's framing and sealing hold at once, at most (TASK-330.2), from how they hold it.
/// A store frames one closed segment at a time (`Store::frame_behind`), reading its data whole,
/// at most a segment and an extent (`Options::segment_size` and `max_extent`: a segment closes
/// past its size once it holds an extent that doesn't fit), into a buffer, and framing it into
/// another, as long again and its tail (`extent_frame::framed`), which grows by doubling, but is
/// resident, as this budget counts it, only as far as it is written: twice that, TASK-325's bug,
/// which framing part by part would bound to a few parts.
/// Meanwhile a commit of the store's seals its extents, copying one not in one run into a buffer
/// of its length, an extent at most, and framing it into its own file a chunk at a time
/// (`Store::write_own`'s `OWN_CHUNK`, 16 MiB, and its framed bytes): an extent and two chunks.
pub const FRAMING: u64 = {
    let (segment, extent, chunk) = (32 << 20, crate::ExtentStore::MAX_EXTENT, 16 << 20);
    2 * (segment + extent) + extent + 2 * chunk
};

/// What the host's fetches hold at once, at most: the GETs of extents' data in flight
/// (`crate::remote::BUDGET`, 64 MiB), the objects' tails kept (`extent_remote::fetch`'s `TAILS`,
/// 64 MiB) and the parts fetched last (its `RECENT`, 64 MiB), each bounded for the host, however
/// many of its stores read (TASK-92.21, TASK-92.22).
pub const FETCHES: u64 = 3 * (64 << 20);

/// What the daemon holds of its own, whatever it serves, at most: its threads' stacks, as deep as
/// they reached, its runtimes', its allocator's arenas' small allocations, its tables of stores,
/// connections and cached extents. TASK-317's heap profile of a daemon under an upload backlog
/// found all of its heap but uploads, framings and fetches under 10 MB at its peak; its threads,
/// some forty and one a connection, each touch tens of KiB of their stacks; and glibc keeps an
/// arena a thread, up to eight times the CPUs, each holding its small allocations: 64 MiB is
/// those four times over, a bound that a daemon growing with its connections or stores past what
/// its tables need passes.
pub const OWN: u64 = 64 << 20;

/// The budget's parts and the samples, under one lock, as the stats' thread takes them.
static STATE: Mutex<State> = Mutex::new(State {
    sample: Sample {
        cpu_ns: 0,
        anon: 0,
        max_anon: 0,
        shmem: 0,
        max_shmem: 0,
        anon_budget: 0,
        over_budget: 0,
        max_over_budget: 0,
    },
    writers: 0,
    rooms: 0,
});

struct State {
    sample: Sample,
    /// The most stores written at once so far, and the most the tenants' rooms held at once.
    writers: u64,
    rooms: u64,
}

/// The budget of the daemon's anonymous memory as of now, its parts' most so far: so that what the
/// allocator keeps of what was freed, which it uses again, is within it as it was when held.
pub fn anon_budget() -> AnonBudget {
    budget(&mut STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn budget(state: &mut State) -> AnonBudget {
    state.writers = state.writers.max(super::stores::open_count() as u64);
    state.rooms = state.rooms.max(super::memory::rooms_held());
    AnonBudget {
        uploads: crate::remote::upload_memory().budget,
        framings: state.writers * FRAMING,
        fetches: FETCHES,
        rooms: state.rooms,
        own: OWN,
    }
}

/// Samples what the daemon holds and spent now, against its budget now; and returns the sample.
pub fn sample() -> Sample {
    let (anon, shmem) = resident();
    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let parts = budget(&mut state);
    let budget = parts.total();
    let last = state.sample;
    let over = anon.saturating_sub(budget);
    state.sample = Sample {
        cpu_ns: cpu_ns(),
        anon,
        max_anon: last.max_anon.max(anon),
        shmem,
        max_shmem: last.max_shmem.max(shmem),
        anon_budget: budget,
        over_budget: last.over_budget + u64::from(over > 0),
        max_over_budget: last.max_over_budget.max(over),
    };
    if over > 0 && last.over_budget == 0 {
        log::warn!(
            "extentd: {} MiB of anonymous memory, past its budget of {} MiB: {:?}",
            anon >> 20,
            budget >> 20,
            parts,
        );
    }
    state.sample
}

/// The last sample.
pub fn last() -> Sample {
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).sample
}

/// The last sample as a line of `NAME=COUNT` after `process:`, which the daemon's stats file
/// keeps (`super::stores`): CPU time, then anonymous and shared memory in bytes, now and at most,
/// the anonymous memory's budget, and the samples past it and by how much at most.
pub fn line() -> String {
    let sample = last();
    format!(
        "process: cpu-ns={} anon={} max-anon={} shmem={} max-shmem={} anon-budget={} \
         over-budget={} max-over-budget={}",
        sample.cpu_ns,
        sample.anon,
        sample.max_anon,
        sample.shmem,
        sample.max_shmem,
        sample.anon_budget,
        sample.over_budget,
        sample.max_over_budget,
    )
}

/// The process's anonymous and shared memory resident, in bytes, as /proc/self/status says them
/// (`RssAnon`, `RssShmem`): 0 of what it doesn't say.
fn resident() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    (kib_of(&status, "RssAnon:") << 10, kib_of(&status, "RssShmem:") << 10)
}

/// The KiB `status`, as /proc/PID/status has it, says of `field`: 0 if it doesn't.
fn kib_of(status: &str, field: &str) -> u64 {
    (status.lines())
        .find_map(|line| line.strip_prefix(field))
        .and_then(|value| value.trim().strip_suffix("kB"))
        .and_then(|kib| kib.trim().parse().ok())
        .unwrap_or(0)
}

/// The process's CPU time, user and system, in nanoseconds.
fn cpu_ns() -> u64 {
    // SAFETY: getrusage writes the struct it is given, which is the caller's.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let ns = |time: libc::timeval| time.tv_sec as u64 * 1_000_000_000 + time.tv_usec as u64 * 1_000;
    ns(usage.ru_utime) + ns(usage.ru_stime)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// /proc/self/status's memory, of the fields it says, in KiB.
    #[test]
    fn residents_are_read_as_the_kernel_says_them() {
        let status = "Name:\textentd\nRssAnon:\t  123456 kB\nRssFile:\t    9 kB\nRssShmem:\t       \
                      7 kB\n";
        assert_eq!(kib_of(status, "RssAnon:"), 123_456);
        assert_eq!(kib_of(status, "RssShmem:"), 7);
        assert_eq!(kib_of(status, "VmSwap:"), 0);
        let (anon, _) = resident();
        assert!(anon > 0, "no RssAnon");
    }

    /// The budget's parts as the design has them: a store's framing two segments and their
    /// extents, an extent and two chunks; the host's fetches 192 MiB.
    #[test]
    fn the_budgets_parts_are_the_designs() {
        assert_eq!(FRAMING, 2 * (96 << 20) + (64 << 20) + (32 << 20));
        assert_eq!(FETCHES, 192 << 20);
        let (uploads, framings) = (512 << 20, 2 * FRAMING);
        let budget = AnonBudget { uploads, framings, fetches: FETCHES, rooms: 0, own: OWN };
        assert_eq!(budget.total(), (512 << 20) + 2 * FRAMING + (192 << 20) + (64 << 20));
    }
}
