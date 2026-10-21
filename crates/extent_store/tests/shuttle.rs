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
use shuttle::scheduler::{PctScheduler, RandomScheduler, ReplayScheduler};
use shuttle::sync::Mutex;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;
use shuttle::{Config, Runner};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent};

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
    view_lag: None,
    view_kept_bytes: None,
};
const FOOTING: Footing = Footing { epoch: 1, next_seq: 0, after: None, next_sequence: 0 };

/// The schedules each test runs, randomly and by PCT: some seconds; `SHUTTLE_SOAK` multiplies them.
fn runs() -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    1500 * soak
}

/// `Ok` if `holds`, or else what `failure` says went wrong.
fn check(holds: bool, failure: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(failure()) }
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

/// How the follower follows: advancing its view, and pinning an extent, or dropping its view and
/// opening another, which no view may be live between.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    Advance,
    Reopen,
}

/// One run: the writer, the closer, the uploader and a follower, then the checks.
fn run() {
    run_with(true, Follow::Advance);
}

/// One run without the uploader, which keeps frees too, so that only views keep them.
fn following() {
    run_with(false, Follow::Advance);
}

/// One run without the uploader, its follower reopening its view.
fn reopening() {
    run_with(false, Follow::Reopen);
}

fn run_with(uploading: bool, follow: Follow) {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    if uploading {
        assert!(store.upload(FOOTING, 0).unwrap());
    }
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
    let uploader = uploading.then(|| {
        let (store, dir, history, written, taken) =
            (store.clone(), dir.clone(), history.clone(), written.clone(), taken.clone());
        thread::spawn(move || {
            // The segments taken, not yet released: their last commits, and their numbers.
            let mut unreleased = std::collections::VecDeque::new();
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
                    *taken.lock().unwrap() = commit.epoch;
                }
                // As fxdax's uploader does (remote.rs): takes the segments closed, reading them
                // and their extents' own files, and only then releases the commits they hold,
                // whose frees those files may be of: here, once the commits are checked too.
                for closed in store.closed().unwrap() {
                    closed.bytes().unwrap();
                    closed.own_bytes().unwrap();
                    unreleased.push_back((closed.last(), closed.number));
                }
                let checked = *taken.lock().unwrap();
                while unreleased.front().is_some_and(|(last, _)| *last <= checked) {
                    let (last, number) = unreleased.pop_front().unwrap();
                    store.release(last).unwrap();
                    store.uploaded(number).unwrap();
                }
                if done {
                    return;
                }
                thread::yield_now();
            }
        })
    });

    // A follower (TASK-42.2): reads, through a view, every extent the commit it is of holds, as
    // the writer recorded it, though freed since; advances; and pins an extent the writer frees
    // later, which reads as it was written after the view advances past its free.
    let follower = {
        let (store, history, written) = (store.clone(), history.clone(), written.clone());
        thread::spawn(move || {
            let pinned = extent(2);
            let mut pin = None;
            let mut view = Some(store.view());
            // What it finds wrong, it panics with once its view is gone: a task panicking unwinds
            // through what it holds, whose drops take Shuttle's locks, which fail in a failed run
            // with panics that abort the process (TASK-55).
            let followed = (|| -> Result<(), String> {
                loop {
                    let current = view.as_mut().unwrap();
                    let done = written.load(Ordering::SeqCst);
                    // A view opens as of a commit as it is made: its record may come later.
                    let holds = loop {
                        if let Some(holds) = history.lock().unwrap().get(current.epoch() as usize) {
                            break holds.clone();
                        }
                        thread::yield_now();
                    };
                    for (extent, data) in &holds {
                        let mut read = vec![0; data.len()];
                        store.read_view(current, *extent, 0, &mut read).map_err(|error| {
                            format!("a view's read of {extent:?} at {}: {error:#}", current.epoch())
                        })?;
                        check(&read == data, || {
                            format!("a view's read of {extent:?} reads other data")
                        })?;
                    }
                    // An extent its commit doesn't hold, and it doesn't pin, it refuses: written
                    // since, or freed before, kept for others (the uploader) or not.
                    for sequence in 1..=5 {
                        let extent = extent(sequence);
                        let pinning = pin.is_some() && extent == pinned;
                        if holds.contains_key(&extent) || pinning {
                            continue;
                        }
                        let mut read = vec![0; BLOCK];
                        let result = store.read_view(current, extent, 0, &mut read);
                        check(
                            matches!(&result, Err(error) if error.downcast_ref::<NoExtent>().is_some()),
                            || {
                                format!(
                                    "a view at {} read {extent:?}, which its commit doesn't hold: {result:?}",
                                    current.epoch()
                                )
                            },
                        )?;
                    }
                    if follow == Follow::Advance
                        && pin.is_none()
                        && let Some(data) = holds.get(&pinned)
                    {
                        current.pin(pinned).map_err(|error| format!("pinning: {error:#}"))?;
                        pin = Some(data.clone());
                    }
                    if let Some(data) = &pin {
                        let mut read = vec![0; data.len()];
                        store.read_view(current, pinned, 0, &mut read).map_err(|error| {
                            format!(
                                "a view's read of pinned {pinned:?} at {}: {error:#}",
                                current.epoch()
                            )
                        })?;
                        check(&read == data, || {
                            format!("a view's read of pinned {pinned:?} reads other data")
                        })?;
                    }
                    if done {
                        return Ok(());
                    }
                    match follow {
                        Follow::Advance => {
                            current.advance().map_err(|error| format!("advancing: {error:#}"))?
                        }
                        Follow::Reopen => {
                            view = None;
                            thread::yield_now();
                            view = Some(store.view());
                        }
                    }
                    thread::yield_now();
                }
            })();
            drop(view);
            if let Err(failure) = followed {
                panic!("{failure}");
            }
        })
    };

    writer.join().unwrap();
    closer.join().unwrap();
    if let Some(uploader) = uploader {
        uploader.join().unwrap();
    }
    follower.join().unwrap();

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

