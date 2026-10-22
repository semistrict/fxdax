// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A lineage's branches, while a fork's or a deletion's I/O stalls: a branch's record written, or a
//! deletion's index files removed, stalls in its file system, as on a slow disk, for as long as a
//! scenario says ([`Stalling`]). Shuttle, whose waits that can't end are deadlocks it reports,
//! shows that the other branches go on meanwhile; and that the openings, forks, deletions and lists
//! of branches that wait for a stalled fork or deletion act on what came of it, which the `needs_`
//! tests, turning [`Rules::wait_for_forks_and_deletions`] off, show to need the wait.

use super::{config, fails_without, randomly, runs};
use anyhow::Error;
use extent_store::{Description, Dir, Lineage, MemDir, MemFile, Options, Rules, Store, set_rules};
use shuttle::Runner;
use shuttle::scheduler::PctScheduler;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::sync::{Condvar, Mutex};
use shuttle::thread;
use std::io;
use std::sync::Arc;
use storage_device::ExtentId;

/// The directory of the branches' records, as the crate's docs lay a store's directory out.
const BRANCHES: &str = "branches";

/// The path of `branch`'s record's `version`.
fn record(branch: u64, version: u64) -> String {
    format!("{BRANCHES}/{branch:016x}.{version:016x}")
}

/// The directory of `branch`'s index files.
fn index(branch: u64) -> String {
    format!("index/{branch:016x}")
}

/// The operations of a file system a scenario stalls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Create,
    SyncDir,
}

#[derive(Default)]
struct Stall {
    /// The operation and path whose next call stalls.
    at: Option<(Op, String)>,
    /// Whether a call is stalled.
    stalled: bool,
    /// What the stalled call gives, once the scenario lets it go.
    outcome: Option<io::Result<()>>,
}

/// A [`MemDir`] whose next call of an operation on a path, once armed, stalls until the scenario
/// lets it go, saying whether it fails: on Shuttle's condition variable, so that Shuttle schedules
/// the other threads meanwhile.
#[derive(Clone)]
struct Stalling {
    dir: MemDir,
    stall: Arc<(Mutex<Stall>, Condvar)>,
}

impl Stalling {
    fn new() -> Self {
        Self { dir: MemDir::new(), stall: Arc::new((Mutex::new(Stall::default()), Condvar::new())) }
    }

    /// Has the next call of `op` on `path` stall.
    fn arm(&self, op: Op, path: &str) {
        self.stall.0.lock().unwrap().at = Some((op, path.to_owned()));
    }

    /// Waits until the call armed is stalled.
    fn wait_until_stalled(&self) {
        let (stall, changed) = &*self.stall;
        let mut stall = stall.lock().unwrap();
        while !stall.stalled {
            stall = changed.wait(stall).unwrap();
        }
    }

    /// Lets the call stalled go on, giving `outcome`: if an error, it fails without doing anything.
    fn let_go(&self, outcome: io::Result<()>) {
        let (stall, changed) = &*self.stall;
        stall.lock().unwrap().outcome = Some(outcome);
        changed.notify_all();
    }

    /// Stalls a call of `op` on `path`, if armed for it, until the scenario lets it go, and gives
    /// what the scenario said.
    fn pass(&self, op: Op, path: &str) -> io::Result<()> {
        let (stall, changed) = &*self.stall;
        let mut stall = stall.lock().unwrap();
        if stall.at.as_ref().is_none_or(|(at, at_path)| (*at, at_path.as_str()) != (op, path)) {
            return Ok(());
        }
        stall.at = None;
        stall.stalled = true;
        changed.notify_all();
        while stall.outcome.is_none() {
            stall = changed.wait(stall).unwrap();
        }
        stall.stalled = false;
        stall.outcome.take().unwrap()
    }
}

impl Dir for Stalling {
    type File = MemFile;

    fn create(&self, path: &str, data: bool) -> io::Result<MemFile> {
        self.pass(Op::Create, path)?;
        self.dir.create(path, data)
    }

    fn open(&self, path: &str, data: bool) -> io::Result<MemFile> {
        self.dir.open(path, data)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.dir.remove(path)
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        self.dir.create_dir(path)
    }

    fn list(&self, path: &str) -> io::Result<Vec<String>> {
        self.dir.list(path)
    }

    fn sync_dir(&self, path: &str) -> io::Result<()> {
        self.pass(Op::SyncDir, path)?;
        self.dir.sync_dir(path)
    }

    fn barrier(&self) -> io::Result<()> {
        self.dir.barrier()
    }
}

