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
/// its footer names; fxdax: reading its raw data, part by part, each checksummed and
/// sampled if it may be framed in place (`framing::read`); compressing them, if it is to be framed
/// in a copy (`framing::pack`); and writing its framed file, or its tail, written back as it goes
/// (TASK-336: one phase, `write`, said nothing of whether a framing's second went to the host's
/// CPUs or its disk). fxdax: its framed file is made durable after, with others framed meanwhile
/// ([`DURABLE_PHASES`], TASK-310).
pub const FRAMING_PHASES: [&str; 4] = ["own", "read", "compress", "write"];

/// fxdax: the phases of making segments framed durable together (`segments::make_durable`,
/// TASK-310): the time the first of them waited for it, framed; syncing the directories of raw
/// files and logs, if segments opened since they were; syncing their framed files; and renaming
/// them theirs and syncing the directory of framed files.
pub const DURABLE_PHASES: [&str; 4] = ["wait", "entries", "files", "dirs"];

/// Something timed, phase by phase, in microseconds: [`FLUSH_PHASES`], [`FRAMING_PHASES`] or
/// [`DURABLE_PHASES`]. fxdax: and of that time, how long the threads it ran on waited to run,
/// runnable, the host's CPUs busy with others' ([`Waits`], TASK-336): what tells a phase that took
/// long for want of a CPU from one waiting for the host's disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phases<const N: usize> {
    pub us: [u64; N],
    pub runnable_us: u64,
}

impl<const N: usize> Default for Phases<N> {
    fn default() -> Self {
        Self { us: [0; N], runnable_us: 0 }
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

    /// The longer of each phase of `self`'s and `other`'s, and of their waits to run.
    fn max(self, other: Self) -> Self {
        Self {
            us: std::array::from_fn(|phase| self.us[phase].max(other.us[phase])),
            runnable_us: self.runnable_us.max(other.runnable_us),
        }
    }

    /// Each phase of `self`'s and `other`'s, and their waits to run, summed.
    fn plus(self, other: Self) -> Self {
        Self {
            us: std::array::from_fn(|phase| self.us[phase] + other.us[phase]),
            runnable_us: self.runnable_us + other.runnable_us,
        }
    }
}

/// fxdax: how long the calling thread has waited to run, runnable, in nanoseconds, since it
/// started, as the host's scheduler counts it (Linux's `/proc/thread-self/schedstat`, its second
/// field); 0 where it doesn't (TASK-336). A read of a file of /proc's, a few microseconds: once or
/// twice per thing timed, of a segment's framing, a flush, or a making durable.
pub fn runnable_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut stat = [0u8; 96];
        let read = std::fs::File::open("/proc/thread-self/schedstat")
            .and_then(|mut file| file.read(&mut stat));
        let Ok(read) = read else { return 0 };
        let text = std::str::from_utf8(&stat[..read]).unwrap_or("");
        text.split_ascii_whitespace().nth(1).and_then(|ns| ns.parse().ok()).unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    0
}

/// fxdax: the waits to run of a thing timed ([`Phases::runnable_us`]): those of the thread timing
/// it, since it started, but for the spans it handed to others, whose own waits are added instead
/// (TASK-336).
pub(crate) struct Waits {
    since: u64,
    ns: u64,
}

impl Waits {
    /// Counting the calling thread's from now.
    pub fn start() -> Self {
        Self { since: runnable_ns(), ns: 0 }
    }

    /// The calling thread hands what it times to others, which wait `waited` nanoseconds to run
    /// while it waits for them, from `handing`, what [`Waits::handing`] returned, till now: their
    /// wait counted in its stead.
    pub fn handed(&mut self, handing: u64, waited: u64) {
        let now = runnable_ns();
        self.ns += waited;
        self.since += now.saturating_sub(handing);
    }

