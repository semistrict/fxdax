// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the store's writer, the closer of its open segment and its uploader, run together under
//! Shuttle (awslabs/shuttle), whose scheduler chooses every interleaving of their threads at each
//! lock, condition variable and file operation, and replays a failing one from its seed
//! (TASK-44.4).
//!
//! The writer writes extents (some in segments, some large enough for files of their own), frees
//! some, and commits, ordered or durably, recording what the store holds as of each commit. The
//! closer closes the open segment between commits, as fxdax's uploader has it do. The uploader,
//! as fxdax's does, takes each durable commit, reads what it wrote (from the store, or kept for it
//! if freed since), and releases it, and takes the segments closed. Their history is checked:
//!
//! - every extent a commit wrote reads, for the uploader, as the writer wrote it, though freed
//!   since, until the uploader releases the commit;
//! - once all is done, the store holds what the writer's last commit left;
//! - the store, crashed and reopened, holds at least every commit the uploader took: it only
//!   takes durable ones.
//!
//! Each rule the store relies on for these ([`Rules`]) is turned off by a test that expects a
//! failure: a check that never fails proves nothing. Run with the `shuttle` feature:
//! `cargo nextest run -p extent_store --features shuttle --test shuttle`.

#![cfg(feature = "shuttle")]

use extent_store::{Description, Footing, MemDir, Options, Rules, Store, set_rules};
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::sync::Mutex;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;
use shuttle::{Config, Runner};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;
/// Small segments, so that commits and the closer close several, and an extent of four blocks in
/// a file of its own.
const OPTIONS: Options = Options {
    segment_size: 3 * BLOCK as u64,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 4 * BLOCK as u64,
    checkpoint_after: 4 << 10,
    open_files: 2,
};
const FOOTING: Footing = Footing { epoch: 1, next_seq: 0, after: None, next_sequence: 0 };

/// The schedules each test runs, randomly and by PCT: some seconds; `SHUTTLE_SOAK` multiplies them.
fn runs() -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    1500 * soak
}

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
}

/// Bytes each their offset modulo 251, plus `seed`.
fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed).collect()
}

/// What the store holds as of a commit: each extent's data.
type Holds = BTreeMap<ExtentId, Vec<u8>>;

/// The writer's steps: extents written (sequence, blocks), freed, commits, and a fork, which
/// commits durably.
#[derive(Clone, Copy)]
enum Step {
    Write(u64, usize),
    Free(u64),
    Commit { durable: bool },
    Fork,
}

/// The branch the writer forks.
const CHILD: u64 = BRANCH + 1;

const STEPS: &[Step] = &[
    Step::Write(1, 1),
    Step::Write(2, 4),
    Step::Commit { durable: false },
    Step::Write(3, 2),
    Step::Free(1),
    Step::Commit { durable: true },
    Step::Free(2),
    Step::Write(4, 1),
    Step::Fork,
    Step::Commit { durable: false },
    Step::Free(3),
    Step::Write(5, 1),
    Step::Commit { durable: true },
];