const BLOCK: usize = 4096;
/// The store's first branch, which the others are forks of.
const PARENT: u64 = 1;
/// Its forks: one deleted, or opened for the first time, as a scenario says; the sibling, which
/// goes on meanwhile; and another, which forks or changes its capacity.
const FORK: u64 = 2;
const SIBLING: u64 = 3;
const OTHER: u64 = 4;
/// The branch `OTHER` forks to; and, first, the sibling, in a scenario of two forks to one branch.
const CHILD: u64 = 5;
/// Small segments, and a checkpoint at each commit, which asks which index files forks need.
const OPTIONS: Options = Options {
    segment_size: 2 * BLOCK as u64,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 1 << 20,
    checkpoint_after: 1,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};

/// The parent's extent its forks inherit.
fn inherited() -> ExtentId {
    ExtentId::new(PARENT, 1)
}

/// A store of the parent, closed, which wrote an extent, and forked to each of `forks`, none of
/// them opened yet.
fn forked(dir: &Stalling, forks: &[u64]) -> Arc<Lineage<Stalling>> {
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: PARENT };
    let lineage = Lineage::create(dir.clone(), description, OPTIONS).unwrap();
    let parent = lineage.open_branch(PARENT).unwrap();
    parent.write(inherited(), 0, &[1; BLOCK]).unwrap();
    for &fork in forks {
        parent.fork(fork).unwrap();
    }
    drop(parent);
    lineage
}

/// What the sibling reads once it has gone on: the extent it wrote, and the one it inherited.
type Reads = (Vec<u8>, Vec<u8>);

/// The sibling's work: it writes, frees, commits, a checkpoint each time, which asks which of its
/// index files forks need, makes its commits durable, which removes the files they let go of, and
/// cleans; then reads what it holds.
fn sibling_works(sibling: &Store<Stalling>) -> Reads {
    let (kept, freed) = (ExtentId::new(SIBLING, 1), ExtentId::new(SIBLING, 2));
    sibling.write(kept, 0, &[3; BLOCK]).unwrap();
    sibling.write(freed, 0, &[4; BLOCK]).unwrap();
    sibling.commit(None).unwrap();
    sibling.free(freed).unwrap();
    sibling.commit_ordered(None).unwrap();
    sibling.flush().unwrap();
    sibling.clean(0.9).unwrap();
    let (mut own, mut inherited_data) = (vec![0; BLOCK], vec![0; BLOCK]);
    sibling.read(kept, 0, &mut own).unwrap();
    sibling.read(inherited(), 0, &mut inherited_data).unwrap();
    (own, inherited_data)
}

/// Runs `stalled`, which stalls at `op` on `path`, on a thread of its own; meanwhile the sibling
/// works, and the parent is opened and closed; then lets `stalled` go on. Returns what `stalled`
/// gave, and what the sibling read.
fn while_stalled<R: Send + 'static>(
    dir: &Stalling,
    lineage: &Arc<Lineage<Stalling>>,
    (op, path): (Op, &str),
    stalled: impl FnOnce() -> R + Send + 'static,
) -> (R, Reads) {
    let sibling = Arc::new(lineage.open_branch(SIBLING).unwrap());
    dir.arm(op, path);
    let stalled = thread::spawn(stalled);
    dir.wait_until_stalled();
    let working = thread::spawn(move || sibling_works(&sibling));
    let opening = {
        let lineage = lineage.clone();
        thread::spawn(move || lineage.open_branch(PARENT).and_then(|parent| parent.close()))
    };
    let reads = working.join().unwrap();
    opening.join().unwrap().unwrap();
    dir.let_go(Ok(()));
    (stalled.join().unwrap(), reads)
}

/// The sibling read what it wrote and inherited.
fn assert_read(reads: Reads) {
    assert_eq!(reads, (vec![3; BLOCK], vec![1; BLOCK]), "the sibling reads other data");
}

/// A fork's deletion, stalled at `op` on `path`, while the sibling works: it stalls no other
/// branch, and is done.
fn deleting_stalled_at(op: Op, path: &str) {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[FORK, SIBLING]);
    // Opened once, it has index files of its own, which its deletion removes.
    drop(lineage.open_branch(FORK).unwrap());
    let deleting = lineage.clone();
    let (deleted, reads) = while_stalled(&dir, &lineage, (op, path), move || deleting.delete(FORK));
    deleted.unwrap();
    assert_read(reads);
    assert_eq!(lineage.branches(), [(PARENT, None), (SIBLING, Some(PARENT))]);
}

/// A fork's deletion, its record's next version written, which says it is deleted, stalled.
pub(super) fn deleting_stalled_marking() {
    deleting_stalled_at(Op::SyncDir, BRANCHES);
}

/// A fork's deletion, its index files removed, stalled.
pub(super) fn deleting_stalled_removing_its_index() {
    deleting_stalled_at(Op::SyncDir, &index(FORK));
}