    /// The calling thread's count now, before it hands what it times to others
    /// ([`Waits::handed`]): its own waits so far, banked.
    pub fn handing(&mut self) -> u64 {
        let now = runnable_ns();
        self.ns += now.saturating_sub(self.since);
        self.since = now;
        now
    }

    /// In microseconds, all told, till now.
    pub fn us(mut self) -> u64 {
        self.handing();
        self.ns / 1000
    }
}

/// fxdax: something being timed on the calling thread, phase by phase, and its waits to run
/// ([`Phases`], [`Waits`]): each phase from the end of the one before (TASK-336).
pub(crate) struct Timing<const N: usize> {
    phases: Phases<N>,
    at: Instant,
    pub waits: Waits,
}

impl<const N: usize> Timing<N> {
    /// From now, or from `since`, as its first phase began.
    pub fn since(since: Instant) -> Self {
        Self { phases: Phases::default(), at: since, waits: Waits::start() }
    }

    /// Phase `phase` took from the end of the one before till now.
    pub fn took(&mut self, phase: usize) {
        self.at = self.phases.took(phase, self.at);
    }

    /// Its phases, and its waits to run, all told.
    pub fn end(self) -> Phases<N> {
        Phases { runnable_us: self.waits.us(), ..self.phases }
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
        slowest: Phases { us: [0; N], runnable_us: 0 },
        max: Phases { us: [0; N], runnable_us: 0 },
        sum: Phases { us: [0; N], runnable_us: 0 },
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
        self.sum = self.sum.plus(phases);
    }
}

/// How long this process's stores' flushes and framings took; fxdax: and making their segments
/// framed durable, so many at a time, `durable_segments` of them in all (TASK-310); and, of the
/// files flushes synced, how long the longest sync of an extent's own file, and of a segment's raw
/// file, took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub flushes: Timed<6>,
    pub framings: Timed<4>,
    pub durable: Timed<4>,
    pub durable_segments: u64,
    pub own_sync_max_us: u64,
    pub raw_sync_max_us: u64,
}

