// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: what an extent store's commits, and the requests behind them, cost as its branch
//! grows, and while its segments close between commits: what the 2026-10-21 review's performance
//! issues were measured by, before and after their changes. Say:
//!
//! ```sh
//! cargo run --release -p extent_store --example store-bench -- CASE [N]
//! ```
//!
//! CASE is one of:
//!
//! - `lagging-view`: N commits (20,000), each writing an extent and freeing the one before, with a
//!   view that never advances, which keeps their frees: the time of each thousand commits, as the
//!   view lags more (TASK-92.29).
//! - `segments`: a branch of N closed segments (2,000), each holding an extent: the time of a
//!   thousand commits after them, and of opening the store, and the bytes opening it reads
//!   (TASK-92.16).
//! - `close-now`: a writer writing extents of 1 MiB and closing the open segment between commits
//!   each N MiB (32), as fxdax's uploader has the store do, while a guest writes a block and
//!   commits, again and again, in a directory of the host's: how long the guest's writes and
//!   commits take, the longest of them, and how long each closing takes (TASK-41.1).
//! - `cleaning`: a branch of N extents of a block (400,000), in segments: with three in eight of
//!   each segment's freed, which leaves cleaning nothing to take; then with most of four segments'
//!   freed besides: whether cleaning is due, how long a pass that copies nothing holds the store,
//!   as extentd's does each time it is due, and how long a pass copying them takes (TASK-79.2).
//! - `burst`: a guest writing N MiB (256) of extents of 256 KiB after a commit, and committing,
//!   in a directory of the host's: more than a segment and an extent hold, so that the open
//!   segment fills between commits, and closes, again and again: how long the writes take, the
//!   longest, and the commit after them, and what it seals in files of their own (TASK-79.2),
//!   which its flush syncs at once (TASK-92.42).
//! - `durable`: a guest's durable commits, a hundred of them one after another, each sealing N
//!   extents (2) in files of their own, in a directory of the host's: how long each commit's flush
//!   takes, which syncs their files and their directory, as many at once as there are of each,
//!   up to sixteen; and with N 1, one at a time (TASK-92.42).
//!
//! The stores of all but `close-now`, `burst` and `durable` are in memory ([`MemDir`]), so that
//! what is timed is the store's own work.

use extent_store::{Description, Dir, HostDir, MemDir, Options, Store};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const BRANCH: u64 = 1;

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
}

fn create<D: Dir>(dir: D, options: Options) -> Store<D> {
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 30, branch: BRANCH };
    Store::create(dir, description, options).unwrap()
}

/// Bytes that compress, as a filesystem's do, differing by `seed`.
fn data(seed: u64, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed as u8 ^ (i / BLOCK) as u8).collect()
}

/// Microseconds each of `count` operations took, which took `elapsed` together.
fn each(elapsed: Duration, count: u64) -> f64 {
    elapsed.as_secs_f64() * 1e6 / count as f64
}

/// Commits `store`, writing `extent` first, and freeing `freed` if any, and returns how long the
/// commit took: ordered, as its flush, each hundredth commit, is not timed, the store in memory
/// copying each file it syncs whole.
fn commit(store: &Store<MemDir>, extent: ExtentId, freed: Option<ExtentId>) -> Duration {
    store.write(extent, 0, &data(extent.sequence, BLOCK)).unwrap();
    if let Some(freed) = freed {
        store.free(freed).unwrap();
    }
    let started = Instant::now();
    store.commit_ordered(None).unwrap();
    let elapsed = started.elapsed();
    if extent.sequence.is_multiple_of(100) {
        store.flush().unwrap();
    }
    elapsed
}

/// `commits` commits, each writing an extent and freeing the one before, with a view that never
/// advances if `viewing`: the time each takes, by five thousands.
fn commits_freeing(commits: u64, viewing: bool) {
    let store = create(MemDir::new(), Options::default());
    commit(&store, extent(0), None);
    let view = viewing.then(|| store.view());
    let mut elapsed = Duration::ZERO;
    for sequence in 1..=commits {
        elapsed += commit(&store, extent(sequence), Some(extent(sequence - 1)));
        if sequence.is_multiple_of(5000) {
            let first = sequence - 4999;
            println!("commits {first:>6} to {sequence:>6}: {:8.1} µs each", each(elapsed, 5000));
            elapsed = Duration::ZERO;
        }
    }
    let stats = store.view_stats();
    let kept = store.usage().unwrap().kept_for_views;
    println!("views: {}, {} commits behind, keeping {kept} bytes", stats.views, stats.behind);
    drop(view);
}