/// A fork, its record written, stalled: it stalls no other branch, and is made.
pub(super) fn forking_stalled() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[SIBLING, OTHER]);
    let other = lineage.open_branch(OTHER).unwrap();
    let stall = (Op::Create, record(CHILD, 0));
    let (forked, reads) =
        while_stalled(&dir, &lineage, (stall.0, &stall.1), move || other.fork(CHILD));
    forked.unwrap();
    assert_read(reads);
    assert_eq!(
        lineage.branches(),
        [(PARENT, None), (SIBLING, Some(PARENT)), (OTHER, Some(PARENT)), (CHILD, Some(OTHER))]
    );
}

/// A fork's first opening, which records that its index holds its state, stalled: it stalls no
/// other branch, and the fork reads what it inherited.
pub(super) fn first_opening_stalled() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[FORK, SIBLING]);
    let opening = lineage.clone();
    let (opened, reads) = while_stalled(&dir, &lineage, (Op::SyncDir, BRANCHES), move || {
        let fork = opening.open_branch(FORK)?;
        let mut data = vec![0; BLOCK];
        fork.read(inherited(), 0, &mut data)?;
        Ok::<_, Error>(data)
    });
    assert_eq!(opened.unwrap(), [1; BLOCK]);
    assert_read(reads);
    assert_eq!(lineage.branches(), [(PARENT, None), (FORK, Some(PARENT)), (SIBLING, Some(PARENT))]);
}

/// A branch's capacity changed, in its record, stalled: it stalls no other branch, and is
/// changed.
pub(super) fn capacity_change_stalled() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[SIBLING, OTHER]);
    let other = lineage.open_branch(OTHER).unwrap();
    let (changed, reads) = while_stalled(&dir, &lineage, (Op::SyncDir, BRANCHES), move || {
        other.set_capacity(2 << 20)?;
        Ok::<_, Error>(other.description().capacity)
    });
    assert_eq!(changed.unwrap(), 2 << 20);
    assert_read(reads);
}

/// Openings and deletions of a branch during its deletion: they wait for it, and act on what came
/// of it. Done, the branch is gone; failed before it wrote anything, the branch is as it was, and
/// opens.
pub(super) fn openings_and_deletions_during_a_deletion() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[FORK]);
    dir.arm(Op::SyncDir, BRANCHES);
    let deleting = spawn_deletion(&lineage);
    dir.wait_until_stalled();
    let (opening, deleting_again) = (spawn_opening(&lineage), spawn_deletion(&lineage));
    dir.let_go(Ok(()));
    deleting.join().unwrap().unwrap();
    assert_eq!(opening.join().unwrap().unwrap_err(), "the store has no branch 0x2");
    assert_eq!(deleting_again.join().unwrap().unwrap_err(), "the store has no branch 0x2");

    let dir = Stalling::new();
    let lineage = forked(&dir, &[FORK]);
    dir.arm(Op::Create, &record(FORK, 1));
    let deleting = spawn_deletion(&lineage);
    dir.wait_until_stalled();
    let opening = spawn_opening(&lineage);
    dir.let_go(Err(io::Error::other("the disk failed")));
    let failed = deleting.join().unwrap().unwrap_err();
    assert_eq!(failed, format!("creating {}", record(FORK, 1)));
    opening.join().unwrap().unwrap();
}

/// The fork's deletion, on a thread of its own: what it gave, as text.
fn spawn_deletion(lineage: &Arc<Lineage<Stalling>>) -> thread::JoinHandle<Result<(), String>> {
    let lineage = lineage.clone();
    thread::spawn(move || lineage.delete(FORK).map_err(|error| error.to_string()))
}

/// The fork's opening, and its closing, on a thread of its own: what the opening gave, as text.
fn spawn_opening(lineage: &Arc<Lineage<Stalling>>) -> thread::JoinHandle<Result<(), String>> {
    let lineage = lineage.clone();
    thread::spawn(move || lineage.open_branch(FORK).map(drop).map_err(|error| error.to_string()))
}

/// Two forks to one branch, from two branches: the first stalls, writing the fork's record, and
/// fails; the second waits for it, and is made.
pub(super) fn forks_to_one_branch() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[SIBLING, OTHER]);
    let sibling = lineage.open_branch(SIBLING).unwrap();
    let other = lineage.open_branch(OTHER).unwrap();
    dir.arm(Op::Create, &record(CHILD, 0));
    let first = thread::spawn(move || sibling.fork(CHILD).map_err(|error| error.to_string()));
    dir.wait_until_stalled();
    let second = thread::spawn(move || other.fork(CHILD).map_err(|error| error.to_string()));
    dir.let_go(Err(io::Error::other("the disk failed")));
    let failed = first.join().unwrap().unwrap_err();
    assert_eq!(failed, format!("creating {}", record(CHILD, 0)));
    second.join().unwrap().unwrap();
    assert_eq!(
        lineage.branches(),
        [(PARENT, None), (SIBLING, Some(PARENT)), (OTHER, Some(PARENT)), (CHILD, Some(OTHER))]
    );
}

