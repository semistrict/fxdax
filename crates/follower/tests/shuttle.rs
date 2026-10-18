// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The protocol's scenarios under Shuttle (backlog doc-8, testing 2; TASK-42.5): the real protocol
//! over a kernel of Shuttle's primitives, with the tiny filesystem of the TLA+ spec
//! (specs/follower): a token two directories pass between (a rename, cleared from one, then set
//! in the other, so that a rename half applied shows it in neither), and files whose data a
//! write puts in a new extent, freeing the old. A writer commits; the follower applies its
//! commits in batches, mutation by mutation; readers read the token's directories in pairs,
//! fault files' extents into mappings, and copy from them in read(2)s. Then:
//!
//! - a pair read sees the token once (the seqcount);
//! - after a batch, no mapping of a file it changed is of its old extent (invalidation, after
//!   the apply);
//! - a read(2) copies from an extent its device still keeps (the grace period);
//! - a read is tried at most `TRIES` times, and once more under the mutex (the fallback).
//!
//! Each rule turned off ([`Rules`]) makes a scenario fail. The smallest scenario is checked
//! exhaustively; the larger, with the random and PCT schedulers. Every lock and atomic is a
//! scheduling point, and every yield, which only stand for work that takes a while (a read, a
//! copy): the exhaustive search's runs grow with each.

#![cfg(feature = "shuttle")]

use anyhow::Error;
use follower::shuttle_kernel::ShuttleKernel;
use follower::shuttle_testing::fails_alone;
use follower::{Follow, Protocol, Rules, set_rules};
use shuttle::future::block_on;
use shuttle::scheduler::{DfsScheduler, PctScheduler, RandomScheduler, Scheduler};
use shuttle::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use shuttle::sync::{Condvar, Mutex, RwLock};
use shuttle::{Config, Runner, thread};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A tree read's tries before it falls back to the mutex.
const TRIES: u32 = 2;

const FILES: [&str; 2] = ["f1", "f2"];

/// The directories the token is in (1) or not (0), and the files' extents.
type Tree = BTreeMap<&'static str, u64>;

fn initial() -> Tree {
    [("d1", 1), ("d2", 0), ("f1", 1), ("f2", 2)].into_iter().collect()
}

/// A commit of the writer's.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// The token moves to the other directory: cleared where it was, then set.
    Rename,
    /// A file's data goes to a new extent, and the old one is freed.
    Write(&'static str),
}

/// The writer's commits, and the store's extents: a free is kept until the follower lets go of
/// the commit before it, then released.
struct Store {
    state: Mutex<StoreState>,
    committed: Condvar,
}

struct StoreState {
    tree: Tree,
    /// The commits' mutations, and what each freed: commit `c` is at `c - 1`.
    commits: Vec<Vec<(&'static str, u64)>>,
    frees: Vec<Vec<u64>>,
    next_extent: u64,
    /// The commit the follower was as of when it last let go of one: frees up to it are released.
    view: usize,
    released: BTreeSet<u64>,
}

impl Store {
    fn new() -> Self {
        let state = StoreState {
            tree: initial(),
            commits: Vec::new(),
            frees: Vec::new(),
            next_extent: 3,
            view: 0,
            released: BTreeSet::new(),
        };
        Self { state: Mutex::new(state), committed: Condvar::new() }
    }

    fn commit(&self, op: Op) {
        let mut state = self.state.lock().unwrap();
        let (mutations, freed) = match op {
            Op::Rename => {
                let (from, to) = if state.tree["d1"] == 1 { ("d1", "d2") } else { ("d2", "d1") };
                (vec![(from, 0), (to, 1)], vec![])
            }
            Op::Write(file) => {
                let extent = state.next_extent;
                state.next_extent += 1;
                (vec![(file, extent)], vec![state.tree[file]])
            }
        };
        for (key, value) in &mutations {
            state.tree.insert(key, *value);
        }
        state.commits.push(mutations);
        state.frees.push(freed);
        self.committed.notify_all();
    }