/// One run of writes racing commits, as a VM's requests may: a writer writes extents, in segments
/// and in a file of its own, while a committer commits and the closer closes the open segment
/// between commits. Each extent a commit sealed holds what was written to it, and the segments
/// closed after hold what their commits sealed, in them.
fn racing() {
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(MemDir::new(), description, OPTIONS).unwrap());
    assert!(store.upload(FOOTING, 0).unwrap());
    // Extents of one block, in segments, and of four, in a file of its own.
    const WRITES: [(u64, usize); 3] = [(1, 1), (2, 4), (3, 1)];
    let writer = {
        let store = store.clone();
        thread::spawn(move || {
            for (sequence, blocks) in WRITES {
                store.write(extent(sequence), 0, &pattern(sequence as u8, blocks * BLOCK)).unwrap();
            }
            // A write begun in a segment and abandoned, as a failed one is: commits and closings
            // waiting for it go on.
            drop(store.begin_write(extent(9), 0, BLOCK as u64).unwrap());
        })
    };
    let committer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.commit_ordered(None).expect("a write raced a commit");
            }
        })
    };
    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.close_now().expect("a write raced a commit");
                thread::yield_now();
            }
        })
    };
    for thread in [writer, committer, closer] {
        thread.join().unwrap();
    }
    store.commit(None).expect("a write raced a commit");
    store.close_segment().unwrap();
    store.commit(None).expect("a write raced a commit");
    for (sequence, blocks) in WRITES {
        let mut data = vec![0; blocks * BLOCK];
        store.read(extent(sequence), 0, &mut data).expect("a write raced a commit");
        assert!(
            data == pattern(sequence as u8, blocks * BLOCK),
            "a write raced a commit: extent {sequence} reads other data"
        );
    }
}

/// Shuttle's configuration: threads' stacks large enough for the store's (Shuttle runs each on a
/// stack of its own, of 60 KiB by default).
fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 4 << 20;
    config
}

/// Runs random schedules of `test`.
fn randomly(test: fn()) {
    Runner::new(RandomScheduler::new(runs()), config()).run(test);
}

#[test]
fn writer_closer_and_uploader_randomly() {
    set_rules(Rules::ALL);
    randomly(run);
}

#[test]
fn writer_closer_and_uploader_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(run);
}

#[test]
fn writes_racing_commits_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(racing);
}

#[test]
fn writer_and_follower_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(following);
}

#[test]
fn writer_and_follower_reopening_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(reopening);
}

/// Replays the schedule the file `SHUTTLE_REPLAY` names holds, as a failing run prints it, if set:
/// with this test's configuration, which `shuttle::replay`'s default stacks are too small for; and
/// with the rules `SHUTTLE_RULES_OFF` names (separated by commas) off, as a `needs_` test had them.
#[test]
fn replay() {
    let Some(path) = std::env::var_os("SHUTTLE_REPLAY") else { return };
    let mut rules = Rules::ALL;
    for rule in std::env::var("SHUTTLE_RULES_OFF").unwrap_or_default().split(',') {
        match rule.trim() {
            "" => {}
            "keep_frees" => rules.keep_frees = false,
            "close_waits_for_writers" => rules.close_waits_for_writers = false,
            "commit_waits_for_writes" => rules.commit_waits_for_writes = false,
            "durable_only" => rules.durable_only = false,
            "keep_frees_for_views" => rules.keep_frees_for_views = false,
            "keep_pins" => rules.keep_pins = false,
            "decide_at_publish" => rules.decide_at_publish = false,
            "views_refuse_later_extents" => rules.views_refuse_later_extents = false,
            "views_hold_their_commit" => rules.views_hold_their_commit = false,
            other => panic!("SHUTTLE_RULES_OFF names no rule {other:?}"),
        }
    }
    set_rules(rules);
    let schedule = std::fs::read_to_string(path).unwrap();
    let scheduler = ReplayScheduler::new_from_encoded(schedule.trim());
    // The scenario it is of: `SHUTTLE_SCENARIO`, by its function's name.
    let scenario = match std::env::var("SHUTTLE_SCENARIO").as_deref() {
        Ok("following") => following,
        Ok("reopening") => reopening,
        Ok("racing") => racing,
        _ => run,
    };
    Runner::new(scheduler, config()).run(scenario);
}

