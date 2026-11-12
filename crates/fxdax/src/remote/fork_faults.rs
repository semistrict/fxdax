// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Forks with something going wrong at each of their steps (TASK-44.10), on the host, in seconds,
//! with no VM: a step failing ([`crate::faults`]), the host dying there, another branch taking the
//! child's ID there, or a write of the bucket's going wrong ([`Mishap`]); then, whatever came of
//! the fork, what [`World::settle`] checks: no data lost, no other branch harmed, nothing leaked
//! for good, and a later fork succeeding, or the same one failing cleanly.
//!
//! A fork is the bucket's alone (TASK-165.7): its child's store is made only once the bucket has
//! the child's record, so that nothing here names a child the bucket may not have. What a fork
//! whose host died, or whose cleanup failed, leaves in the bucket stands, as the bucket's protocol
//! has it (doc-14): an orphan, a live branch no store holds, which an operator finds among the live
//! branches and deletes ([`World::settle`], `fxdax-gc --branches`).
//!
//! A host dying is its disk, copied there and then, restarted, and its bucket cut off, so that what
//! it had going runs to its end unseen ([`Gated::sever`]): every write of a dead host's kept, synced
//! or not, as its file system may keep them.
//!
//! The extents are a guest's: minted from its store's next sequence number or its writer's floor,
//! which jumps at each writer, on from 1 << 40, its branches random; never counted from nothing, as
//! the extents of `remote`'s other tests are, which no store's are.