/// The branches listed while a fork's deletion is in flight, removing its index files, once the
/// parent's deletion is done: none, but only once the fork's is done too. So of two deletions of a
/// store's last branches at once, as [`delete_host`](extent_store::delete_host)'s, the last to be
/// done finds the store has no branches, and removes its directory, and none finds so before.
pub(super) fn listing_during_a_deletion() {
    let dir = Stalling::new();
    let lineage = forked(&dir, &[FORK]);
    // Opened once, it has index files of its own, which its deletion removes.
    drop(lineage.open_branch(FORK).unwrap());
    dir.arm(Op::SyncDir, &index(FORK));
    let deleting = spawn_deletion(&lineage);
    dir.wait_until_stalled();
    let released = Arc::new(AtomicBool::new(false));
    let listing = {
        let (lineage, released) = (lineage.clone(), released.clone());
        thread::spawn(move || {
            lineage.delete(PARENT).unwrap();
            let listed = lineage.branches();
            (listed, released.load(Ordering::SeqCst))
        })
    };
    released.store(true, Ordering::SeqCst);
    dir.let_go(Ok(()));
    deleting.join().unwrap().unwrap();
    let (listed, after) = listing.join().unwrap();
    assert!(after, "listed {listed:?} before the deletion in flight was done");
    assert_eq!(listed, []);
}

/// Runs `scenario` in schedules by PCT, which runs a thread as far as it goes before those of
/// lower priority, as a thread that waits for a fork or deletion in flight needs to before the
/// scenario lets it go; then in random ones.
fn explore(scenario: fn()) {
    Runner::new(PctScheduler::new(3, runs()), config()).run(scenario);
    randomly(scenario);
}

/// A fork's deletion, its record's next version written, or its index files removed, stalls no
/// other branch.
#[test]
fn deletions_stall_no_other_branch() {
    set_rules(Rules::ALL);
    explore(deleting_stalled_marking);
    explore(deleting_stalled_removing_its_index);
}

/// A fork, its record written, stalls no other branch.
#[test]
fn forks_stall_no_other_branch() {
    set_rules(Rules::ALL);
    explore(forking_stalled);
}

/// A fork's first opening, which records that its index holds its state, stalls no other branch.
#[test]
fn first_openings_stall_no_other_branch() {
    set_rules(Rules::ALL);
    explore(first_opening_stalled);
}

/// A branch's capacity changed, in its record, stalls no other branch.
#[test]
fn capacity_changes_stall_no_other_branch() {
    set_rules(Rules::ALL);
    explore(capacity_change_stalled);
}

#[test]
fn openings_and_deletions_wait_for_a_deletion_in_flight() {
    set_rules(Rules::ALL);
    explore(openings_and_deletions_during_a_deletion);
}

#[test]
fn forks_to_one_branch_wait_for_each_other() {
    set_rules(Rules::ALL);
    explore(forks_to_one_branch);
}

#[test]
fn lists_of_branches_wait_for_a_deletion_in_flight() {
    set_rules(Rules::ALL);
    explore(listing_during_a_deletion);
}

/// An opening or a deletion of a branch being deleted fails at once, rather than act on what came
/// of the deletion.
#[test]
fn needs_openings_and_deletions_to_wait() {
    let rules = Rules { wait_for_forks_and_deletions: false, ..Rules::ALL };
    let expected = "branch 0x2 is being forked or deleted";
    fails_without(rules, &[expected], openings_and_deletions_during_a_deletion);
}

/// A fork to a branch another fork is making fails at once, though the other then fails: only in
/// schedules where it reaches the branch's reservation before the first lets it go.
#[test]
fn needs_forks_to_wait() {
    let rules = Rules { wait_for_forks_and_deletions: false, ..Rules::ALL };
    fails_without(rules, &["branch 0x5 is being forked or deleted"], forks_to_one_branch);
}

/// A list of the branches, while a deletion is in flight, says what the deletion has yet to do: of
/// two deletions of a store's last branches at once, one removing the store's directory while the
/// other is still under way.
#[test]
fn needs_lists_of_branches_to_wait() {
    let rules = Rules { wait_for_forks_and_deletions: false, ..Rules::ALL };
    fails_without(rules, &["before the deletion in flight was done"], listing_during_a_deletion);
}