/// The batches of schedules a `needs_` test runs at most to find its failure, by PCT and randomly
/// in turn.
const BATCHES: usize = 8;

/// Runs the schedules with `rules`, expecting one to fail with a panic saying `expected`: the
/// violation the rule turned off prevents, not another. A failing schedule's first panic may
/// poison locks, which other threads then panic on: any of its panics may say it.
///
/// The schedules are random: a batch may find none that fails, which isn't that the rule is not
/// needed (TASK-55: one of 1,500 PCT schedules missed on Linux). Batches are run, by PCT and
/// randomly in turn, until one finds it, up to [`BATCHES`].
///
/// The panics are collected, and printed, by a hook installed for good, which Shuttle's chains
/// to: a failing run may leave the thread counted as panicking (its tasks' coroutines torn down
/// mid-panic), when the hook can't be set back. Each test is a process of its own (nextest).
fn fails_without(rules: Rules, expected: &str, test: fn()) {
    static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
    }));
    set_rules(rules);
    let failed = (0..BATCHES).any(|batch| {
        match std::panic::catch_unwind(|| {
            if batch % 2 == 0 {
                Runner::new(PctScheduler::new(3, runs()), config()).run(test);
            } else {
                Runner::new(RandomScheduler::new(runs()), config()).run(test);
            }
        }) {
            Ok(()) => false,
            // Its payload may hold Shuttle's objects (a lock's poisoned guard), which can't be
            // dropped outside a Shuttle run.
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        }
    });
    set_rules(Rules::ALL);
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    assert!(failed, "no schedule failed with {rules:?}");
    assert!(
        panics.iter().any(|panic| panic.contains(expected)),
        "with {rules:?}, expected {expected:?}, got {panics:#?}"
    );
}

#[test]
fn needs_frees_kept() {
    fails_without(Rules { keep_frees: false, ..Rules::ALL }, "no extent", run);
}

#[test]
fn needs_closes_to_wait_for_writers() {
    fails_without(Rules { close_waits_for_writers: false, ..Rules::ALL }, "reads other data", run);
}

/// Commits sealing extents writes in progress write to: what they write lands after the seal, in
/// a segment closed before the next commit names it, or lost from an extent's own file.
#[test]
fn needs_commits_to_wait_for_writes() {
    let rules = Rules { commit_waits_for_writes: false, ..Rules::ALL };
    fails_without(rules, "a write raced a commit", racing);
}

#[test]
fn needs_durable_commits_only() {
    fails_without(Rules { durable_only: false, ..Rules::ALL }, "before durable", run);
}

/// Frees not kept for views: a view's read of an extent its commit holds finds it gone, or, the
/// follower having pinned it too late, its read of it pinned.
#[test]
fn needs_frees_kept_for_views() {
    let rules = Rules { keep_frees_for_views: false, ..Rules::ALL };
    fails_without(rules, "a view's read of", following);
}

#[test]
fn needs_pins_kept() {
    fails_without(Rules { keep_pins: false, ..Rules::ALL }, "a view's read of pinned", following);
}

/// What a commit's frees keep decided apart from its publishing: a view opening between the two,
/// as of the commit before, finds an extent it holds freed and gone.
#[test]
fn needs_frees_decided_as_commits_are_published() {
    let rules = Rules { decide_at_publish: false, ..Rules::ALL };
    fails_without(rules, "a view's read of", reopening);
}

/// Views reading frees kept for others from before their commits: a follower, reading what its
/// commit doesn't hold, would take another's data for its own.
#[test]
fn needs_views_to_hold_only_their_commit() {
    let rules = Rules { views_hold_their_commit: false, ..Rules::ALL };
    fails_without(rules, "which its commit doesn't hold", following);
}

/// Views reading extents sealed after their commits: a follower's journal replay would go on past
/// its view's commit.
#[test]
fn needs_views_to_refuse_later_extents() {
    let rules = Rules { views_refuse_later_extents: false, ..Rules::ALL };
    fails_without(rules, "which its commit doesn't hold", following);
}