/// [`Timings`] of this process's stores.
static TIMINGS: Mutex<Timings> = Mutex::new(Timings {
    flushes: Timed::NONE,
    framings: Timed::NONE,
    durable: Timed::NONE,
    durable_segments: 0,
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
        log::warn!(
            "extent store: branch {branch:#x}'s flush of {commits} commits took {:.1} ms ({}); \
             synced {} own extents' files, the longest {:.1} ms, and {} raw files, the longest \
             {:.1} ms",
            ms(phases.total_us()),
            named(&FLUSH_PHASES, *phases),
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

/// `phases`, named by `names`, as a slow flush's or framing's warning says them, in milliseconds:
/// fxdax: and how long of them its threads waited to run (TASK-336).
fn named<const N: usize>(names: &[&str; N], phases: Phases<N>) -> String {
    let ms = |us: u64| us as f64 / 1000.0;
    let named = names.iter().zip(phases.us);
    let named: Vec<String> = named.map(|(phase, us)| format!("{phase} {:.1}", ms(us))).collect();
    format!("{} ms; runnable {:.1} ms", named.join(", "), ms(phases.runnable_us))
}

/// A framing of a store of this process's, of `segment`, took `phases`: one past extentd's turn
/// said as it happens, as a slow flush is ([`flushed`]).
pub(crate) fn framed(segment: impl std::fmt::Debug, phases: Phases<4>) {
    if phases.total_us() > 50_000 {
        log::warn!(
            "extent store: framing {segment:?} took {:.1} ms ({})",
            phases.total_us() as f64 / 1000.0,
            named(&FRAMING_PHASES, phases)
        );
    }
    TIMINGS.lock().unwrap().framings.record(phases);
}

/// fxdax: making `segments` segments of branch `branch` framed durable together took `phases`
/// (`segments::make_durable`, TASK-310): past extentd's turn, said as it happens, as a slow
/// framing is ([`framed`]).
pub(crate) fn made_durable(branch: u64, segments: usize, phases: Phases<4>) {
    if phases.total_us() > 50_000 {
        log::warn!(
            "extent store: making branch {branch:#x}'s {segments} segments framed durable took \
             {:.1} ms ({})",
            phases.total_us() as f64 / 1000.0,
            named(&DURABLE_PHASES, phases)
        );
    }
    let mut timings = TIMINGS.lock().unwrap();
    timings.durable.record(phases);
    timings.durable_segments += segments as u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slowest's phases are kept whole, beside each phase's longest of any.
    #[test]
    fn the_slowest_keeps_its_phases() {
        let mut timed = Timed::<3>::default();
        let all = [[40_000, 1_000, 0], [0, 30_000, 30_000], [0, 0, 2_000]];
        for (us, runnable_us) in all.into_iter().zip([5, 50_000, 7]) {
            timed.record(Phases { us, runnable_us });
        }
        assert_eq!((timed.count, timed.slow), (3, 1));
        assert_eq!(timed.slowest, Phases { us: all[1], runnable_us: 50_000 });
        assert_eq!(timed.max, Phases { us: [40_000, 30_000, 30_000], runnable_us: 50_000 });
        assert_eq!(timed.sum, Phases { us: [40_000, 31_000, 32_000], runnable_us: 50_012 });
    }

    /// fxdax: a thread spinning beside more spinning threads than there are CPUs it may run on
    /// waits to run, as its scheduler counts it: what a framing's warning says of the host's CPUs
    /// (TASK-336). Pinned to one CPU, with three others there, it runs about a quarter of the time.
    #[cfg(target_os = "linux")]
    #[test]
    fn waits_to_run_are_counted() {
        use std::sync::atomic::{AtomicBool, Ordering};
        assert!(std::fs::metadata("/proc/thread-self/schedstat").is_ok(), "CONFIG_SCHED_INFO");
        let pin = || {
            // SAFETY: pins the calling thread to CPU 0 of those it may run on.
            unsafe {
                let mut allowed: libc::cpu_set_t = std::mem::zeroed();
                let size = std::mem::size_of::<libc::cpu_set_t>();
                assert_eq!(libc::sched_getaffinity(0, size, &mut allowed), 0);
                let first = (0..libc::CPU_SETSIZE as usize)
                    .find(|&cpu| libc::CPU_ISSET(cpu, &allowed))
                    .expect("a CPU");
                let mut one: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_SET(first, &mut one);
                assert_eq!(libc::sched_setaffinity(0, size, &one), 0);
            }
        };
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| {
                    pin();
                    while !done.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                });
            }
            let timed = scope.spawn(|| {
                pin();
                let waits = Waits::start();
                let started = Instant::now();
                while started.elapsed() < std::time::Duration::from_millis(400) {
                    std::hint::spin_loop();
                }
                (waits.us(), started.elapsed().as_micros() as u64)
            });
            let (runnable_us, took_us) = timed.join().unwrap();
            done.store(true, Ordering::Relaxed);
            assert!(
                runnable_us > took_us / 2 && runnable_us < took_us,
                "waited {runnable_us} µs of {took_us} µs, a quarter of a CPU's"
            );
        });
    }

    /// fxdax: what a thread hands others counts their waits, not its own meanwhile (TASK-336).
    #[test]
    fn handed_spans_count_the_others_waits() {
        let mut waits = Waits::start();
        let handing = waits.handing();
        waits.handed(handing, 7_000_000);
        assert!(waits.us() >= 7_000, "the others' 7 ms counted");
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
    pub const READ: usize = 1;
    pub const COMPRESS: usize = 2;
    pub const WRITE: usize = 3;
}

/// fxdax: the indices of [`DURABLE_PHASES`].
pub(crate) mod durable {
    pub const WAIT: usize = 0;
    pub const ENTRIES: usize = 1;
    pub const FILES: usize = 2;
    pub const DIRS: usize = 3;
}
