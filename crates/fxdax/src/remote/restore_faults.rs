// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Restores of one snapshot on several hosts with something going wrong at each of their steps
//! (TASK-158), as [`super::fork_faults`] has it for a fork: a step of a fork of a point failing, on
//! each host; its host dying there; or, there, other hosts restoring the point and a pass of the
//! collector running, or the point being deleted; or a write of it going wrong. Then, whatever came
//! of it, what [`Hosts::settle`] checks: no fork told it was made lost anything, though only the
//! point keeps what it holds, the branch it was taken of having freed all of it; nothing wrote the
//! point; every other host restores it still; and nothing is left once everything is deleted.

use super::fork_faults::{self, Leftover, MISHAPS, commit, host, options};
use super::tests::{Gated, Mishap, kept_in, naming, reopened, restart};
use crate::faults::RESTORE_STEPS;
use crate::{Errno, ExtentId, ExtentStore, StoreRole};
use extent_remote::layout::{self, Kind};
use extent_remote::store::Store as Bucket;
use extent_remote::{branch, recover};
use fail::FailGuard;
use futures::executor::block_on;
use object_store::memory::InMemory;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// How many hosts hold the point: the snapshot's own, whose copy the point's fork made, then
/// others.
const HOSTS: usize = 3;

/// What goes wrong in a restore on one of the hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// The step of this name fails.
    Fails(&'static str),
    /// The host dies at the step of this name.
    Dies(&'static str),
    /// At the step of this name, every other host restores the point, and a pass of the collector
    /// runs, deleting what is free at once.
    Raced(&'static str),
    /// At the step of this name, the point is deleted, and a pass of the collector runs.
    Deleted(&'static str),
    /// So, and then the host dies: what it leaves, in the middle of its fork, nothing keeps.
    DeletedAndDies(&'static str),
    /// This goes wrong with the write numbered so, counting from the restore's first.
    Write(usize, Mishap),
}

/// A bucket, and the hosts of one point in it: the branch it was taken of has freed what it held.
struct Hosts {
    temp: tempfile::TempDir,
    bucket: Arc<Bucket>,
    gated: Vec<Arc<Gated>>,
    /// Each host's disk: a dead host's, its copy, restarted.
    disks: Vec<PathBuf>,
    /// Each host's copy of the point, open, but for one that died.
    copies: Vec<Option<Arc<ExtentStore>>>,
    /// The store the point was taken of, and its branch.
    source: Option<ExtentStore>,
    point: crate::Branch,
    /// What the point holds, and its root.
    held: Vec<(ExtentId, Vec<u8>)>,
    /// The forks made, by their branches, with their stores' paths.
    children: Arc<Mutex<Vec<(crate::Branch, PathBuf)>>>,
    /// Whether a host died.
    dead: Arc<AtomicBool>,
    /// Whether the point was deleted.
    deleted: Arc<AtomicBool>,
}

/// Where a host's copy of the point is on its disk.
fn copy_at(host: usize, disk: &Path) -> PathBuf {
    disk.join(if host == 0 { "point" } else { "parent" })
}

/// The bytes the point's extents hold, as `bucket` has them for the fork `child`.
fn in_bucket(
    bucket: &Arc<Bucket>,
    child: crate::Branch,
) -> Result<Vec<(ExtentId, Vec<u8>)>, String> {
    block_on(async {
        let record = branch::record(&**bucket, child).await.map_err(|err| err.to_string())?;
        let origin = record.ok_or("no record")?.origin();
        let state = recover(&**bucket, child, origin).await.map_err(|err| err.to_string())?.state;
        let mut read = Vec::new();
        for (extent, location) in &state.extents {
            let bytes = extent_remote::read(bucket, *extent, location).await;
            let extent = ExtentId { branch: extent.branch, sequence: extent.sequence };
            read.push((extent, bytes.map_err(|err| err.to_string())?));
        }
        Ok(read)
    })
}

impl Hosts {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let gated: Vec<_> = (0..HOSTS).map(|_| Arc::new(Gated::over(memory.clone()))).collect();
        let reached = |host: usize| Arc::new(Bucket::new(gated[host].clone(), ""));
        let disks: Vec<_> =
            (0..HOSTS).map(|host| temp.path().join(format!("host{host}"))).collect();
        for disk in &disks {
            std::fs::create_dir(disk).unwrap();
        }
        let source = kept_in(disks[0].join("parent"), (1 << 30, None), &reached(0), options());
        let source = source.unwrap();
        let lens = [fork_faults::BLOCK, 4 << 20];
        let held = commit(&source, 1, &lens, b"root");
        let point = crate::host_store::new_alone();
        source.point_to(disks[0].join("point"), point).unwrap();
        for (extent, _) in &held {
            source.free(*extent).unwrap();
        }
        source.commit(Some(b"later")).unwrap();
        // Another fork, gone: a checkpoint past the point's, so that only the point keeps that, and
        // what the point names is all gone once nothing keeps it: its state too, not just its data.
        let later = disks[0].join("later");
        source.fork_to(&later, crate::host_store::new_alone()).unwrap();
        ExtentStore::delete(&later, Some(&bucket)).unwrap();

        let mut copies = Vec::new();
        for (host, disk) in disks.iter().enumerate() {
            let path = copy_at(host, disk);
            let copy = match host {
                0 => reopened(&path, options()).unwrap(),
                _ => kept_in(&path, (1 << 30, Some(point)), &reached(host), options()).unwrap(),
            };
            assert_eq!(copy.access().role, StoreRole::Point, "host {host} holds no point");
            copies.push(Some(Arc::new(copy)));
        }
        Self {
            temp,
            bucket,
            gated,
            disks,
            copies,
            source: Some(source),
            point,
            held,
            children: Default::default(),
            dead: Default::default(),
            deleted: Default::default(),
        }
    }

    /// Sweeps of the collector over every cell, deleting what is free at once, until two in a row
    /// delete nothing: a dead cell's pass retires the references the point's origin's cell keeps
    /// the fork point for, which that cell's next pass reclaims (TASK-395.4).
    fn collect(bucket: &Arc<Bucket>, case: &str) {
        let options = extent_remote::collector::Options { compact_below: 0.0, grace: 0 };
        let collected = extent_remote::testing::collect_until_quiet(&**bucket, &options);
        block_on(collected).unwrap_or_else(|err| panic!("{case}: the collector: {err}"));
    }

    /// Asserts nothing wrote the point: no lease, but its deletion's, no segment, no checkpoint.
    fn check_unwritten(&self, case: &str) {
        let keys = block_on(extent_remote::Bucket::list(&*self.bucket, "")).unwrap();
        let written = keys.iter().filter(|meta| match layout::kind(&meta.key) {
            Kind::Segment { branch, .. } | Kind::Checkpoint { branch, .. } => branch == self.point,
            Kind::Lease { branch } => branch == self.point && !self.deleted.load(Ordering::SeqCst),
            _ => false,
        });
        let written: Vec<_> = written.map(|meta| &meta.key).collect();
        assert_eq!(written, Vec::<&String>::new(), "{case}: the point was written");
    }

    /// Asserts the fork `child` holds what the point held, in the bucket, and, its store at
    /// `path`, opened, reads it and goes on.
    fn check_fork(&self, child: crate::Branch, path: &Path, case: &str) {
        let mut read = in_bucket(&self.bucket, child).unwrap_or_else(|err| panic!("{case}: {err}"));
        read.sort();
        let mut held = self.held.clone();
        held.sort();
        assert!(read == held, "{case}: fork {child} holds other extents");
        naming(path, &self.bucket);
        let store = reopened(path, options()).unwrap();
        assert_eq!(store.root(), Ok(b"root".to_vec()), "{case}");
        let own = commit(&store, 5, &[fork_faults::BLOCK], b"child");
        assert_eq!(store.wait_uploaded(), Ok(()), "{case}");
        assert!(fork_faults::read(&store, own[0].0, fork_faults::BLOCK) == own[0].1, "{case}");
    }

    /// Restores the point on `host`, to a new child at `name` on its disk, and gives what that
    /// gave: the child noted, if made.
    fn restore(&mut self, host: usize, name: &str) -> Result<(), Option<Errno>> {
        let path = self.disks[host].join(name);
        let child = crate::host_store::new_alone();
        let copy = self.copies[host].as_ref().expect("a host's copy");
        let forked = copy.fork_to(&path, child).map_err(|err| err.errno());
        if forked.is_ok() {
            self.children.lock().unwrap().push((child, path));
        }
        forked
    }

    /// Arms `fault`, for a restore on `host` of the child whose store will be at `name`.
    fn arm(&self, host: usize, fault: Fault) -> Option<FailGuard> {
        let once = Arc::new(AtomicBool::new(false));
        let guard = match fault {
            Fault::Fails(step) => FailGuard::new(step, "1*return"),
            Fault::Dies(step) => {
                let die = self.death(host);
                FailGuard::with_callback(step, move || die())
            }
            Fault::Raced(step) => {
                let others: Vec<_> = (0..HOSTS).filter(|&other| other != host).collect();
                let copies: Vec<_> = (others.iter())
                    .map(|&other| (self.copies[other].clone().unwrap(), self.disks[other].clone()))
                    .collect();
                let (bucket, children) = (self.bucket.clone(), self.children.clone());
                FailGuard::with_callback(step, move || {
                    if once.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    // Off this host's thread, which may be on a runtime of its own.
                    let (copies, bucket, children) =
                        (copies.clone(), bucket.clone(), children.clone());
                    let raced = std::thread::spawn(move || {
                        for (copy, disk) in copies {
                            let child = crate::host_store::new_alone();
                            let path = disk.join("raced");
                            copy.fork_to(&path, child).unwrap();
                            children.lock().unwrap().push((child, path));
                        }
                        Self::collect(&bucket, "raced");
                    });
                    raced.join().unwrap();
                })
            }
            Fault::Deleted(step) | Fault::DeletedAndDies(step) => {
                let (bucket, point) = (self.bucket.clone(), self.point);
                let deleted = self.deleted.clone();
                let die = matches!(fault, Fault::DeletedAndDies(_)).then(|| self.death(host));
                FailGuard::with_callback(step, move || {
                    if once.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let bucket = bucket.clone();
                    deleted.store(true, Ordering::SeqCst);
                    let deleting = std::thread::spawn(move || {
                        block_on(branch::delete(&*bucket, point, rand::random(), u64::MAX))
                            .unwrap();
                        Self::collect(&bucket, "the point deleted");
                    });
                    deleting.join().unwrap();
                    if let Some(die) = &die {
                        die();
                    }
                })
            }
            Fault::Write(write, mishap) => {
                let bucket = self.bucket.clone();
                let act: Arc<dyn Fn() + Send + Sync> = match mishap {
                    Mishap::Collected => Arc::new(move || Self::collect(&bucket, "a write")),
                    _ => self.death(host),
                };
                self.gated[host].arm(write, mishap, act);
                return None;
            }
        };
        Some(guard.unwrap())
    }

    /// What `host`'s dying does: its disk, as it is, copied and restarted, then it cut off.
    fn death(&self, host: usize) -> Arc<dyn Fn() + Send + Sync> {
        let (gated, dead) = (self.gated[host].clone(), self.dead.clone());
        let (from, to) = (self.disks[host].clone(), self.restarted());
        Arc::new(move || {
            if !dead.swap(true, Ordering::SeqCst) {
                restart(&from, &to);
                gated.sever();
            }
        })
    }

    /// Where a dead host restarts.
    fn restarted(&self) -> PathBuf {
        self.temp.path().join("restarted")
    }

    /// Runs a restore on `host`, with `fault` going wrong: and what must hold, `leftover` being
    /// what is done with what a dead host left.
    fn run(
        mut self,
        host: usize,
        fault: Option<Fault>,
        leftover: Leftover,
    ) -> Result<(), Option<Errno>> {
        let case = format!("host {host}, {fault:?}, {leftover:?}");
        let guard = fault.and_then(|fault| self.arm(host, fault));
        let handle = self.disks[host].join("child");
        let child = crate::host_store::new_alone();
        let forked = self.copies[host].as_ref().unwrap().fork_to(&handle, child);
        let forked = forked.map_err(|err| err.errno());
        drop(guard);
        let dead = self.dead.load(Ordering::SeqCst);
        if dead {
            self.copies[host] = None;
            self.disks[host] = self.restarted();
            let path = copy_at(host, &self.disks[host]);
            naming(&path, &self.bucket);
            match reopened(&path, options()) {
                Ok(copy) => self.copies[host] = Some(Arc::new(copy)),
                Err(err) => {
                    assert!(self.deleted.load(Ordering::SeqCst), "{case}: the copy: {err}");
                }
            }
        }
        self.settle(host, child, forked, leftover, &case);
        forked
    }

    /// What must hold, whatever the restore of `child` on `host` gave.
    fn settle(
        mut self,
        host: usize,
        child: crate::Branch,
        forked: Result<(), Option<Errno>>,
        leftover: Leftover,
        case: &str,
    ) {
        Self::collect(&self.bucket, case);
        let handle = self.disks[host].join("child");
        let told = forked.is_ok() && !self.dead.load(Ordering::SeqCst);
        let opened = || {
            naming(&handle, &self.bucket);
            reopened(&handle, options()).map(drop).map_err(|err| err.to_string())
        };
        if told || handle.exists() && leftover == Leftover::Opened && opened().is_ok() {
            self.children.lock().unwrap().push((child, handle.clone()));
        } else if handle.exists() {
            let deleted = ExtentStore::delete(&handle, Some(&self.bucket));
            assert_eq!(deleted.map_err(|err| err.to_string()), Ok(()), "{case}: deleting it");
        }
        // What the dead host had going, after what was done with what it left.
        self.gated[host].land_late();
        Self::collect(&self.bucket, case);
        self.check_unwritten(case);
        // What a restore whose host died left standing in the bucket, no store holding it, an
        // operator finds among the live branches, and deletes (TASK-165.7).
        let mut named = vec![self.point, self.source.as_ref().unwrap().branch()];
        named.extend(self.children.lock().unwrap().iter().map(|(child, _)| *child));
        let live = block_on(extent_remote::testing::live(&*self.bucket)).unwrap();
        let orphans: Vec<_> = live.into_keys().filter(|live| !named.contains(live)).collect();
        if !orphans.is_empty() {
            assert!(!told, "{case}: a restore made left orphans: {orphans:x?}");
            assert_eq!(orphans, [child], "{case}: orphans");
            let deleted = block_on(branch::delete(&*self.bucket, child, rand::random(), u64::MAX));
            assert_eq!(deleted, Ok(()), "{case}: deleting the orphan");
            Self::collect(&self.bucket, case);
        }

        // Every host restores it still: none's copy went stale. None, once it is deleted, of
        // which what the fork leaves goes with it.
        let deleted = self.deleted.load(Ordering::SeqCst);
        for other in 0..HOSTS {
            if self.copies[other].is_some() {
                let restored = self.restore(other, "again");
                let expected = if deleted { Err(Some(Errno::ENOENT)) } else { Ok(()) };
                assert_eq!(restored, expected, "{case}: host {other} restores it");
                assert_eq!(self.disks[other].join("again").exists(), !deleted, "{case}");
            }
        }
        // Whatever passes came, each holds what it was told.
        Self::collect(&self.bucket, case);
        let children = self.children.lock().unwrap().clone();
        for (child, path) in &children {
            self.check_fork(*child, path, case);
        }

        // The point goes: what its forks hold stays, for as long as they live.
        self.copies.clear();
        if !deleted {
            let holder = rand::random();
            let gone = block_on(branch::delete(&*self.bucket, self.point, holder, u64::MAX));
            assert_eq!(gone, Ok(()), "{case}");
        }
        Self::collect(&self.bucket, case);
        for (child, _) in &children {
            let read = in_bucket(&self.bucket, *child).map(|read| read.len());
            assert_eq!(read, Ok(self.held.len() + 1), "{case}: {child}");
        }

        // Everything deleted, nothing is left of it.
        for (_, path) in &children {
            let deleted = ExtentStore::delete(path, Some(&self.bucket));
            assert_eq!(deleted.map_err(|err| err.to_string()), Ok(()), "{case}: deleting a fork");
        }
        let source = self.source.take().unwrap();
        let branch = source.branch();
        drop(source);
        let gone = block_on(branch::delete(&*self.bucket, branch, rand::random(), u64::MAX));
        assert_eq!(gone, Ok(()), "{case}");
        Self::collect(&self.bucket, case);
        let keys = block_on(extent_remote::Bucket::list(&*self.bucket, "")).unwrap();
        let leaked: Vec<_> = (keys.into_iter().map(|meta| meta.key))
            .filter(|key| match layout::kind(key) {
                // TASK-395.10: every reference retired, whether its child got a record or not.
                Kind::Reference { .. }
                | Kind::Segment { .. }
                | Kind::Checkpoint { .. }
                | Kind::Extent { .. }
                | Kind::Moves { .. } => true,
                Kind::Record { branch } => !block_on(branch::record(&*self.bucket, branch))
                    .unwrap()
                    .is_some_and(|record| record.deleted),
                // fxdax: a cell's object stays, as its branches' records do, its epoch fencing
                // its volumes' leases (TASK-364): left over only if it names a live branch.
                Kind::Cell { cell } => {
                    let object = block_on(extent_remote::cell::read(&*self.bucket, cell));
                    let object = object.unwrap().expect("a cell listed");
                    object.volumes.values().any(|volume| {
                        let branch = extent_remote::layout::Branch { cell, id: volume.branch };
                        let record = block_on(branch::record(&*self.bucket, branch)).unwrap();
                        record.is_some_and(|record| !record.deleted)
                    })
                }
                // fxdax: a block of object IDs taken stays for good (TASK-364).
                Kind::GcLease { .. } | Kind::Marks { .. } | Kind::ObjectIds { .. } => false,
                _ => true,
            })
            .collect();
        assert_eq!(leaked, Vec::<String>::new(), "{case}: left in the bucket");
    }
}

/// Runs `cases`, each in hosts of its own, going on past those that fail, then fails with them all:
/// or gives what each restore gave.
fn all(
    cases: impl IntoIterator<Item = (usize, Option<Fault>, Leftover)>,
) -> Vec<Result<(), Option<Errno>>> {
    let (mut failed, mut restored) = (Vec::new(), Vec::new());
    for (host, fault, leftover) in cases {
        match std::panic::catch_unwind(|| Hosts::new().run(host, fault, leftover)) {
            Ok(result) => restored.push(result),
            Err(panic) => {
                let said = panic.downcast_ref::<String>().cloned().unwrap_or_default();
                failed.push(format!("host {host}, {fault:?}, {leftover:?}: {said}"));
            }
        }
    }
    assert!(failed.is_empty(), "{} failed:\n{}", failed.len(), failed.join("\n"));
    restored
}

/// Each host in turn, with `fault`, whatever is done with what it leaves.
fn on_each_host(fault: Fault) -> impl Iterator<Item = (usize, Option<Fault>, Leftover)> {
    (0..HOSTS).flat_map(move |host| {
        [Leftover::Deleted, Leftover::Opened].map(|leftover| (host, Some(fault), leftover))
    })
}

/// A restore on each host with nothing going wrong restores, and what must hold holds.
#[test]
fn restores_with_nothing_going_wrong() {
    host();
    let restored = all((0..HOSTS).map(|host| (host, None, Leftover::Deleted)));
    assert_eq!(restored, vec![Ok(()); HOSTS]);
}

/// A restore's steps come in the order [`RESTORE_STEPS`] says, each passed, and none of a fork's
/// on a branch with a writer: no upload, no segment, no acknowledgement.
#[test]
fn restores_take_their_steps_in_order() {
    use crate::faults::FORK_STEPS;
    use std::sync::Mutex;
    host();
    let hosts = Hosts::new();
    let steps = Arc::new(Mutex::new(Vec::new()));
    let guards: Vec<_> = (FORK_STEPS.into_iter().chain(RESTORE_STEPS))
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
    let copy = hosts.copies[1].as_ref().unwrap();
    copy.fork_to(hosts.disks[1].join("child"), crate::host_store::new_alone()).unwrap();
    drop(guards);
    assert_eq!(*steps.lock().unwrap(), RESTORE_STEPS);
}

/// A step of a restore failing on any host fails it, and what must hold holds.
#[test]
fn restores_failing_at_each_step_on_each_host() {
    host();
    let cases = RESTORE_STEPS.into_iter().flat_map(|step| on_each_host(Fault::Fails(step)));
    let cases: Vec<_> = cases.collect();
    let restored = all(cases.clone());
    let made = cases.iter().zip(&restored).filter(|(_, result)| result.is_ok());
    assert_eq!(made.count(), 0, "restores made though a step of them failed");
}

/// What a host dying at a step of a restore leaves, deleted or opened, breaks nothing, on any host.
#[test]
fn hosts_dying_at_each_step_of_a_restore() {
    host();
    all(RESTORE_STEPS.into_iter().flat_map(|step| on_each_host(Fault::Dies(step))));
}

/// At each step of a restore, every other host restoring the point and a pass of the collector
/// take nothing from it, and it restores.
#[test]
fn restores_racing_others_and_the_collector_at_each_step() {
    host();
    let restored = all(RESTORE_STEPS.into_iter().flat_map(|step| on_each_host(Fault::Raced(step))));
    assert!(restored.iter().all(Result::is_ok), "{restored:?}");
}

/// The point deleted, and a pass of the collector, at each step of a restore: it fails with
/// `ENOENT`, leaving nothing, if it hadn't been made; and if it had, it holds all it held.
#[test]
fn points_deleted_at_each_step_of_a_restore() {
    host();
    let restored =
        all(RESTORE_STEPS.into_iter().flat_map(|step| on_each_host(Fault::Deleted(step))));
    let unexpected =
        restored.iter().filter(|result| !matches!(result, Ok(()) | Err(Some(Errno::ENOENT))));
    assert_eq!(unexpected.collect::<Vec<_>>(), Vec::<&Result<(), Option<Errno>>>::new());
    assert!(restored.iter().any(Result::is_ok), "no restore outlived the point's deletion");
    assert!(restored.iter().any(Result::is_err), "no restore failed for it");
}

/// A point deleted, and what it named collected, with a fork of it being made whose host dies
/// there, on any host: its record, being made, names what is gone, which must stop no pass of the
/// collector, and what it left goes, or fails to open, whatever is done with it.
#[test]
fn points_deleted_as_hosts_die_in_the_middle_of_a_restore() {
    host();
    let steps = ["point::record", "point::recorded", "point::referred", "point::checked"];
    all(steps.into_iter().flat_map(|step| on_each_host(Fault::DeletedAndDies(step))));
}

/// A restore's writes: its child's record, and the record saying it is made.
const WRITES: usize = 2;

/// A write of a restore going wrong, its answer lost or the host dying as it goes, a dead host's
/// PUT landing late, or a pass of the collector just before it, breaks nothing, on any host.
#[test]
fn each_write_of_a_restore_going_wrong() {
    host();
    let mishaps: Vec<_> = MISHAPS.into_iter().chain([Mishap::Collected]).collect();
    all((0..WRITES).flat_map(|write| {
        let mishaps = mishaps.clone();
        mishaps.into_iter().flat_map(move |mishap| on_each_host(Fault::Write(write, mishap)))
    }));
}