use super::tests::{Gated, Mishap, idle, kept_in, naming, pattern, reopened, restart, uploaded};
use crate::faults::{CLEANUP_STEPS, FORK_STEPS, plan};
use crate::{Errno, ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use extent_remote::layout::{self, Kind};
use extent_remote::store::Store as Bucket;
use extent_remote::{EPOCH_SHIFT, branch};
use fail::FailGuard;
use futures::executor::block_on;
use object_store::memory::InMemory;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The fencing token of the placement of this host, and of the others.
const TOKEN: u64 = 1;

pub(super) const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

/// What goes wrong in a fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// The step of this name fails, at its next pass.
    Fails(&'static str),
    /// The step of this name fails, at the pass after this many, as a seed says.
    FailsAfter(&'static str, usize),
    /// The host dies at the step of this name.
    Dies(&'static str),
    /// Another branch takes the child's ID at the step of this name, if none has it yet.
    Taken(&'static str),
    /// This goes wrong with the write numbered so, counting from the fork's first.
    Write(usize, Mishap),
}

/// What the host's control plane does with what a dead host left at a fork's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Leftover {
    /// Deletes it, as of a fork that failed.
    Deleted,
    /// Opens it, as of a fork made, if it opens; and deletes it if not.
    Opened,
}

/// The writes of a bucket's that go wrong one after another.
pub(super) const MISHAPS: [Mishap; 5] = [
    Mishap::Refused,
    Mishap::Unanswered,
    Mishap::Dies,
    Mishap::DiesAnswered,
    Mishap::DiesLandingLate,
];

/// The stores of a host, and another's, in a bucket: a parent, in `host`, reaching the bucket
/// through `gated`, holding extents two writers minted, one stored on its own; and another tenant's.
struct World {
    temp: tempfile::TempDir,
    /// Where the parent's host's disk is: once it died, its copy, restarted.
    host: PathBuf,
    bucket: Arc<Bucket>,
    gated: Arc<Gated>,
    parent: Option<ExtentStore>,
    branch: crate::Branch,
    /// The child of the fork.
    child: crate::Branch,
    /// The parent's extents, with their data.
    held: Vec<(ExtentId, Vec<u8>)>,
    other: ExtentStore,
    other_held: Vec<(ExtentId, Vec<u8>)>,
    /// Whether the host died.
    dead: Arc<AtomicBool>,
    /// Whether another branch took the child's ID.
    taken: Arc<AtomicBool>,
}

/// `store`'s next extents' IDs, `count` of them, as its guest mints them: never below its writer's
/// floor, whatever it minted before.
fn mint(store: &ExtentStore, count: u64) -> Vec<ExtentId> {
    let info = store.info().unwrap();
    let first = info.next_sequence.max(info.sequence_floor);
    assert!(first >= 1 << EPOCH_SHIFT, "a guest mints from its writer's floor: {info:?}");
    (first..first + count)
        .map(|sequence| ExtentId { branch: store.branch().id, sequence })
        .collect()
}

fn write(store: &ExtentStore, extent: ExtentId, data: &[u8]) {
    store.write(extent, 0, &[VolatileSlice::from(data)]).unwrap();
}

pub(super) fn read(store: &ExtentStore, extent: ExtentId, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    data
}

/// Options of a store of this host's or another's.
pub(super) fn options() -> extent_store::upload::Options {
    extent_store::upload::Options { token: TOKEN, ..idle() }
}

/// Mints `extents` of `store`'s, of those lengths, with data from `seed`, and commits them, the
/// root `root`: returns them.
pub(super) fn commit(
    store: &ExtentStore,
    seed: u8,
    lens: &[usize],
    root: &[u8],
) -> Vec<(ExtentId, Vec<u8>)> {
    let minted = mint(store, lens.len() as u64);
    let extents: Vec<_> = (minted.into_iter().zip(lens))
        .enumerate()
        .map(|(at, (extent, &len))| (extent, pattern(seed + at as u8, len)))
        .collect();
    for (extent, data) in &extents {
        write(store, *extent, data);
    }
    store.commit(Some(root)).unwrap();
    extents
}

/// Makes this process a host: deleting a store takes its placement's token.
pub(super) fn host() {
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe { std::env::set_var("FXDAX_EXTENTD_TOKEN", TOKEN.to_string()) };
}

impl World {
    /// A new world, the parent's second writer's commits uploaded: the sequence floor jumped.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().join("host");
        std::fs::create_dir(&host).unwrap();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let gated = Arc::new(Gated::over(memory));
        let reached = Arc::new(Bucket::new(gated.clone(), ""));

        let other =
            kept_in(temp.path().join("other"), (1 << 30, None), &bucket, options()).unwrap();
        let other_held = commit(&other, 9, &[BLOCK], b"other");

        let path = host.join("parent");
        let parent = kept_in(&path, (1 << 30, None), &reached, options()).unwrap();
        let branch = parent.branch();
        let mut held = commit(&parent, 1, &[BLOCK], b"first");
        parent.wait_uploaded().unwrap();
        let floor = parent.info().unwrap().sequence_floor;
        drop(parent);
        let parent = reopened(&path, options()).unwrap();
        assert!(parent.info().unwrap().sequence_floor > floor, "a writer's floor jumps");
        held.extend(commit(&parent, 2, &[2 * BLOCK, 4 << 20], b"root"));
        parent.wait_uploaded().unwrap();
        Self {
            temp,
            host,
            bucket,
            gated,
            parent: Some(parent),
            branch,
            child: crate::host_store::new_alone(),
            held,
            other,
            other_held,
            dead: Default::default(),
            taken: Default::default(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.host.join(name)
    }

    /// What the host's dying does: its disk, as it is, copied and restarted, then it cut off from
    /// the bucket.
    fn death(&self) -> Arc<dyn Fn() + Send + Sync> {
        let (gated, dead) = (self.gated.clone(), self.dead.clone());
        let (from, to) = (self.host.clone(), self.temp.path().join("restarted"));
        Arc::new(move || {
            if !dead.swap(true, Ordering::SeqCst) {
                restart(&from, &to);
                gated.sever();
            }
        })
    }

    /// Sets `fault` to happen, until what it returns goes.
    fn arm(&self, fault: Fault) -> Option<FailGuard> {
        let guard = match fault {
            Fault::Fails(step) => FailGuard::new(step, "1*return"),
            Fault::FailsAfter(step, passes) => {
                FailGuard::new(step, &format!("{passes}*off->1*return"))
            }
            Fault::Dies(step) => {
                let die = self.death();
                FailGuard::with_callback(step, move || die())
            }
            Fault::Taken(step) => {
                let (bucket, child, taken) = (self.bucket.clone(), self.child, self.taken.clone());
                FailGuard::with_callback(step, move || {
                    if !taken.load(Ordering::SeqCst)
                        && block_on(branch::create(&*bucket, child, 0)).is_ok()
                    {
                        taken.store(true, Ordering::SeqCst);
                        let lease = layout::lease(child);
                        let leased = branch::take_lease(&*bucket, &lease, rand::random(), TOKEN);
                        assert_eq!(block_on(leased), Ok(1));
                    }
                })
            }
            Fault::Write(write, mishap) => {
                self.gated.arm(write, mishap, self.death());
                return None;
            }
        };
        Some(guard.unwrap())
    }

    /// Forks the parent to `child`, its store at `child` in the host's disk, with `faults`
    /// happening; and lets go of the parent, whatever its uploader has left to do.
    fn fork(&mut self, faults: &[Fault]) -> Result<(), crate::Error> {
        let guards: Vec<_> = faults.iter().filter_map(|fault| self.arm(*fault)).collect();
        let forked = self.parent.as_ref().unwrap().fork(self.path("child"), self.child);
        self.parent = None;
        drop(guards);
        if self.dead.load(Ordering::SeqCst) {
            self.host = self.temp.path().join("restarted");
        }
        forked
    }

    /// One pass of the collector, deleting what is free at once.
    fn collect(&self, case: &str) {
        let options = extent_remote::collector::Options { compact_below: 0.0, grace: 0 };
        let collected = extent_remote::collector::collect(&*self.bucket, rand::random(), &options);
        block_on(collected).unwrap_or_else(|err| panic!("{case}: the collector: {err}"));
    }

    /// Opens the store at `path`, and takes its branch.
    fn open(&self, path: &Path) -> Result<ExtentStore, String> {
        naming(path, &self.bucket);
        reopened(path, options()).map_err(|err| err.to_string())
    }

    /// Whether `store`, a fork of the parent, reads what the parent held at the fork, whose root
    /// was `root`, and goes on committing, its writer's floor its own.
    fn check_fork(&self, store: &ExtentStore, root: &[u8], case: &str) {
        assert_eq!(store.root(), Ok(root.to_vec()), "{case}");
        for (extent, data) in &self.held {
            assert!(read(store, *extent, data.len()) == *data, "{case}: {extent:?} of a fork");
        }
        let own = commit(store, 5, &[BLOCK], b"child");
        assert_eq!(store.wait_uploaded(), Ok(()), "{case}");
        assert!(read(store, own[0].0, BLOCK) == own[0].1, "{case}");
    }

    /// Deletes the fork at `path`, as the control API does.
    fn delete(&self, path: &Path, case: &str) {
        let deleted = ExtentStore::delete(path, Some(&self.bucket));
        assert_eq!(deleted.map_err(|err| err.to_string()), Ok(()), "{case}: deleting it");
        assert!(!path.exists(), "{case}: it is there still");
    }

    /// The child's record, if it has one.
    fn record(&self, child: crate::Branch) -> Option<extent_remote::record::Record> {
        block_on(branch::record(&*self.bucket, child)).unwrap()
    }

    /// The branches live in the bucket.
    fn live(&self) -> Vec<crate::Branch> {
        block_on(branch::live(&*self.bucket)).unwrap().into_keys().collect()
    }

    /// The branches live in the bucket that no store holds, of those `named`: what a fork whose
    /// host died, or whose cleanup failed, leaves standing, which an operator finds so, and
    /// deletes, whichever placement took it last, as `fxdax-gc --branches` and `--delete-branch`
    /// do. Only the fork's child may be one, and only if the fork failed.
    fn delete_orphans(&self, named: &[crate::Branch], failed: bool, case: &str) {
        let orphans: Vec<_> =
            self.live().into_iter().filter(|live| !named.contains(live)).collect();
        if orphans.is_empty() {
            return;
        }
        assert!(failed, "{case}: a fork made left orphans: {orphans:x?}");
        assert_eq!(orphans, [self.child], "{case}: orphans");
        let deleted = block_on(branch::delete(&*self.bucket, self.child, rand::random(), u64::MAX));
        assert_eq!(deleted, Ok(()), "{case}: deleting the orphan");
    }

    /// What must hold whatever came of the fork, `forked`, `case` saying what went wrong. The
    /// collector's passes go first, then between each step, as nothing keeps them away.
    fn settle(self, forked: Result<(), crate::Error>, leftover: Leftover, case: &str) {
        let path = self.path("child");
        let mut children = Vec::new();
        // Those to check, once the parent's next writer took its branch.
        let mut unchecked: Vec<(PathBuf, &[u8])> = Vec::new();
        self.collect(case);

        // What the host left at the fork's path goes, or is the fork, as its caller was told, or
        // finds: a store of the child's, or a directory its making left unmade.
        let made = forked.is_ok() && !self.dead.load(Ordering::SeqCst);
        if path.exists() {
            if made {
                children.push(path.clone());
                unchecked.push((path.clone(), b"root"));
            } else if leftover == Leftover::Opened {
                match self.open(&path) {
                    Ok(store) => {
                        assert!(!self.taken.load(Ordering::SeqCst), "{case}: it took another's");
                        self.check_fork(&store, b"root", case);
                        children.push(path.clone());
                    }
                    Err(_) => self.delete(&path, case),
                }
            } else {
                self.delete(&path, case);
            }
        } else {
            assert!(!made, "{case}: a fork made left no store");
        }
        self.collect(case);

        // The parent's next writer takes its branch, and it holds what it held.
        let parent_path = self.path("parent");
        let parent =
            self.open(&parent_path).unwrap_or_else(|err| panic!("{case}: the parent: {err}"));
        self.gated.land_late();
        assert_eq!(parent.root(), Ok(b"root".to_vec()), "{case}");
        for (extent, data) in &self.held {
            assert!(read(&parent, *extent, data.len()) == *data, "{case}: the parent's {extent:?}");
        }
        let later = commit(&parent, 7, &[BLOCK], b"later");
        assert_eq!(parent.wait_uploaded(), Ok(()), "{case}");
        let (mut extents, root) = uploaded(&self.bucket, self.branch);
        extents.sort();
        let mut all: Vec<_> = (self.held.iter().chain(&later))
            .map(|(extent, data)| (extent.sequence, data.clone()))
            .collect();
        all.sort();
        assert!(extents == all, "{case}: the parent's branch holds other extents");
        assert_eq!(root, Some(b"later".to_vec()), "{case}");
        self.collect(case);

        // What the fork left standing in the bucket, no store holding it, an operator deletes.
        let taken = self.taken.load(Ordering::SeqCst);
        let mut named = vec![self.branch, self.other.branch()];
        named.extend(children.iter().map(|_| self.child));
        if taken {
            named.push(self.child);
        }
        self.delete_orphans(&named, !made, case);
        self.collect(case);

        // A fork to the same child, and to the same place, makes it if nothing has its ID, or fails
        // cleanly; one to a new child forks.
        let spent = self.record(self.child).is_some() || !children.is_empty();
        let again = parent.fork(&path, self.child).map_err(|err| err.errno());
        assert_eq!(again, if spent { Err(Some(Errno::EEXIST)) } else { Ok(()) }, "{case}: again");
        if !spent {
            children.push(path.clone());
            unchecked.push((path.clone(), b"later"));
        }
        let retried = crate::host_store::new_alone();
        assert_eq!(
            parent.fork(self.path("retried"), retried).map_err(|err| err.to_string()),
            Ok(()),
            "{case}: a fork to a new child"
        );
        children.push(self.path("retried"));
        unchecked.push((self.path("retried"), b"later"));
        for (child, root) in unchecked {
            let opened = self.open(&child).unwrap_or_else(|err| panic!("{case}: a fork: {err}"));
            self.check_fork(&opened, root, case);
        }

        // No branch lives that no store holds, none another's was harmed, and it goes on.
        let mut live = self.live();
        live.sort();
        let mut named = vec![self.branch, self.other.branch()];
        named.extend(children.iter().map(|child| ExtentStore::open(child).unwrap().branch()));
        if taken {
            named.push(self.child);
        }
        named.sort();
        assert_eq!(live, named, "{case}: branches no store holds, or lost");
        for (extent, data) in &self.other_held {
            assert!(read(&self.other, *extent, data.len()) == *data, "{case}: another's extent");
        }
        commit(&self.other, 8, &[BLOCK], b"other, later");
        assert_eq!(self.other.wait_uploaded(), Ok(()), "{case}: another's branch");
        if taken {
            let writer =
                extent_remote::Writer::open(self.bucket.clone(), self.child, rand::random(), TOKEN);
            assert_eq!(block_on(writer).map(drop), Ok(()), "{case}: the branch that took the ID");
        }

        // Everything but the other's deleted, nothing is left of it, in the bucket or here.
        for child in &children {
            self.delete(child, case);
        }
        drop(parent);
        self.delete(&parent_path, case);
        self.collect(case);
        let keys = block_on(extent_remote::Bucket::list(&*self.bucket, "")).unwrap();
        let kept = |branch| branch == self.other.branch() || taken && branch == self.child;
        let leaked: Vec<_> = (keys.into_iter().map(|meta| meta.key))
            .filter(|key| match layout::kind(key) {
                Kind::Segment { branch, .. }
                | Kind::Checkpoint { branch, .. }
                | Kind::Extent { branch, .. }
                | Kind::Lease { branch } => !kept(branch),
                Kind::Record { branch } => {
                    !kept(branch)
                        && !self
                            .record(branch)
                            .is_some_and(|record| record.deleted && !record.may_drop())
                }
                // A cell's object stays, as its records do (TASK-364).
                Kind::GcLease | Kind::Marks { .. } | Kind::Cell { .. } => false,
                Kind::Moves { .. } | Kind::Unknown => true,
            })
            .collect();
        assert_eq!(leaked, Vec::<String>::new(), "{case}: left in the bucket");
        // But the file buckets beside the stores their descriptions name, which outlive them, and
        // which they keep nothing in here, attached to the world's bucket instead.
        let left = std::fs::read_dir(&self.host).unwrap();
        let left = left.map(|entry| entry.unwrap().file_name().into_string().unwrap());
        let left: Vec<_> = left.filter(|name| !name.ends_with(".bucket")).collect();
        assert_eq!(left, Vec::<String>::new(), "{case}: left on the host's disk");
    }
}

/// Runs `faults` in a world of their own, checking what must hold ([`World::settle`]), and gives
/// the fork's result.
fn run(faults: &[Fault], leftover: Leftover) -> Result<(), Option<Errno>> {
    let mut world = World::new();
    let forked = world.fork(faults);
    let errno = forked.as_ref().map(drop).map_err(crate::Error::errno);
    world.settle(forked, leftover, &format!("{faults:?}, {leftover:?}"));
    errno
}

/// Runs `cases`, going on past those that fail, then fails with them all: or gives what each fork
/// gave.
fn all(cases: impl IntoIterator<Item = (Vec<Fault>, Leftover)>) -> Vec<Result<(), Option<Errno>>> {
    let (mut failed, mut forked) = (Vec::new(), Vec::new());
    for (faults, leftover) in cases {
        match std::panic::catch_unwind(|| run(&faults, leftover)) {
            Ok(result) => forked.push(result),
            Err(panic) => {
                let said = panic.downcast_ref::<String>().cloned().unwrap_or_default();
                failed.push(format!("{faults:?}, {leftover:?}: {said}"));
            }
        }
    }
    assert!(failed.is_empty(), "{} failed:\n{}", failed.len(), failed.join("\n"));
    forked
}

/// A fork with nothing going wrong forks, and what must hold holds.
#[test]
fn forks_with_nothing_going_wrong() {
    host();
    assert_eq!(run(&[], Leftover::Deleted), Ok(()));
}

/// A fork's steps come in the order [`FORK_STEPS`] says, each passed; and a failed one's cleanup's
/// in the order [`CLEANUP_STEPS`] says.
#[test]
fn forks_take_their_steps_in_order() {
    use std::sync::Mutex;
    host();
    // Each step, in the order passed, once.
    let traced = |faults: &[Fault]| {
        let steps = Arc::new(Mutex::new(Vec::new()));
        let guards: Vec<_> = (FORK_STEPS.into_iter().chain(CLEANUP_STEPS))
            .chain(["branch::record_fork", "branch::recorded_fork"])
            .filter(|step| !faults.contains(&Fault::Fails(step)))
            .map(|step| {
                let steps = steps.clone();
                FailGuard::with_callback(step, move || {
                    let mut steps = steps.lock().unwrap();
                    if !steps.contains(&step) {
                        steps.push(step);
                    }
                })
                .unwrap()
            })
            .collect();
        let mut world = World::new();
        steps.lock().unwrap().clear();
        let _ = world.fork(faults);
        drop(guards);
        Arc::try_unwrap(steps).unwrap().into_inner().unwrap()
    };
    assert_eq!(traced(&[]), FORK_STEPS);
    let cleanup = traced(&[Fault::Fails("writer::acknowledge")]);
    let cleaned: Vec<_> = cleanup.into_iter().filter(|step| !FORK_STEPS.contains(step)).collect();
    assert_eq!(cleaned, ["branch::claimed", "branch::claim_lease_ended"]);
}

/// A step of a fork that fails fails the fork, and what must hold holds.
#[test]
fn forks_failing_at_each_step() {
    host();
    let forked = all(FORK_STEPS.map(|step| (vec![Fault::Fails(step)], Leftover::Deleted)));
    let made = (FORK_STEPS.iter().zip(forked)).filter(|(_, forked)| forked.is_ok());
    let made: Vec<_> = made.map(|(step, _)| *step).collect();
    assert_eq!(made, Vec::<&str>::new(), "forks made though these steps failed");
}

/// What a host dying at a step of a fork leaves, deleted or opened, breaks nothing.
#[test]
fn hosts_dying_at_each_step_of_a_fork() {
    host();
    all(FORK_STEPS.into_iter().flat_map(|step| {
        [Leftover::Deleted, Leftover::Opened].map(|leftover| (vec![Fault::Dies(step)], leftover))
    }));
}

/// Another branch taking the child's ID, as a guest may name one that was free, whichever step of
/// the fork it comes at, keeps it, and the parent goes on.
#[test]
fn branches_taking_the_childs_id_at_each_step() {
    host();
    all(FORK_STEPS.map(|step| (vec![Fault::Taken(step)], Leftover::Deleted)));
}

/// So does it whose host dies later in the fork, whatever is done with what it left.
#[test]
fn branches_taking_the_id_of_a_fork_whose_host_dies() {
    host();
    let after_free = FORK_STEPS.iter().position(|step| *step == "remote::child_free").unwrap();
    all(FORK_STEPS[after_free + 1..].iter().flat_map(|&step| {
        [Leftover::Deleted, Leftover::Opened]
            .map(|leftover| (vec![Fault::Taken("remote::child_free"), Fault::Dies(step)], leftover))
    }));
}

/// How many writes the fork that `faults` go wrong in makes.
fn writes(faults: &[Fault]) -> usize {
    let mut world = World::new();
    world.gated.arm(usize::MAX, Mishap::Refused, Arc::new(|| {}));
    let _ = world.fork(faults);
    world.gated.writes()
}

/// A write of a fork's going wrong, its answer lost or the host dying as it goes, a dead host's
/// PUT landing late, breaks nothing.
#[test]
fn each_write_of_a_fork_going_wrong() {
    host();
    let count = writes(&[]);
    assert!(count >= 3, "a fork's segment, its child's record, its checkpoint: {count}");
    all((0..count).flat_map(|write| {
        MISHAPS.map(|mishap| (vec![Fault::Write(write, mishap)], Leftover::Deleted))
    }));
}

/// TASK-150.1: a pass of the collector, which nothing keeps away from any step of a fork, just
/// before each of the fork's writes takes nothing a branch holds, or is about to.
#[test]
fn each_write_of_a_fork_meeting_the_collector() {
    host();
    all((0..writes(&[]))
        .map(|write| (vec![Fault::Write(write, Mishap::Collected)], Leftover::Deleted)));
}

/// Where a fork may fail for what its cleanup finds: nothing of it in object storage, its segment
/// on its way, there unacknowledged, and acknowledged, its child's record given; and its child's
/// store made.
const FAILING_FOR_CLEANUPS: [&str; 5] =
    ["fork::committed", "writer::upload", "writer::uploaded", "writer::checkpoint", "fork::made"];

/// A failed fork's cleanup failing, or its host dying, at each step, or a write of it going wrong,
/// leaves nothing here, and in the bucket what stands, as an orphan, which an operator deletes.
#[test]
fn cleanups_of_failed_forks_going_wrong_at_each_step() {
    host();
    all(FAILING_FOR_CLEANUPS.into_iter().flat_map(|first| {
        CLEANUP_STEPS.into_iter().flat_map(move |step| {
            let first = Fault::Fails(first);
            [
                (vec![first, Fault::Fails(step)], Leftover::Deleted),
                (vec![first, Fault::Dies(step)], Leftover::Deleted),
            ]
        })
    }));
}

/// So does a write of the cleanup's going wrong.
#[test]
fn each_write_of_a_failed_forks_cleanup_going_wrong() {
    host();
    all(FAILING_FOR_CLEANUPS.into_iter().flat_map(|first| {
        let first = Fault::Fails(first);
        (0..writes(&[first])).flat_map(move |write| {
            MISHAPS.map(|mishap| (vec![first, Fault::Write(write, mishap)], Leftover::Deleted))
        })
    }));
}

/// TASK-79.78: a fork whose segment landed, its host lost with its disk before anything came of
/// it, is an orphan: a live branch no host holds, once the parent's next writer, elsewhere, gives
/// it its record. The collector keeps what it inherited, though its parent freed it, as it cannot
/// tell it from a branch held idle; an operator finds it among the live branches, as `fxdax-gc
/// --branches` does, and deletes it, whichever placement took it last, as `--delete-branch` does;
/// and the next pass frees what only it held.
#[test]
fn forks_orphaned_by_a_lost_host_keep_what_they_inherited_until_deleted() {
    host();
    let mut world = World::new();
    let large = world.held.last().unwrap().0;
    let _ = world.fork(&[Fault::Dies("writer::uploaded")]);
    assert!(world.dead.load(Ordering::SeqCst), "the host lives");

    let moved = world.temp.path().join("moved");
    let parent = kept_in(&moved, (1 << 30, Some(world.branch)), &world.bucket, options()).unwrap();
    assert!(world.record(world.child).is_some_and(|record| !record.deleted), "no orphan");
    let state = block_on(extent_remote::recover(&*world.bucket, world.branch, None));
    let key = state.unwrap().state.extents[&storage_device::ExtentId::from(large)].key.clone();
    assert!(matches!(layout::kind(&key), Kind::Extent { .. }), "{key}");
    for (extent, _) in &world.held {
        parent.free(*extent).unwrap();
    }
    parent.commit(None).unwrap();
    assert_eq!(parent.wait_uploaded(), Ok(()));
    drop(parent);
    let stored = || {
        let listed = block_on(extent_remote::Bucket::list(&*world.bucket, "")).unwrap();
        listed.iter().any(|meta| meta.key == key)
    };
    world.collect("orphaned");
    assert!(stored(), "what the fork inherited went while it is live");

    let held = [world.branch, world.other.branch()];
    let orphans: Vec<_> = world.live().into_iter().filter(|live| !held.contains(live)).collect();
    assert_eq!(orphans, [world.child]);
    assert_eq!(block_on(branch::delete(&*world.bucket, world.child, 9, u64::MAX)), Ok(()));
    world.collect("deleted");
    assert!(!stored(), "what the fork inherited stayed once it was deleted");
}

/// What a seed sets ([`plan`]), on top of the host dying at a step if it is odd.
fn seeded(seed: u64) -> (Vec<Fault>, Leftover) {
    let planned = plan(seed).into_iter().map(|(step, actions)| {
        let passes = actions.split('*').next().unwrap().parse().unwrap();
        Fault::FailsAfter(step, passes)
    });
    let dies = FORK_STEPS[seed as usize / 2 % FORK_STEPS.len()];
    let leftover = if seed % 4 == 1 { Leftover::Opened } else { Leftover::Deleted };
    (planned.chain((seed % 2 == 1).then_some(Fault::Dies(dies))).collect(), leftover)
}

/// Faults a seed picks, several to a fork, its cleanup and the daemon's steps among them: a failure
/// says the seed; FXDAX_FAULT_SEED says which to run alone.
#[test]
fn faults_a_seed_picks() {
    host();
    let seeds = match std::env::var("FXDAX_FAULT_SEED") {
        Ok(seed) => {
            let seed = seed.parse().unwrap();
            seed..seed + 1
        }
        Err(_) => 0..64,
    };
    all(seeds.map(seeded));
}