/// One run: the writer, the closer and the uploader, then the checks.
fn run() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    assert!(store.upload(FOOTING, 0).unwrap());
    // What the store holds as of each commit, by epoch, the writer's record.
    let history = Arc::new(Mutex::new(vec![Holds::new()]));
    let written = Arc::new(AtomicBool::new(false));
    // What the store held at the fork, which its child holds.
    let forked = Arc::new(Mutex::new(Holds::new()));

    let writer = {
        let (store, history, written, forked) =
            (store.clone(), history.clone(), written.clone(), forked.clone());
        thread::spawn(move || {
            let mut holds = Holds::new();
            for step in STEPS {
                match *step {
                    Step::Write(sequence, blocks) => {
                        let data = pattern(sequence as u8, blocks * BLOCK);
                        store.write(extent(sequence), 0, &data).unwrap();
                        holds.insert(extent(sequence), data);
                    }
                    Step::Free(sequence) => {
                        store.free(extent(sequence)).unwrap();
                        holds.remove(&extent(sequence));
                    }
                    Step::Commit { .. } | Step::Fork => {
                        match *step {
                            Step::Fork => {
                                store.fork(CHILD).unwrap();
                                *forked.lock().unwrap() = holds.clone();
                            }
                            Step::Commit { durable: true } => store.commit(None).unwrap(),
                            _ => store.commit_ordered(None).unwrap(),
                        }
                        let mut history = history.lock().unwrap();
                        assert_eq!(store.epoch().unwrap(), history.len() as u64);
                        history.push(holds.clone());
                    }
                }
            }
            written.store(true, Ordering::SeqCst);
        })
    };

    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..3 {
                store.close_now().unwrap();
                thread::yield_now();
            }
        })
    };

    // The last commit the uploader took, which the store must recover at least.
    let taken = Arc::new(Mutex::new(0));
    let uploader = {
        let (store, dir, history, written, taken) =
            (store.clone(), dir.clone(), history.clone(), written.clone(), taken.clone());
        thread::spawn(move || {
            loop {
                // Whether the writer was done before this round: then it took everything.
                let done = written.load(Ordering::SeqCst);
                for commit in store.committed().unwrap() {
                    // The writer records a commit once it returns: its record may come later.
                    let holds = loop {
                        if let Some(holds) = history.lock().unwrap().get(commit.epoch as usize) {
                            break holds.clone();
                        }
                        thread::yield_now();
                    };
                    for (extent, len) in &commit.written {
                        let mut data = vec![0; *len as usize];
                        store.read_kept(*extent, 0, &mut data).unwrap_or_else(|error| {
                            panic!("{extent:?} of commit {}: {error:#}", commit.epoch)
                        });
                        assert!(
                            Some(&data) == holds.get(extent),
                            "{extent:?} of commit {} reads other data than was written",
                            commit.epoch
                        );
                    }
                    // The host crashing now, the store recovers at least what was taken.
                    for seed in [commit.epoch, commit.epoch + 100] {
                        let crashed = Store::open(dir.crash(seed), OPTIONS).unwrap();
                        let epoch = crashed.epoch().unwrap();
                        assert!(
                            epoch >= commit.epoch,
                            "uploaded commit {} before durable: a crash recovered {epoch}",
                            commit.epoch
                        );
                    }
                    store.release(commit.epoch).unwrap();
                    *taken.lock().unwrap() = commit.epoch;
                }
                for closed in store.closed().unwrap() {
                    store.uploaded(closed.number).unwrap();
                }
                if done {
                    return;
                }
                thread::yield_now();
            }
        })
    };

    writer.join().unwrap();
    closer.join().unwrap();
    uploader.join().unwrap();

    // The store holds what the writer's last commit left, and nothing it freed.
    let holds = history.lock().unwrap().last().unwrap().clone();
    for (extent, data) in &holds {
        let mut read = vec![0; data.len()];
        store.read(*extent, 0, &mut read).unwrap();
        assert!(&read == data, "{extent:?} reads other data than was written");
    }
    let held: Vec<_> = store.extents().unwrap().into_iter().map(|(extent, _)| extent).collect();
    assert_eq!(held, holds.keys().copied().collect::<Vec<_>>());

    // The fork holds what the store held at the fork, though the store freed some since.
    let forked = forked.lock().unwrap().clone();
    let child = store.lineage().open_branch(CHILD).unwrap();
    let held: Vec<_> = child.extents().unwrap().into_iter().map(|(extent, _)| extent).collect();
    assert_eq!(held, forked.keys().copied().collect::<Vec<_>>());
    for (extent, data) in &forked {
        let mut read = vec![0; data.len()];
        child.read(*extent, 0, &mut read).unwrap();
        assert!(&read == data, "{extent:?} reads other data in the fork than was written");
    }

    // Crashed, at any of these points, the store recovers at least what the uploader took.
    let taken = *taken.lock().unwrap();
    for seed in 0..4 {
        let crashed = Store::open(dir.crash(seed), OPTIONS).unwrap();
        let epoch = crashed.epoch().unwrap();
        assert!(epoch >= taken, "crash {seed}: recovered commit {epoch}, uploaded {taken}");
    }
}

/// Shuttle's configuration: threads' stacks large enough for the store's (Shuttle runs each on a
/// stack of its own, of 60 KiB by default).
fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 4 << 20;
    config
}

/// Runs random schedules.
fn randomly() {
    Runner::new(RandomScheduler::new(runs()), config()).run(run);
}

#[test]
fn writer_closer_and_uploader_randomly() {
    set_rules(Rules::ALL);
    randomly();
}

#[test]
fn writer_closer_and_uploader_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(run);
}

/// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`: the
/// violation the rule turned off prevents, not another. A failing schedule's first panic may
/// poison locks, which other threads then panic on: any of its panics may say it.
fn fails_without(rules: Rules, expected: &str) {
    static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
    }));
    set_rules(rules);
    let failed = std::panic::catch_unwind(randomly).is_err();
    set_rules(Rules::ALL);
    std::panic::set_hook(previous);
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    assert!(failed, "no schedule failed with {rules:?}");
    assert!(
        panics.iter().any(|panic| panic.contains(expected)),
        "with {rules:?}, expected {expected:?}, got {panics:#?}"
    );
}

#[test]
fn needs_frees_kept() {
    fails_without(Rules { keep_frees: false, ..Rules::ALL }, "no extent");
}

#[test]
fn needs_closes_to_wait_for_writers() {
    fails_without(
        Rules { close_waits_for_writers: false, ..Rules::ALL },
        "reads other data than was written",
    );
}

#[test]
fn needs_durable_commits_only() {
    fails_without(Rules { durable_only: false, ..Rules::ALL }, "before durable");
}