    fn released(&self, extent: u64) -> bool {
        self.state.lock().unwrap().released.contains(&extent)
    }
}

/// What keeps the commit the follower was as of until dropped: then the frees of the commits it
/// applied since are released.
struct Previous {
    store: Arc<Store>,
    applied: usize,
}

impl Drop for Previous {
    fn drop(&mut self) {
        let mut state = self.store.state.lock().unwrap();
        for commit in state.view + 1..=self.applied {
            let freed = state.frees[commit - 1].clone();
            state.released.extend(freed);
        }
        state.view = self.applied;
    }
}

/// The follower: the tree readers read, and the commit it applied up to.
struct Follower {
    store: Arc<Store>,
    tree: Arc<Mutex<Tree>>,
    applied: AtomicUsize,
}

impl Follow for Follower {
    type Changes = BTreeSet<&'static str>;
    type Batch = (Vec<(&'static str, u64)>, usize);
    type Previous = Previous;

    async fn fetch(&self) -> Result<(Self::Batch, Self::Changes), Error> {
        let applied = self.applied.load(Ordering::SeqCst);
        let state = self.store.state.lock().unwrap();
        let state = self.store.committed.wait_while(state, |s| s.commits.len() == applied).unwrap();
        let latest = state.commits.len();
        let mutations: Vec<_> = state.commits[applied..latest].concat();
        let changes = mutations.iter().map(|(key, _)| *key).collect();
        Ok(((mutations, latest), changes))
    }

    async fn apply(&self, (mutations, latest): Self::Batch) -> Result<Previous, Error> {
        for (key, value) in mutations {
            self.tree.lock().unwrap().insert(key, value);
        }
        self.applied.store(latest, Ordering::SeqCst);
        Ok(Previous { store: self.store.clone(), applied: latest })
    }
}

/// A file's extent mapped by a fault (DAX).
#[derive(Clone, Copy, Debug)]
struct Mapping {
    file: &'static str,
    extent: u64,
}

/// The files' DAX state, which the kernel invalidates: each file's invalidate_lock, and the
/// mappings faults made.
struct Dax {
    invalidate_locks: BTreeMap<&'static str, RwLock<()>>,
    mappings: Mutex<Vec<Mapping>>,
}

impl Dax {
    fn new() -> Self {
        let invalidate_locks = FILES.iter().map(|file| (*file, RwLock::new(()))).collect();
        Self { invalidate_locks, mappings: Mutex::new(Vec::new()) }
    }

    /// Tears down the mappings of the files `changes` names, holding each one's invalidate_lock
    /// exclusively.
    fn invalidate(&self, changes: &BTreeSet<&'static str>) {
        for file in changes.iter().filter(|key| FILES.contains(key)) {
            let _invalidating = self.invalidate_locks[file].write().unwrap();
            self.mappings.lock().unwrap().retain(|mapping| mapping.file != *file);
        }
    }
}

type TestKernel = ShuttleKernel<BTreeSet<&'static str>>;

/// What readers read through, and check against: the protocol, the follower's tree, the store,
/// and the files' DAX state.
struct Readers {
    protocol: Protocol<TestKernel>,
    tree: Arc<Mutex<Tree>>,
    store: Arc<Store>,
    dax: Arc<Dax>,
}

impl Readers {
    fn new(tree: Arc<Mutex<Tree>>, store: Arc<Store>) -> Self {
        let dax = Arc::new(Dax::new());
        let invalidated = dax.clone();
        let kernel = ShuttleKernel::new(move |changes| invalidated.invalidate(changes));
        Self { protocol: Protocol::new(kernel, TRIES), tree, store, dax }
    }
}

/// What a scenario runs.
#[derive(Clone, Debug)]
struct Scenario {
    commits: Vec<Op>,
    /// Pair reads, faults and read(2)s, of each reader: one reader each.
    pairs: usize,
    faults: usize,
    copies: usize,
}

/// Runs `scenario`: the writer, the follower, and its readers.
fn run(scenario: &Scenario) {
    let store = Arc::new(Store::new());
    let tree = Arc::new(Mutex::new(initial()));
    let follower = Arc::new(Follower {
        store: store.clone(),
        tree: tree.clone(),
        applied: AtomicUsize::new(0),
    });
    let readers = Arc::new(Readers::new(tree, store.clone()));
    let mut threads = Vec::new();

    let (commits, writer) = (scenario.commits.clone(), store.clone());
    threads.push(thread::spawn(move || {
        for op in commits {
            writer.commit(op);
        }
    }));

    let total = scenario.commits.len();
    let (following, reading) = (follower.clone(), readers.clone());
    threads.push(thread::spawn(move || {
        block_on(async {
            while following.applied.load(Ordering::SeqCst) < total {
                reading.protocol.batch(&*following).await.unwrap();
                // No mapping of a file the batch changed is of its old extent.
                let tree = following.tree.lock().unwrap().clone();
                for mapping in reading.dax.mappings.lock().unwrap().iter() {
                    assert_eq!(
                        mapping.extent, tree[mapping.file],
                        "a mapping of {} survived its invalidation",
                        mapping.file
                    );
                }
            }
        })
    }));

    let reads: [(usize, fn(&Readers, usize)); 3] =
        [(scenario.pairs, pair), (scenario.faults, fault), (scenario.copies, copy)];
    for (count, read) in reads.into_iter().filter(|(count, _)| *count > 0) {
        let readers = readers.clone();
        threads.push(thread::spawn(move || {
            for index in 0..count {
                read(&readers, index);
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
}

/// A tree read, which takes a while, that counts its tries: at most `TRIES`, and once more under
/// the mutex.
async fn counted<T>(protocol: &Protocol<TestKernel>, read: impl Fn() -> T) -> T {
    let tries = AtomicU32::new(0);
    let value = protocol
        .read(|| {
            let tries = tries.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(tries <= TRIES + 1, "a read was tried {tries} times");
            let value = read();
            thread::yield_now();
            async move { value }
        })
        .await;
    value
}

/// Reads the token's directories, one after the other: it is in one of them.
fn pair(readers: &Readers, _: usize) {
    let (d1, d2) = block_on(readers.protocol.read(|| async {
        let d1 = readers.tree.lock().unwrap()["d1"];
        let d2 = readers.tree.lock().unwrap()["d2"];
        (d1, d2)
    }));
    assert_eq!(d1 + d2, 1, "a reader saw a rename half applied: d1 {d1}, d2 {d2}");
}

/// Faults a file's extent into a mapping, holding its invalidate_lock shared throughout.
fn fault(readers: &Readers, index: usize) {
    let file = FILES[index % FILES.len()];
    let _faulting = readers.dax.invalidate_locks[file].read().unwrap();
    let extent = block_on(counted(&readers.protocol, || readers.tree.lock().unwrap()[file]));
    readers.dax.mappings.lock().unwrap().push(Mapping { file, extent });
}

/// A read(2): copies from a file's extent, which it found inside its grace period read section.
/// The copy takes a while.
fn copy(readers: &Readers, index: usize) {
    let file = FILES[index % FILES.len()];
    let _section = readers.protocol.grace_section();
    let extent = block_on(counted(&readers.protocol, || readers.tree.lock().unwrap()[file]));
    for _ in 0..3 {
        thread::yield_now();
    }
    let released = readers.store.released(extent);
    assert!(!released, "a read(2) of {file} copied from extent {extent}, released");
}

/// The smallest scenario's follower: its one batch renames, and frees nothing. Its model is as
/// small as it gets, for the exhaustive search: its runs grow with every scheduling point.
struct Renaming {
    tree: Arc<Mutex<Tree>>,
}

impl Follow for Renaming {
    type Changes = BTreeSet<&'static str>;
    type Batch = ();
    type Previous = ();

    async fn fetch(&self) -> Result<((), Self::Changes), Error> {
        Ok(((), ["d1", "d2"].into_iter().collect()))
    }

    async fn apply(&self, (): ()) -> Result<(), Error> {
        self.tree.lock().unwrap().insert("d1", 0);
        self.tree.lock().unwrap().insert("d2", 1);
        Ok(())
    }
}

/// The smallest: the follower's one batch, a rename, against one pair read.
fn smallest() {
    let tree = Arc::new(Mutex::new(initial()));
    let follower = Renaming { tree: tree.clone() };
    let readers = Arc::new(Readers::new(tree, Arc::new(Store::new())));
    let reader = {
        let readers = readers.clone();
        thread::spawn(move || pair(&readers, 0))
    };
    block_on(readers.protocol.batch(&follower)).unwrap();
    reader.join().unwrap();
}

/// Renames and writes, against pair reads, faults and read(2)s.
fn mixed() -> Scenario {
    let commits =
        vec![Op::Rename, Op::Write("f1"), Op::Rename, Op::Write("f2"), Op::Write("f1"), Op::Rename];
    Scenario { commits, pairs: 3, faults: 3, copies: 3 }
}

/// A busy writer, and a reader of a file whose tries are counted.
fn busy() -> Scenario {
    let commits = vec![Op::Write("f1"); 8];
    Scenario { commits, pairs: 0, faults: 0, copies: 6 }
}

fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 1 << 20;
    config
}

/// The runs each randomized test makes; `SHUTTLE_SOAK` multiplies them.
fn runs() -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    1000 * soak
}

fn check(scheduler: impl Scheduler + 'static, scenario: impl Fn() + Send + Sync + 'static) {
    Runner::new(scheduler, config()).run(scenario);
}

/// `scenario`, run.
fn running(scenario: Scenario) -> impl Fn() + Send + Sync + 'static {
    move || run(&scenario)
}

#[test]
fn smallest_exhaustively() {
    check(DfsScheduler::new(None, false), smallest);
}

/// Reads while no batch applies take the follower's mutex never: the fallback is for reads a
/// batch overtook.
#[test]
fn reads_without_batches_take_no_lock() {
    check(RandomScheduler::new(runs()), || {
        let tree = Arc::new(Mutex::new(initial()));
        let readers = Readers::new(tree, Arc::new(Store::new()));
        for index in 0..4 {
            pair(&readers, index);
            copy(&readers, index);
        }
        assert_eq!(readers.protocol.kernel().fallbacks(), 0, "reads took the mutex");
    });
}

#[test]
fn mixed_randomly() {
    check(RandomScheduler::new(runs()), running(mixed()));
}

#[test]
fn mixed_by_pct() {
    check(PctScheduler::new(3, runs()), running(mixed()));
}

#[test]
fn busy_randomly() {
    check(RandomScheduler::new(runs()), running(busy()));
}

/// PCT favours some threads for long: the writer and the follower, over a reader, at times.
#[test]
fn busy_by_pct() {
    check(PctScheduler::new(3, runs()), running(busy()));
}

/// Runs `scenario` with `rules`, expecting a schedule to fail with a panic saying `expected`, in
/// a process of its own (`fails_alone`).
fn fails_without(
    rules: Rules,
    expected: &str,
    scheduler: impl Scheduler + 'static,
    scenario: impl Fn() + Send + Sync + 'static,
) {
    fails_alone(expected, &[], || {
        set_rules(rules);
        check(scheduler, scenario);
    });
}

/// Without the seqcount's check, a pair read sees a rename half applied, as the smallest
/// scenario's exhaustive search finds.
#[test]
fn needs_reads_validated() {
    let rules = Rules { validate: false, ..Rules::ALL };
    fails_without(rules, "half applied", DfsScheduler::new(None, false), smallest);
}

/// Without the fallback, a read under a busy writer is tried again and again: under PCT, a reader
/// favoured over the follower spins without end while a batch applies, which Shuttle's bound on a
/// run's steps stops.
#[test]
fn needs_the_fallback() {
    let rules = Rules { fallback: false, ..Rules::ALL };
    fails_without(rules, "exceeded max_steps", PctScheduler::new(3, runs()), running(busy()));
}

/// Invalidating before the apply, a fault that read the old extent maps it after.
#[test]
fn needs_invalidation_after_the_apply() {
    let rules = Rules { invalidate_after_apply: false, ..Rules::ALL };
    fails_without(
        rules,
        "survived its invalidation",
        RandomScheduler::new(runs()),
        running(mixed()),
    );
}

/// Letting go of the commit without a grace period, a read(2) copies from an extent released: when
/// a whole batch lands while it copies, which PCT, preempting the reader, finds.
#[test]
fn needs_the_grace_period() {
    let rules = Rules { grace: false, ..Rules::ALL };
    fails_without(rules, "released", PctScheduler::new(3, runs()), running(mixed()));
}