fn lagging_view(commits: u64) {
    println!("with no view:");
    commits_freeing(commits, false);
    println!("with a view that never advances:");
    commits_freeing(commits, true);
}

fn segments(count: u64) {
    let dir = MemDir::new();
    let options = Options::default();
    let store = create(dir.clone(), options);
    for sequence in 0..count {
        store.write(extent(sequence), 0, &data(sequence, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    println!("{} segments", store.usage().unwrap().segments);
    const COMMITS: u64 = 1000;
    let elapsed: Duration =
        (count..count + COMMITS).map(|sequence| commit(&store, extent(sequence), None)).sum();
    store.flush().unwrap();
    println!("commits after them: {:8.1} µs each", each(elapsed, COMMITS));
    drop(store);
    let before = dir.io();
    let started = Instant::now();
    let store = Store::open(dir.clone(), options).unwrap();
    let elapsed = started.elapsed();
    println!(
        "opening: {:8.1} ms, reading {} bytes",
        elapsed.as_secs_f64() * 1e3,
        dir.io().read - before.read
    );
    assert_eq!(store.extents().unwrap().len() as u64, count + COMMITS);
}

/// What the guest's requests took: how many, together, and the longest.
#[derive(Default)]
struct Waits {
    count: u64,
    total: Duration,
    longest: Duration,
}

impl Waits {
    fn add(&mut self, wait: Duration) {
        self.count += 1;
        self.total += wait;
        self.longest = self.longest.max(wait);
    }

    fn print(&self, name: &str) {
        println!(
            "{name}: {} of them, {:8.1} µs each, the longest {:8.1} ms",
            self.count,
            each(self.total, self.count.max(1)),
            self.longest.as_secs_f64() * 1e3
        );
    }
}

fn close_now(mib: u64) {
    const SEGMENTS: u64 = 8;
    const MIB: usize = 1 << 20;
    let temp = tempfile::tempdir().unwrap();
    // Segments close between commits alone.
    let options = Options { segment_size: 1 << 30, ..Options::default() };
    let store = Arc::new(create(HostDir::new(temp.path(), true).unwrap(), options));
    let done = Arc::new(AtomicBool::new(false));
    let guest = {
        let (store, done) = (store.clone(), done.clone());
        std::thread::spawn(move || {
            let (mut writes, mut commits) = (Waits::default(), Waits::default());
            let mut sequence = 1 << 32;
            while !done.load(Ordering::Relaxed) {
                let started = Instant::now();
                store.write(extent(sequence), 0, &[1; BLOCK]).unwrap();
                let written = Instant::now();
                store.commit_ordered(None).unwrap();
                writes.add(written - started);
                commits.add(written.elapsed());
                sequence += 1;
            }
            (writes, commits)
        })
    };
    let mut closings = Waits::default();
    let mut sequence = 0;
    for _ in 0..SEGMENTS {
        for _ in 0..mib {
            store.write(extent(sequence), 0, &data(sequence, MIB)).unwrap();
            store.commit_ordered(None).unwrap();
            sequence += 1;
        }
        store.flush().unwrap();
        let started = Instant::now();
        assert!(store.close_now().unwrap());
        closings.add(started.elapsed());
    }
    done.store(true, Ordering::Relaxed);
    let (writes, commits) = guest.join().unwrap();
    closings.print("closings between commits");
    writes.print("the guest's writes");
    commits.print("the guest's commits");
}

/// The freed bytes past which extentd's cleaning is due, the fraction of a segment its extents
/// must hold for cleaning to leave it be, and the most a pass copies: `fxdax::host_store`'s.
const CLEAN_PAST: u64 = 64 << 20;
const CLEAN_BELOW: f64 = 0.5;
const CLEAN_PASS: u64 = 64 << 20;

fn cleaning(count: u64) {
    let store = create(MemDir::new(), Options::default());
    for sequence in 0..count {
        store.write(extent(sequence), 0, &data(sequence, BLOCK)).unwrap();
        if sequence % 1000 == 999 {
            store.commit(None).unwrap();
        }
    }
    store.commit(None).unwrap();
    let pass = |name: &str| {
        let started = Instant::now();
        let due = store.cleaning_due(CLEAN_PAST, CLEAN_BELOW).unwrap();
        let checked = started.elapsed();
        let started = Instant::now();
        assert_eq!(store.clean_behind(CLEAN_BELOW, 0).unwrap(), 0);
        let passed = started.elapsed();
        let usage = store.usage().unwrap();
        println!(
            "{name}: {} segments, {} MiB dead; due: {due}, found in {:.1} µs; a pass copying \
             nothing: {:.1} µs",
            usage.segments,
            usage.dead_bytes >> 20,
            each(checked, 1),
            each(passed, 1),
        );
    };
    // Three blocks in eight of each segment's freed: each five eighths live.
    for sequence in (0..count).filter(|sequence| sequence % 8 < 3) {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    pass("no segment to clean");
    // Most of the first four segments' freed besides: seven eighths of each.
    let first = 4 * Options::default().segment_size / BLOCK as u64;
    for sequence in (0..first).filter(|sequence| (3..7).contains(&(sequence % 8))) {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    pass("four segments to clean");
    let started = Instant::now();
    let copied = store.clean_behind(CLEAN_BELOW, CLEAN_PASS).unwrap();
    let elapsed = started.elapsed();
    store.commit(None).unwrap();
    let usage = store.usage().unwrap();
    println!(
        "a pass: {} MiB copied in {:.1} ms; then {} segments, {} MiB dead",
        copied >> 20,
        elapsed.as_secs_f64() * 1e3,
        usage.segments,
        usage.dead_bytes >> 20
    );
}

fn burst(mib: u64) {
    const EXTENT: usize = 256 << 10;
    let temp = tempfile::tempdir().unwrap();
    let store = create(HostDir::new(temp.path(), true).unwrap(), Options::default());
    store.write(extent(0), 0, &data(0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let mut writes = Waits::default();
    for sequence in 1..=mib * 4 {
        let started = Instant::now();
        store.write(extent(sequence), 0, &data(sequence, EXTENT)).unwrap();
        writes.add(started.elapsed());
    }
    let started = Instant::now();
    store.commit_ordered(None).unwrap();
    let ordered = started.elapsed();
    store.flush().unwrap();
    let durable = started.elapsed();
    let own = std::fs::read_dir(temp.path().join(format!("extents/{BRANCH:016x}"))).unwrap();
    writes.print("writes");
    println!(
        "the commit: ordered in {:.1} ms, durable in {:.1} ms, sealing {} extents in files of \
         their own",
        ordered.as_secs_f64() * 1e3,
        durable.as_secs_f64() * 1e3,
        own.count()
    );
}

fn durable(files: u64) {
    const COMMITS: u64 = 100;
    // Each extent's one write ends where an extent takes a file of its own, the rest of it a hole,
    // so that its file is quick to write, and the flush's syncs are most of what it takes.
    const WRITE: usize = 64 << 10;
    let temp = tempfile::tempdir().unwrap();
    let options = Options::default();
    let store = create(HostDir::new(temp.path(), true).unwrap(), options);
    let mut flushes = Waits::default();
    let mut sequence = 0;
    for _ in 0..COMMITS {
        for _ in 0..files {
            sequence += 1;
            let offset = options.large_extent - WRITE as u64;
            store.write(extent(sequence), offset, &data(sequence, WRITE)).unwrap();
        }
        store.commit_ordered(None).unwrap();
        let started = Instant::now();
        store.flush().unwrap();
        flushes.add(started.elapsed());
    }
    let own = std::fs::read_dir(temp.path().join(format!("extents/{BRANCH:016x}"))).unwrap();
    flushes.print(&format!("flushes of commits sealing {files} extents in files of their own"));
    println!("{} extents in files of their own", own.count());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n = |default| args.get(2).map_or(default, |n| n.parse().expect("N is a number"));
    match args.get(1).map(String::as_str) {
        Some("lagging-view") => lagging_view(n(20_000)),
        Some("segments") => segments(n(2_000)),
        Some("close-now") => close_now(n(32)),
        Some("cleaning") => cleaning(n(400_000)),
        Some("burst") => burst(n(256)),
        Some("durable") => durable(n(2)),
        _ => {
            eprintln!(
                "usage: store-bench lagging-view|segments|close-now|cleaning|burst|durable [N]"
            );
            std::process::exit(2);
        }
    }
}
