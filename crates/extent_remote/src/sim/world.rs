// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The simulated world: hosts running branches' writers for their guests, forks, takeovers,
//! deletions and collectors, driven by a seeded scheduler that also decides when, in what order and
//! how each request to the bucket completes. After every step it checks every live branch against
//! what its guests did and were told.

use super::bucket::{Direct, Fault, SimBucket};
use crate::batch::{Batch, Commit};
use crate::collector::{self, Options};
use crate::layout::{Child, Origin};
use crate::record::Kind;
use crate::writer::Writer;
use crate::{Error, branch, recovery, rules};
use extent_frame::spent::Spent;
use futures::executor::{LocalPool, LocalSpawner, block_on};
use futures::future::{AbortHandle, abortable};
use futures::task::LocalSpawnExt as _;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use storage_device::ExtentId;

/// How likely each action is, per step.
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    /// A request to the bucket completes.
    pub complete: u32,
    pub commit: u32,
    pub close: u32,
    pub checkpoint: u32,
    pub fork: u32,
    /// A writer takes a branch that has none.
    pub open: u32,
    /// A writer takes a branch from the one it has.
    pub takeover: u32,
    /// A host crashes, sometimes losing its disk.
    pub crash: u32,
    pub delete: u32,
    /// A branch a fork of which lives is deleted: a base whose forks live on (TASK-39.4).
    pub delete_base: u32,
    pub collect: u32,
    /// A collector stops midway.
    pub stop_collector: u32,
    /// A placement cut off takes a branch back, with the fencing token it had (TASK-39.7).
    pub reclaim: u32,
    /// A placement cut off deletes a branch, with the fencing token it had (TASK-45.1).
    pub stale_delete: u32,
    /// A fork that failed is deleted, as its host's cleanup deletes it, or a crash's leftovers are
    /// (TASK-79.82).
    pub unfork: u32,
    /// A fork whose segment never went is deleted as another branch takes its child's ID.
    pub contest: u32,
    /// Two forks, of other parents or on other hosts, name one child within a round trip, neither's
    /// segment landing: each takes a record of its own for it, and the one that lost deletes its
    /// fork (TASK-79.82).
    pub twins: u32,
    /// A fork to a point (doc-7, Points).
    pub point: u32,
    /// A host restores a point it took a copy of, which may be deleted by now: a fork of it.
    pub restore: u32,
    /// A host's stale copy of a point tries to take it as a writer, which must fail.
    pub write_point: u32,
    /// The store's clock advances.
    pub tick: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub steps: usize,
    pub hosts: usize,
    /// The chances that a request fails before, or after, taking effect.
    pub fail_before: f64,
    pub fail_after: f64,
    /// The chance that a takeover holds the creates of segments of the writer it takes the branch
    /// from, each until an object at its key is deleted ([`SimBucket::hold`], TASK-79.18).
    pub hold: f64,
    pub weights: Weights,
    pub options: Options,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            steps: 400,
            hosts: 3,
            fail_before: 0.05,
            fail_after: 0.05,
            hold: 0.0,
            weights: Weights {
                complete: 60,
                commit: 12,
                close: 6,
                checkpoint: 2,
                fork: 1,
                open: 3,
                takeover: 1,
                crash: 1,
                delete: 1,
                delete_base: 0,
                collect: 2,
                stop_collector: 1,
                reclaim: 0,
                stale_delete: 0,
                unfork: 0,
                contest: 0,
                twins: 0,
                point: 0,
                restore: 0,
                write_point: 0,
                tick: 3,
            },
            options: Options { compact_below: 0.5, grace: 4 },
        }
    }
}

type SimWriter = Writer<SimBucket>;

struct Branch {
    origin: Option<Origin>,
    /// The parent's last commit at the fork, which the child's first follows.
    base: Option<u64>,
    /// The commits its guests were told are durable.
    acked: BTreeSet<u64>,
    deleting: bool,
    /// The highest fencing token a writer took it with: its latest placement's.
    taken: u64,
    /// Whether it is a point, which nothing writes.
    point: bool,
    /// Its owner, which its maker chose: its fork's, if it is a fork's child.
    owner: u64,
}

impl Branch {
    /// A branch forked from `origin`, its parent's last commit there `base`, a point if `point`,
    /// of `owner`.
    fn forked(origin: Option<Origin>, base: Option<u64>, point: bool, owner: u64) -> Self {
        Self { origin, base, acked: BTreeSet::new(), deleting: false, taken: 0, point, owner }
    }
}

/// Seals `extent`, of `bytes`, in `commit`: on its own if `own`.
fn seal(commit: &mut Commit, extent: ExtentId, bytes: Vec<u8>, own: bool) {
    if own {
        commit.own.push((extent.sequence, bytes));
    } else {
        commit.extents.push((extent, bytes));
    }
}

/// A commit, as its guest made it.
struct Truth {
    branch: u64,
    parent: Option<u64>,
    /// Its extents: their bytes, and whether stored on their own.
    extents: Vec<(ExtentId, Vec<u8>, bool)>,
    frees: Vec<ExtentId>,
    root: Option<Vec<u8>>,
    /// The branch's capacity, if it sets it (TASK-165.3).
    capacity: Option<u64>,
}

#[derive(Default)]
struct Host {
    /// Each branch's own commits made here, durable on the host's disk, in order.
    disk: HashMap<u64, Vec<u64>>,
    sessions: BTreeMap<u64, Session>,
}

/// A branch's writer on a host, and its guest.
struct Session {
    id: u64,
    writer: Option<Arc<SimWriter>>,
    tasks: Vec<AbortHandle>,
    guest: Guest,
    /// The guest's commits since its writer last took a segment of them.
    batch: Batch,
}

#[derive(Default)]
struct Guest {
    head: Option<u64>,
    held: BTreeMap<ExtentId, Vec<u8>>,
    next_sequence: u64,
}

enum Event {
    Opened {
        host: usize,
        branch: u64,
        session: u64,
        /// The fencing token of the placement it is.
        token: u64,
        result: Result<Arc<SimWriter>, Error>,
    },
    Uploaded {
        host: usize,
        branch: u64,
        session: u64,
        result: Result<(), Error>,
    },
    Acked {
        host: usize,
        branch: u64,
        session: u64,
        result: Result<Option<u64>, Error>,
        head: Option<u64>,
    },
    Failed {
        host: usize,
        branch: u64,
        session: u64,
        error: Error,
    },
    Deleted {
        branch: u64,
        /// Whether a placement cut off deleted it, which must fail.
        stale: bool,
        result: Result<(), Error>,
    },
    /// What deleting a failed fork's child gave ([`branch::delete_fork`]).
    Unforked {
        child: u64,
        result: Result<bool, Error>,
    },
    /// What deleting a fork's child gave, as another branch, of `owner`, took its ID, and what
    /// taking it gave.
    Contested {
        child: u64,
        owner: u64,
        deleted: Result<bool, Error>,
        taken: Result<u64, Error>,
    },
    Collected {
        id: u64,
        result: Result<collector::Report, Error>,
    },
    /// What two forks naming one child, neither's segment landing, gave as each took a record of
    /// its own for it ([`branch::create_forked`]), by their owners.
    Twinned {
        child: u64,
        taken: [(u64, Result<(), Error>); 2],
    },
    /// What deleting the fork whose child the other of two forks took gave.
    Untwinned {
        child: u64,
        result: Result<bool, Error>,
    },
    /// What a host's fork of a point gave.
    Restored {
        child: u64,
        result: Result<(), Error>,
    },
    /// A writer took a point, which must not happen.
    PointTaken {
        point: u64,
        result: Result<(), Error>,
    },
}

/// A failed fork's deletion, until it is done: the record its fork gives its child, as its host
/// knew, and the task, unless a crash stopped it, for its host's next start to go on with.
struct Unfork {
    forked: Kind,
    /// Its child's owner.
    owner: u64,
    running: Option<AbortHandle>,
}

/// A fork of a point, until it is done: of which, naming what, on which host, and the task, unless
/// a crash stopped it, for its host's next start to finish or delete.
struct Restore {
    point: u64,
    origin: Origin,
    host: usize,
    /// Its child's owner.
    owner: u64,
    running: Option<AbortHandle>,
}

pub struct World {
    rng: StdRng,
    profile: Profile,
    bucket: Arc<SimBucket>,
    pool: LocalPool,
    spawner: LocalSpawner,
    events: Rc<RefCell<Vec<Event>>>,
    hosts: Vec<Host>,
    branches: BTreeMap<u64, Branch>,
    commits: HashMap<u64, Truth>,
    /// The commit that minted each extent.
    minted: HashMap<ExtentId, u64>,
    collectors: BTreeMap<u64, AbortHandle>,
    unforks: BTreeMap<u64, Unfork>,
    restores: BTreeMap<u64, Restore>,
    next_id: u64,
    /// Whether requests fail.
    faults: bool,
    /// Whether everything is finishing: then every live branch must open.
    quiescing: bool,
    pub violations: Vec<String>,
}

impl World {
    pub fn new(seed: u64, profile: Profile) -> Self {
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        let mut world = Self {
            rng: StdRng::seed_from_u64(seed),
            profile,
            bucket: SimBucket::new(),
            pool,
            spawner,
            events: Rc::default(),
            hosts: (0..profile.hosts).map(|_| Host::default()).collect(),
            branches: BTreeMap::new(),
            commits: HashMap::new(),
            minted: HashMap::new(),
            collectors: BTreeMap::new(),
            unforks: BTreeMap::new(),
            restores: BTreeMap::new(),
            next_id: 1,
            faults: true,
            quiescing: false,
            violations: Vec::new(),
        };
        let (root, owner) = (world.id(), world.id());
        block_on(branch::create(&Direct(world.bucket.clone()), root, owner)).unwrap();
        world.branches.insert(root, Branch::forked(None, None, false, owner));
        world
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn violation(&mut self, violation: String) {
        self.violations.push(violation);
    }

    /// Runs the profile's steps, then lets everything finish and checks that no garbage is left.
    pub fn run(&mut self) {
        for _ in 0..self.profile.steps {
            self.step();
            self.check();
            if !self.violations.is_empty() {
                return;
            }
        }
        self.quiesce();
    }

    fn step(&mut self) {
        self.settle_tasks();
        let w = self.profile.weights;
        let choices = [
            w.complete,
            w.commit,
            w.close,
            w.checkpoint,
            w.fork,
            w.open,
            w.takeover,
            w.crash,
            w.delete,
            w.delete_base,
            w.collect,
            w.stop_collector,
            w.reclaim,
            w.stale_delete,
            w.unfork,
            w.contest,
            w.twins,
            w.point,
            w.restore,
            w.write_point,
            w.tick,
        ];
        let mut roll = self.rng.random_range(0..choices.iter().sum::<u32>());
        let choice = choices.iter().position(|&weight| {
            let hit = roll < weight;
            roll = roll.saturating_sub(weight);
            hit
        });
        match choice.unwrap() {
            0 => self.complete(),
            1 => self.commit(),
            2 => self.close(),
            3 => self.checkpoint(),
            4 => self.fork(false),
            5 => self.open(false),
            6 => self.open(true),
            7 => self.crash(),
            8 => self.delete(false),
            9 => self.delete(true),
            10 => self.collect(),
            11 => self.stop_collector(),
            12 => self.reclaim(),
            13 => self.stale_delete(),
            14 => self.unfork(),
            15 => self.contest(),
            16 => self.twins(),
            17 => self.fork(true),
            18 => self.restore(),
            19 => self.write_point(),
            _ => {
                let ticks = self.rng.random_range(1..=self.profile.options.grace);
                self.bucket.advance(ticks);
            }
        }
        self.settle_tasks();
    }

    /// Runs tasks until they wait for requests, handling what they report.
    fn settle_tasks(&mut self) {
        loop {
            self.pool.run_until_stalled();
            let events = std::mem::take(&mut *self.events.borrow_mut());
            if events.is_empty() {
                return;
            }
            for event in events {
                self.handle(event);
            }
        }
    }

    fn complete(&mut self) {
        let waiting = self.bucket.waiting();
        if waiting == 0 {
            return;
        }
        let index = self.rng.random_range(0..waiting);
        let roll: f64 = self.rng.random();
        let fault = if !self.faults {
            Fault::None
        } else if roll < self.profile.fail_before {
            Fault::Before
        } else if roll < self.profile.fail_before + self.profile.fail_after {
            Fault::After
        } else {
            Fault::None
        };
        self.bucket.complete(index, fault);
    }

    // Tasks.

    fn spawn(&mut self, task: impl Future<Output = ()> + 'static) -> AbortHandle {
        let (task, handle) = abortable(task);
        self.spawner
            .spawn_local(async move {
                let _ = task.await;
            })
            .unwrap();
        handle
    }

    fn spawn_session(
        &mut self,
        host: usize,
        branch: u64,
        task: impl Future<Output = ()> + 'static,
    ) {
        let handle = self.spawn(task);
        match self.hosts[host].sessions.get_mut(&branch) {
            Some(session) => session.tasks.push(handle),
            None => handle.abort(),
        }
    }

    fn upload(&mut self, host: usize, branch: u64, session: u64, writer: Arc<SimWriter>, seq: u64) {
        let events = self.events.clone();
        self.spawn_session(host, branch, async move {
            let result = writer.upload(seq).await;
            events.borrow_mut().push(Event::Uploaded { host, branch, session, result });
        });
    }

    fn acknowledge(&mut self, host: usize, branch: u64, session: u64, writer: Arc<SimWriter>) {
        let events = self.events.clone();
        self.spawn_session(host, branch, async move {
            let result = writer.acknowledge().await;
            let head = writer.state().last_commit;
            events.borrow_mut().push(Event::Acked { host, branch, session, result, head });
        });
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Opened { host, branch, session, token, result } => match result {
                Ok(writer) => {
                    let found = self.branches.get_mut(&branch).unwrap();
                    found.taken = found.taken.max(token);
                    self.opened(host, branch, session, writer)
                }
                Err(error) => {
                    if self.quiescing {
                        self.violation(format!("branch {branch:x} does not open: {error}"));
                    }
                    self.end(host, branch, session)
                }
            },
            Event::Uploaded { host, branch, session, result } => match result {
                Ok(()) => {
                    if let Some(writer) = self.writer(host, branch, session) {
                        self.acknowledge(host, branch, session, writer);
                    }
                }
                Err(_) => self.end(host, branch, session),
            },
            Event::Acked { host, branch, session, result, head } => match result {
                Ok(_) => {
                    if let Some(head) = head.filter(|head| self.commits[head].branch == branch) {
                        self.branches.get_mut(&branch).unwrap().acked.insert(head);
                    }
                }
                Err(_) => self.end(host, branch, session),
            },
            Event::Failed { host, branch, session, error } => {
                if let Error::Lost(_) | Error::Corrupt(_) = error {
                    self.violation(format!("branch {branch:x}'s writer: {error}"));
                }
                self.end(host, branch, session);
            }
            Event::Deleted { branch, stale: false, result } => match result {
                Ok(()) => {}
                // A later placement took it as it was deleted: it lives on.
                Err(Error::Stale) => self.branches.get_mut(&branch).unwrap().deleting = false,
                Err(error) => self.violation(format!("deleting branch {branch:x}: {error}")),
            },
            Event::Deleted { branch, stale: true, result } => match result {
                Err(Error::Stale) => {}
                // It found the branch's lease final, which only its latest placement's delete,
                // begun since, writes: the branch is deleted, by that, as deletes are idempotent.
                Ok(()) if self.branches[&branch].deleting => {}
                Ok(()) => self.violation(format!("a placement cut off deleted branch {branch:x}")),
                Err(error) => self.violation(format!("branch {branch:x}'s stale delete: {error}")),
            },
            Event::Unforked { child, result: Ok(true) } => drop(self.unforks.remove(&child)),
            // A later placement took it as it was deleted, as a writer that went did: it lives on.
            Event::Unforked { child, result: Err(Error::Stale) } => {
                self.unforks.remove(&child);
                self.branches.get_mut(&child).unwrap().deleting = false;
            }
            Event::Unforked { child, result } => {
                self.violation(format!("deleting the failed fork to {child:x}: {result:?}"));
            }
            Event::Contested { child, owner, deleted, taken } => match (deleted, taken) {
                // The fork's deletion took the ID first.
                (Ok(true), Err(Error::Corrupt(_))) => {}
                // The other branch did: it lives on.
                (Ok(false), Ok(_)) => {
                    self.branches.insert(child, Branch::forked(None, None, false, owner));
                }
                (deleted, taken) => self.violation(format!(
                    "deleting the failed fork to {child:x} gave {deleted:?}, another branch taking \
                     its ID {taken:?}"
                )),
            },
            Event::Collected { id, result } => {
                self.collectors.remove(&id);
                if let Err(error) = result {
                    self.violation(format!("collector: {error}"));
                }
            }
            Event::Twinned { child, taken } => match taken {
                [(owner, Ok(())), (lost, Err(Error::ChildTaken(_)))]
                | [(lost, Err(Error::ChildTaken(_))), (owner, Ok(()))] => {
                    // Its fork's VM manager told: a branch from now on. The other's host deletes
                    // what it thinks its fork made.
                    self.branches.insert(child, Branch::forked(None, None, false, owner));
                    self.untwin(child, lost);
                }
                taken => self.violation(format!(
                    "two forks naming branch {child:x} took records of their own for it: {taken:?}"
                )),
            },
            Event::Untwinned { result: Ok(false), .. } => {}
            // The other fork's branch deleted meanwhile, as any branch may be: gone, whoever's.
            Event::Untwinned { child, result: Ok(true) } if self.branches[&child].deleting => {}
            Event::Untwinned { child, result } => self.violation(format!(
                "deleting a fork to branch {child:x}, which another fork took, gave {result:?}"
            )),
            Event::Restored { child, result } => {
                let Some(restore) = self.restores.remove(&child) else { return };
                let (point, origin, owner) = (restore.point, Some(restore.origin), restore.owner);
                let base = self.branches[&point].base;
                match result {
                    // Told to its VM's manager: a branch from now on, whatever comes of the point.
                    Ok(()) => {
                        let found = Branch::forked(origin, base, false, owner);
                        drop(self.branches.insert(child, found))
                    }
                    // The point went, or the fork was given up on: its host deletes it.
                    Err(Error::Deleted) => self.fail_restore(child, &restore, base),
                    Err(error) => self.violation(format!("forking point {point:x}: {error}")),
                }
            }
            Event::PointTaken { point, result } => match result {
                // A fork's child is no branch until it has its record, nor a point.
                Err(Error::Point | Error::Deleted | Error::NoBranch) => {}
                result => self.violation(format!("a writer of point {point:x}: {result:?}")),
            },
        }
    }

    /// `child`, a fork of a point that failed, or a crash left, goes as its host's cleanup deletes
    /// it, or an operator does what no host held: which nothing relies on.
    fn fail_restore(&mut self, child: u64, restore: &Restore, base: Option<u64>) {
        let Restore { point, origin, owner, .. } = *restore;
        let mut found = Branch::forked(Some(origin), base, false, owner);
        found.deleting = true;
        self.branches.insert(child, found);
        let forked = Kind::Forking { origin, point };
        self.unforks.insert(child, Unfork { forked, owner, running: None });
        self.spawn_unfork(child);
    }

    fn writer(&self, host: usize, branch: u64, session: u64) -> Option<Arc<SimWriter>> {
        let found = self.hosts[host].sessions.get(&branch).filter(|found| found.id == session)?;
        found.writer.clone()
    }

    /// Ends a session: its guest stops.
    fn end(&mut self, host: usize, branch: u64, session: u64) {
        let sessions = &mut self.hosts[host].sessions;
        if sessions.get(&branch).is_some_and(|found| found.id == session) {
            for task in sessions.remove(&branch).unwrap().tasks {
                task.abort();
            }
        }
    }

    /// A writer took `branch` on `host`: its guest starts from what the branch holds, and the
    /// commits on the host's disk past that, if the branch holds the disk's up to there, go again.
    fn opened(&mut self, host: usize, branch: u64, session: u64, writer: Arc<SimWriter>) {
        if !self.hosts[host].sessions.get(&branch).is_some_and(|found| found.id == session) {
            return;
        }
        let start = writer.state().last_commit;
        let disk = self.hosts[host].disk.get(&branch).cloned().unwrap_or_default();
        let own = start.filter(|tag| self.commits[tag].branch == branch);
        let suffix = match own {
            Some(tag) => disk.iter().position(|&on| on == tag).map(|at| disk[at + 1..].to_vec()),
            None => disk
                .first()
                .is_none_or(|first| self.commits[first].parent == start)
                .then(|| disk.clone()),
        }
        .unwrap_or_default();
        let mut path = self.own_path(branch, start);
        path.extend(&suffix);
        self.hosts[host].disk.insert(branch, path);
        let mut batch = Batch::default();
        for &tag in &suffix {
            let truth = &self.commits[&tag];
            let mut commit = Commit {
                tag,
                frees: truth.frees.clone(),
                root: truth.root.clone(),
                capacity: truth.capacity,
                ..Commit::default()
            };
            for (extent, bytes, own) in &truth.extents {
                seal(&mut commit, *extent, bytes.clone(), *own);
            }
            batch.commit(commit);
        }
        let found = self.hosts[host].sessions.get_mut(&branch).unwrap();
        (found.writer, found.batch) = (Some(writer.clone()), batch);
        if self.close_batch(host, branch, None).is_err() {
            return self.end(host, branch, session);
        }
        let head = suffix.last().copied().or(start);
        let (held, _) = self.expected(head);
        let found = self.hosts[host].sessions.get_mut(&branch).unwrap();
        found.guest = Guest { head, held, next_sequence: writer.chain().next_sequence() };
    }

    // Actions.

    /// A random session whose writer is open.
    fn ready(&mut self) -> Option<(usize, u64)> {
        let ready: Vec<(usize, u64)> = self
            .hosts
            .iter()
            .enumerate()
            .flat_map(|(host, found)| {
                found
                    .sessions
                    .iter()
                    .filter(|(_, session)| {
                        session.writer.as_ref().is_some_and(|w| w.stopped().is_none())
                    })
                    .map(move |(branch, _)| (host, *branch))
            })
            .collect();
        (!ready.is_empty()).then(|| ready[self.rng.random_range(0..ready.len())])
    }

    fn content(tag: u64, extent: ExtentId, len: usize) -> Vec<u8> {
        let seed = tag.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ extent.sequence.rotate_left(17)
            ^ extent.branch;
        (0..len).map(|i| (seed.rotate_left(i as u32 % 64) >> 3) as u8 ^ i as u8).collect()
    }

    fn commit(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let tag = self.id();
        let mut extents = Vec::new();
        let mut minted = Vec::new();
        for _ in 0..self.rng.random_range(0..3) {
            let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
            let extent = ExtentId::new(branch, session.guest.next_sequence);
            session.guest.next_sequence += 1;
            let len = self.rng.random_range(1..24);
            let own = self.rng.random_bool(0.25);
            extents.push((extent, Self::content(tag, extent, len), own));
            minted.push(extent);
        }
        let session = &self.hosts[host].sessions[&branch];
        let held: Vec<ExtentId> =
            session.guest.held.keys().copied().chain(minted.iter().copied()).collect();
        let mut frees = BTreeSet::new();
        for _ in 0..self.rng.random_range(0..3) {
            if !held.is_empty() {
                frees.insert(held[self.rng.random_range(0..held.len())]);
            }
        }
        let root = self.rng.random_bool(0.2).then(|| tag.to_le_bytes().to_vec());
        // Every fifth by its tag resizes the branch: by the tag, not the generator, so that the
        // seeds make the same runs as before resizes came (TASK-165.3).
        let capacity = (tag % 5 == 0).then_some(tag << 20);
        let mut commit = Commit {
            tag,
            frees: frees.iter().copied().collect(),
            root: root.clone(),
            capacity,
            ..Commit::default()
        };
        for (extent, bytes, own) in &extents {
            seal(&mut commit, *extent, bytes.clone(), *own);
        }
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        let parent = session.guest.head;
        session.batch.commit(commit);
        for extent in &minted {
            if let Some(other) = self.minted.insert(*extent, tag) {
                self.violation(format!("{extent:?} minted by commits {other} and {tag}"));
            }
        }
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        for (extent, bytes, _) in &extents {
            session.guest.held.insert(*extent, bytes.clone());
        }
        for extent in &frees {
            session.guest.held.remove(extent);
        }
        session.guest.head = Some(tag);
        self.hosts[host].disk.entry(branch).or_default().push(tag);
        self.commits.insert(
            tag,
            Truth { branch, parent, extents, frees: frees.into_iter().collect(), root, capacity },
        );
    }

    fn close(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let id = self.hosts[host].sessions[&branch].id;
        if self.close_batch(host, branch, None).is_err() {
            self.end(host, branch, id);
        }
    }

    /// Closes the batch of `branch`'s session on `host` on its writer's chain, with a fork of the
    /// branch to `child` at its end if there is one, of its owner, a point if it says so, has the
    /// writer take
    /// its segment, and uploads it: returns its number, if there was a segment to close, or why the
    /// writer refused it.
    fn close_batch(
        &mut self,
        host: usize,
        branch: u64,
        child: Option<Child>,
    ) -> Result<Option<u64>, Error> {
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        if child.is_none() && session.batch.is_empty() {
            return Ok(None);
        }
        let (id, writer) = (session.id, session.writer.clone().expect("an open writer"));
        let batch = std::mem::take(&mut session.batch);
        let mut chain = writer.chain();
        let closed = match child {
            Some(child) => batch.fork(child, &mut chain),
            None => batch.close(&mut chain),
        };
        let seq = writer.add_segment(closed.frame())?;
        self.upload(host, branch, id, writer, seq);
        Ok(Some(seq))
    }

    fn checkpoint(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let session = &self.hosts[host].sessions[&branch];
        let (id, writer) = (session.id, session.writer.clone().unwrap());
        let events = self.events.clone();
        self.spawn_session(host, branch, async move {
            if let Err(error) = writer.checkpoint().await {
                events.borrow_mut().push(Event::Failed { host, branch, session: id, error });
            }
        });
    }

    /// Forks a branch with a writer, as that writer's commit: to a point if `point`.
    fn fork(&mut self, point: bool) {
        let Some((host, branch)) = self.ready() else { return };
        let (child, owner) = (self.id(), self.id());
        let session = &self.hosts[host].sessions[&branch];
        let (id, base) = (session.id, session.guest.head);
        let forked = Child { branch: child, owner, point };
        let Ok(Some(seq)) = self.close_batch(host, branch, Some(forked)) else {
            return self.end(host, branch, id);
        };
        let origin = Origin { parent: branch, at: seq };
        self.branches.insert(child, Branch::forked(Some(origin), base, point, owner));
        if !rules().records_after_ack {
            let bucket = self.bucket.clone();
            self.spawn(async move {
                let _ = branch::record_fork(&*bucket, forked, origin).await;
            });
        }
    }

    /// The points with records, as a host took its copy of one: deleted since, perhaps, which the
    /// copy can't tell.
    fn points(&self) -> Vec<(u64, Origin)> {
        let direct = Direct(self.bucket.clone());
        (self.branches.iter())
            .filter(|(_, found)| found.point)
            .filter_map(|(&point, _)| {
                let record = block_on(branch::record(&direct, point)).ok()??;
                Some((point, record.origin()?))
            })
            .collect()
    }

    /// A host restores a point, as many do at once: writing nothing but its child's record.
    fn restore(&mut self) {
        let points = self.points();
        if points.is_empty() {
            return;
        }
        let (point, origin) = points[self.rng.random_range(0..points.len())];
        let (child, owner) = (self.id(), self.id());
        let host = self.rng.random_range(0..self.hosts.len());
        self.restores.insert(child, Restore { point, origin, host, owner, running: None });
        self.spawn_restore(child);
    }

    /// Makes the fork `child`, or goes on with one a crash stopped, as opening its handle does.
    fn spawn_restore(&mut self, child: u64) {
        let Restore { point, origin, owner, .. } = self.restores[&child];
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        let running = self.spawn(async move {
            let result = branch::fork_point(&*bucket, child, point, origin, owner).await;
            events.borrow_mut().push(Event::Restored { child, result });
        });
        self.restores.get_mut(&child).unwrap().running = Some(running);
    }

    /// A host's stale copy of a point tries to write it, as `extent_remote` allows no one to.
    fn write_point(&mut self) {
        let points = self.points();
        if points.is_empty() {
            return;
        }
        let (point, _) = points[self.rng.random_range(0..points.len())];
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let result = Writer::open(bucket, point, holder, holder).await.map(drop);
            events.borrow_mut().push(Event::PointTaken { point, result });
        });
    }

    /// A writer takes a branch on a host: one with no writer, or, `takeover`, one with.
    fn open(&mut self, takeover: bool) {
        let candidates: Vec<(u64, ())> = self
            .branches
            .iter()
            .filter(|(_, found)| !found.deleting && !found.point)
            .filter_map(|(&branch, _)| {
                let open = self.hosts.iter().any(|host| host.sessions.contains_key(&branch));
                (open == takeover).then_some((branch, ()))
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let (branch, _) = candidates[self.rng.random_range(0..candidates.len())];
        let elsewhere: Vec<usize> = (0..self.hosts.len())
            .filter(|&host| !self.hosts[host].sessions.contains_key(&branch))
            .collect();
        if elsewhere.is_empty() {
            return;
        }
        // Mostly where its commits are on disk.
        let with_disk: Vec<usize> = elsewhere
            .iter()
            .copied()
            .filter(|&host| self.hosts[host].disk.contains_key(&branch))
            .collect();
        let pool =
            if !with_disk.is_empty() && self.rng.random_bool(0.7) { with_disk } else { elsewhere };
        let host = pool[self.rng.random_range(0..pool.len())];
        if takeover && self.profile.hold > 0.0 && self.rng.random_bool(self.profile.hold) {
            let writers =
                self.hosts.iter().filter_map(|host| host.sessions.get(&branch)?.writer.clone());
            if let Some(epoch) = writers.map(|writer| writer.epoch()).max() {
                self.bucket.hold(branch, epoch);
            }
        }
        self.start_session(host, branch, None);
    }

    /// A placement cut off takes a branch back, on a host that has no writer of it, with an earlier
    /// fencing token than the branch's latest placement's: which must fail.
    fn reclaim(&mut self) {
        let candidates: Vec<(u64, u64)> = (self.branches.iter())
            .filter(|(_, found)| !found.deleting && found.taken > 1)
            .map(|(&branch, found)| (branch, found.taken))
            .collect();
        if candidates.is_empty() {
            return;
        }
        let (branch, taken) = candidates[self.rng.random_range(0..candidates.len())];
        let hosts: Vec<usize> = (0..self.hosts.len())
            .filter(|&host| !self.hosts[host].sessions.contains_key(&branch))
            .collect();
        if hosts.is_empty() {
            return;
        }
        let host = hosts[self.rng.random_range(0..hosts.len())];
        let token = self.rng.random_range(1..taken);
        self.start_session(host, branch, Some(token));
    }

    /// Starts a session of `branch` on `host`, its writer taking it over, of the placement whose
    /// fencing token is `token`, or, with none, of a new one, the latest.
    fn start_session(&mut self, host: usize, branch: u64, token: Option<u64>) {
        let (session, holder) = (self.id(), self.id());
        let token = token.unwrap_or(holder);
        let started = Session {
            id: session,
            writer: None,
            tasks: Vec::new(),
            guest: Guest::default(),
            batch: Batch::default(),
        };
        self.hosts[host].sessions.insert(branch, started);
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn_session(host, branch, async move {
            let result = Writer::open(bucket, branch, holder, token).await.map(Arc::new);
            events.borrow_mut().push(Event::Opened { host, branch, session, token, result });
        });
    }

    fn crash(&mut self) {
        let host = self.rng.random_range(0..self.hosts.len());
        for (_, session) in std::mem::take(&mut self.hosts[host].sessions) {
            for task in session.tasks {
                task.abort();
            }
        }
        if self.rng.random_bool(0.3) {
            self.hosts[host].disk.clear();
        }
        // Whichever host's: what they had in flight may land still.
        for unfork in self.unforks.values_mut() {
            if let Some(running) = unfork.running.take() {
                running.abort();
            }
        }
        for restore in self.restores.values_mut().filter(|restore| restore.host == host) {
            if let Some(running) = restore.running.take() {
                running.abort();
            }
        }
    }

    /// Deletes a fork that failed, one no writer took, as its host's cleanup does, in object storage
    /// ([`branch::delete_fork`]): or goes on with one a crash stopped, as deleting its handle again
    /// does.
    fn unfork(&mut self) {
        let stopped = self.unforks.iter().find(|(_, unfork)| unfork.running.is_none());
        if let Some((&child, _)) = stopped {
            return self.spawn_unfork(child);
        }
        // What a crash left of a fork of a point: its handle opened, or deleted.
        let stopped = self.restores.iter().find(|(_, restore)| restore.running.is_none());
        if let Some((&child, _)) = stopped {
            return match self.rng.random_bool(0.5) {
                true => self.spawn_restore(child),
                false => self.give_up_restore(child),
            };
        }
        let held = |branch| self.hosts.iter().any(|host| host.sessions.contains_key(branch));
        let forked = |origin: Option<Origin>, parent| origin.is_some_and(|at| at.parent == parent);
        let candidates: Vec<u64> = (self.branches.iter())
            .filter(|(_, found)| found.origin.is_some() && found.acked.is_empty())
            .filter(|(branch, found)| !found.deleting && found.taken == 0 && !held(branch))
            .filter(|&(&branch, _)| {
                !self.branches.values().any(|child| forked(child.origin, branch))
            })
            .map(|(&branch, _)| branch)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let child = candidates[self.rng.random_range(0..candidates.len())];
        let found = self.branches.get_mut(&child).unwrap();
        found.deleting = true;
        let forked = match found.origin {
            Some(origin) if found.point => Kind::Point(origin),
            Some(origin) => Kind::Fork(origin),
            None => Kind::Own,
        };
        let owner = found.owner;
        self.unforks.insert(child, Unfork { forked, owner, running: None });
        self.spawn_unfork(child);
    }

    /// Deletes what a crash left of the fork of a point to `child`.
    fn give_up_restore(&mut self, child: u64) {
        let restore = self.restores.remove(&child).unwrap();
        let base = self.branches[&restore.point].base;
        self.fail_restore(child, &restore, base);
    }

    /// Deletes `child`, a failed fork's, as its host does.
    fn spawn_unfork(&mut self, child: u64) {
        let Unfork { forked, owner, .. } = self.unforks[&child];
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        let running = self.spawn(async move {
            let result = branch::delete_fork(&*bucket, child, forked, owner, holder, 0).await;
            events.borrow_mut().push(Event::Unforked { child, result });
        });
        self.unforks.get_mut(&child).unwrap().running = Some(running);
    }

    /// Deletes a fork whose segment never went, which names no origin, as another branch takes its
    /// child's ID, the guest having named one that was free when the fork looked: the same token
    /// for both, as a host's stores' are.
    fn contest(&mut self) {
        let (child, holder, other) = (self.id(), self.id(), self.id());
        let (forks, owner) = (self.id(), self.id());
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let deleting = branch::delete_fork(&*bucket, child, Kind::Own, forks, holder, 0);
            let taking = async {
                branch::create(&*bucket, child, owner).await?;
                branch::take_lease(&*bucket, &crate::layout::lease(child), other, 0).await
            };
            let (deleted, taken) = futures::join!(deleting, taking);
            events.borrow_mut().push(Event::Contested { child, owner, deleted, taken });
        });
    }

    /// Two forks name one child, of other parents or on other hosts, within a round trip, neither's
    /// segment landing, as guests may give any ID: each takes a record of its own for it, as the
    /// child of a fork not made in object storage does ([`branch::create_forked`]), of its owner.
    /// One must take it, the other finding it another's (TASK-79.82).
    fn twins(&mut self) {
        let (child, first, second) = (self.id(), self.id(), self.id());
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let (taken, also) = futures::join!(
                branch::create_forked(&*bucket, child, first),
                branch::create_forked(&*bucket, child, second)
            );
            events
                .borrow_mut()
                .push(Event::Twinned { child, taken: [(first, taken), (second, also)] });
        });
    }

    /// Deletes the fork of `owner` to `child`, whose segment never went, as its host's cleanup does
    /// once its child found another fork's record: which must leave that be.
    fn untwin(&mut self, child: u64, owner: u64) {
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let result = branch::delete_fork(&*bucket, child, Kind::Own, owner, holder, 0).await;
            events.borrow_mut().push(Event::Untwinned { child, result });
        });
    }

    /// Deletes a branch, if `bases`, one a fork of which lives.
    fn delete(&mut self, bases: bool) {
        let live = |found: &Branch| !found.deleting;
        // Keep a branch or two.
        if self.branches.values().filter(|found| live(found)).count() < 3 {
            return;
        }
        let candidates: Vec<u64> = self
            .branches
            .iter()
            .filter(|(_, found)| live(found))
            .filter(|&(&branch, _)| {
                !bases
                    || self.branches.values().any(|child| {
                        live(child) && child.origin.is_some_and(|origin| origin.parent == branch)
                    })
            })
            .map(|(&branch, _)| branch)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let branch = candidates[self.rng.random_range(0..candidates.len())];
        let found = self.branches.get_mut(&branch).unwrap();
        found.deleting = true;
        // Its latest placement's, as far as anything here knows: one taking it meanwhile is later.
        let token = found.taken;
        self.spawn_delete(branch, token, false);
    }

    /// A placement cut off deletes a branch, with an earlier fencing token than the branch's latest
    /// placement's: which must fail, the branch living on.
    fn stale_delete(&mut self) {
        let candidates: Vec<(u64, u64)> = (self.branches.iter())
            .filter(|(_, found)| !found.deleting && found.taken > 1)
            .map(|(&branch, found)| (branch, found.taken))
            .collect();
        if candidates.is_empty() {
            return;
        }
        let (branch, taken) = candidates[self.rng.random_range(0..candidates.len())];
        let token = self.rng.random_range(1..taken);
        self.spawn_delete(branch, token, true);
    }

    /// Deletes `branch` with the fencing token `token`, a placement cut off's if `stale`.
    fn spawn_delete(&mut self, branch: u64, token: u64, stale: bool) {
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let result = match branch::delete(&*bucket, branch, holder, token).await {
                // A fork whose segment never made it: no branch.
                Err(Error::NoBranch) => Ok(()),
                result => result,
            };
            events.borrow_mut().push(Event::Deleted { branch, stale, result });
        });
    }

    fn collect(&mut self) {
        if self.collectors.len() >= 2 {
            return;
        }
        let id = self.id();
        let (bucket, events, options) =
            (self.bucket.clone(), self.events.clone(), self.profile.options);
        let handle = self.spawn(async move {
            let result = collector::collect(&*bucket, id, &options).await;
            events.borrow_mut().push(Event::Collected { id, result });
        });
        self.collectors.insert(id, handle);
    }

    fn stop_collector(&mut self) {
        if let Some((&id, _)) = self.collectors.iter().next() {
            self.collectors.remove(&id).unwrap().abort();
        }
    }

    // The truth.

    /// The commits of `branch` up to `head`, oldest first.
    fn own_path(&self, branch: u64, head: Option<u64>) -> Vec<u64> {
        let mut path = Vec::new();
        let mut at = head;
        while let Some(tag) = at.filter(|tag| self.commits[tag].branch == branch) {
            path.push(tag);
            at = self.commits[&tag].parent;
        }
        path.reverse();
        path
    }

    /// The extents and root the commits up to `head` leave.
    fn expected(&self, head: Option<u64>) -> (BTreeMap<ExtentId, Vec<u8>>, Option<Vec<u8>>) {
        let mut path = Vec::new();
        let mut at = head;
        while let Some(tag) = at {
            path.push(tag);
            at = self.commits[&tag].parent;
        }
        let mut extents = BTreeMap::new();
        let mut root = None;
        for tag in path.into_iter().rev() {
            let truth = &self.commits[&tag];
            for (extent, bytes, _) in &truth.extents {
                extents.insert(*extent, bytes.clone());
            }
            for extent in &truth.frees {
                extents.remove(extent);
            }
            if truth.root.is_some() {
                root = truth.root.clone();
            }
        }
        (extents, root)
    }

    /// The capacity the commits up to `head` leave: the last one set, a fork's parent's before it.
    fn capacity(&self, head: Option<u64>) -> Option<u64> {
        let mut at = head;
        while let Some(tag) = at {
            let truth = &self.commits[&tag];
            if truth.capacity.is_some() {
                return truth.capacity;
            }
            at = truth.parent;
        }
        None
    }

    /// The sequence numbers of `branch`'s own extents that the commits up to `head` freed once a
    /// commit before held them, which it writes no more (TASK-79.6, TASK-165.3): not those a commit
    /// sealed and freed itself, which no commit held.
    fn spent(&self, branch: u64, head: Option<u64>) -> Spent {
        let mut spent = Spent::default();
        let mut at = head;
        while let Some(tag) = at {
            let truth = &self.commits[&tag];
            let sealed =
                |extent: &ExtentId| truth.extents.iter().any(|(sealed, ..)| sealed == extent);
            for extent in truth.frees.iter().filter(|extent| extent.branch == branch) {
                if !sealed(extent) {
                    spent.insert(extent.sequence);
                }
            }
            at = truth.parent;
        }
        spent
    }

    /// Whether `ancestor` is `head` or before it.
    fn precedes(&self, ancestor: u64, head: Option<u64>) -> bool {
        let mut at = head;
        while let Some(tag) = at {
            if tag == ancestor {
                return true;
            }
            at = self.commits[&tag].parent;
        }
        false
    }

    /// Checks that every live branch holds what its commits made, up to one of its own, past
    /// everything its guests were told was durable, and that the current writers read what their
    /// guests hold.
    pub fn check(&mut self) {
        let direct = Arc::new(Direct(self.bucket.clone()));
        let mut violations = Vec::new();
        let keys: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        // Nothing writes a point: no segment of its, no checkpoint, no lease but its deletion's.
        for (&id, _) in self.branches.iter().filter(|(_, branch)| branch.point) {
            let written = |prefix: String| keys.iter().any(|key| key.starts_with(&prefix));
            if written(crate::layout::segments(id)) || written(crate::layout::checkpoints(id)) {
                violations.push(format!("point {id:x} was written"));
            }
            if let Some(object) = block_on(crate::bucket::get(&*direct, &crate::layout::lease(id)))
                && crate::layout::Lease::decode(&object.bytes)
                    .is_ok_and(|lease| lease.epoch != crate::layout::Lease::FINAL)
            {
                violations.push(format!("point {id:x} has a writer's lease"));
            }
        }
        for (&id, branch) in &self.branches {
            // A failed fork's, being deleted: no guest knows of it.
            if self.unforks.contains_key(&id) {
                continue;
            }
            let record = match block_on(branch::record(&*direct, id)) {
                Ok(Some(record)) if !record.deleted => record,
                Ok(Some(_)) if !branch.deleting => {
                    violations
                        .push(format!("branch {id:x} is deleted, though no delete of it held"));
                    continue;
                }
                Ok(_) => continue,
                Err(error) => {
                    violations.push(format!("branch {id:x}'s record: {error}"));
                    continue;
                }
            };
            if record.origin() != branch.origin {
                violations.push(format!("branch {id:x}'s record says {:?}", record.origin()));
            }
            if record.is_point() != branch.point {
                violations.push(format!("branch {id:x}'s record says {:?}", record.kind));
            }
            if record.forking().is_some() {
                violations.push(format!("branch {id:x}, told made, is being made still"));
            }
            // No placement took it back from a later one.
            match block_on(crate::bucket::get(&*direct, &crate::layout::lease(id))) {
                Some(object) => match crate::layout::Lease::decode(&object.bytes) {
                    Ok(lease) if lease.token < branch.taken => violations.push(format!(
                        "branch {id:x}'s lease is an earlier placement's: token {} after {}",
                        lease.token, branch.taken
                    )),
                    Ok(_) => {}
                    Err(error) => violations.push(format!("branch {id:x}'s lease: {error}")),
                },
                None if branch.taken > 0 => {
                    violations.push(format!("branch {id:x}, taken, has no lease"));
                }
                None => {}
            }
            let recovered = match block_on(recovery::recover(&*direct, id, record.origin())) {
                Ok(recovered) => recovered,
                Err(error) => {
                    violations.push(format!("branch {id:x}: {error}"));
                    continue;
                }
            };
            let state = &recovered.state;
            // Every acknowledged segment's objects exist: what recovering the branch read, and
            // what holds each extent it names, or the moves that say where that went.
            for key in &recovered.touched {
                if !keys.contains(key) {
                    violations.push(format!("branch {id:x} needs {key}, which is gone"));
                }
            }
            for (extent, location) in &state.extents {
                let moves = crate::layout::moves(&location.key);
                if !keys.contains(&location.key) && !keys.contains(&moves) {
                    violations.push(format!("{extent:?} of {id:x}: {} is gone", location.key));
                }
            }
            let head = state.last_commit;
            let own = self.own_path(id, head);
            let start = own.first().map_or(head, |first| self.commits[first].parent);
            if start != branch.base {
                violations.push(format!("branch {id:x} holds commit {head:?}, not of its history"));
                continue;
            }
            for &acked in &branch.acked {
                if !self.precedes(acked, head) {
                    violations.push(format!("branch {id:x} lost commit {acked}, acknowledged"));
                }
            }
            let (extents, root) = self.expected(head);
            if state.root != root {
                violations.push(format!("branch {id:x}'s root is {:?}, not {root:?}", state.root));
            }
            let spent = self.spent(id, head);
            if state.spent != spent {
                violations.push(format!("branch {id:x} spent {:?}, not {spent:?}", state.spent));
            }
            let capacity = self.capacity(head);
            if state.capacity != capacity {
                let found = state.capacity;
                violations.push(format!("branch {id:x}'s capacity is {found:?}, not {capacity:?}"));
            }
            let held: Vec<_> = state.extents.keys().collect();
            let made: Vec<_> = extents.keys().collect();
            if held != made {
                violations.push(format!("branch {id:x} holds {held:?}, its commits made {made:?}"));
                continue;
            }
            for (extent, location) in &state.extents {
                match block_on(crate::read(&direct, *extent, location)) {
                    Ok(bytes) if bytes == extents[extent] => {}
                    Ok(_) => violations.push(format!("branch {id:x}: {extent:?} reads wrong")),
                    Err(error) => violations.push(format!("branch {id:x}: {error}")),
                }
            }
        }
        // The writers no later one fenced read what their guests hold.
        for host in &self.hosts {
            for (&id, session) in &host.sessions {
                let Some(writer) = &session.writer else { continue };
                let lease = block_on(branch::lease_epoch(&*direct, &crate::layout::lease(id)));
                if writer.stopped().is_some() || lease != Ok(writer.epoch()) {
                    continue;
                }
                for (extent, location) in &writer.state().extents {
                    let Some(bytes) = session.guest.held.get(extent) else { continue };
                    match block_on(crate::read(&direct, *extent, location)) {
                        Ok(read) if read == *bytes => {}
                        Ok(_) => violations
                            .push(format!("branch {id:x}'s writer: {extent:?} reads wrong")),
                        Err(error) => violations.push(format!("branch {id:x}'s writer: {error}")),
                    }
                }
            }
        }
        self.violations.extend(violations);
    }

    // The end.

    /// Requests complete, without failing, until none is left, those held too.
    fn settle(&mut self) {
        self.bucket.release();
        loop {
            self.settle_tasks();
            if self.bucket.waiting() == 0 {
                return;
            }
            self.bucket.complete(0, Fault::None);
        }
    }

    /// The live branches, by their records.
    fn live(&self) -> Vec<u64> {
        let direct = Direct(self.bucket.clone());
        self.branches
            .keys()
            .copied()
            .filter(|&id| matches!(block_on(branch::record(&direct, id)), Ok(Some(record)) if !record.deleted))
            .collect()
    }

    /// Stops the guests, lets every request and task finish, takes every live branch over with a
    /// new writer, then has each write a checkpoint while collectors run, a few times over.
    fn quiesce(&mut self) {
        self.faults = false;
        self.settle();
        // Forks of points a crash stopped, or a lost disk left with no host: deleted.
        let stopped = self.restores.keys().copied().collect::<Vec<_>>();
        for child in stopped {
            self.give_up_restore(child);
        }
        // Those a crash stopped, their hosts up again.
        let stopped = self.unforks.iter().filter(|(_, unfork)| unfork.running.is_none());
        for child in stopped.map(|(&child, _)| child).collect::<Vec<_>>() {
            self.spawn_unfork(child);
        }
        self.settle();
        // Twice, so that extents a writer uploaded again, which it alone can tell are its, belong
        // to an earlier writer.
        for _ in 0..2 {
            for host in 0..self.hosts.len() {
                for (_, session) in std::mem::take(&mut self.hosts[host].sessions) {
                    for task in session.tasks {
                        task.abort();
                    }
                }
            }
            self.settle();
            self.quiescing = true;
            // Twice: opening a parent gives its forks' children their records.
            for _ in 0..2 {
                for branch in self.live() {
                    let held = self.hosts.iter().any(|host| host.sessions.contains_key(&branch));
                    if held || self.branches[&branch].point {
                        continue;
                    }
                    let host = self
                        .hosts
                        .iter()
                        .position(|host| host.disk.contains_key(&branch))
                        .unwrap_or(0);
                    self.start_session(host, branch, None);
                }
                self.settle();
            }
        }
        for _ in 0..4 {
            let sessions: Vec<(usize, u64)> = self
                .hosts
                .iter()
                .enumerate()
                .flat_map(|(host, found)| found.sessions.keys().map(move |&branch| (host, branch)))
                .collect();
            for &(host, branch) in &sessions {
                let Some(session) = self.hosts[host].sessions.get(&branch) else { continue };
                if session.writer.is_none() {
                    continue;
                }
                let parent = session.guest.head;
                let tag = self.id();
                let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
                session.batch.commit(Commit { tag, ..Commit::default() });
                if self.close_batch(host, branch, None).is_err() {
                    self.violation(format!("branch {branch:x}'s last writer stopped"));
                    return;
                }
                self.commits.insert(
                    tag,
                    Truth {
                        branch,
                        parent,
                        extents: Vec::new(),
                        frees: Vec::new(),
                        root: None,
                        capacity: None,
                    },
                );
                self.hosts[host].sessions.get_mut(&branch).unwrap().guest.head = Some(tag);
            }
            self.settle();
            for &(host, branch) in &sessions {
                if let Some(writer) =
                    self.hosts[host].sessions.get(&branch).and_then(|s| s.writer.clone())
                {
                    self.spawn(async move {
                        let _ = writer.checkpoint().await;
                    });
                }
            }
            self.settle();
            self.bucket.advance(self.profile.options.grace + 1);
            let id = self.id();
            let (bucket, options) = (self.bucket.clone(), self.profile.options);
            let reported = Rc::new(RefCell::new(None));
            let report = reported.clone();
            let before: BTreeSet<String> = self.bucket.keys().into_iter().collect();
            self.spawn(async move {
                *report.borrow_mut() = Some(collector::collect(&*bucket, id, &options).await);
            });
            self.settle();
            self.check_report(&before, reported.take());
        }
        self.check();
        self.check_garbage();
        self.check_compacted();
    }

    /// Checks that a pass of the collector that ran alone, the bucket holding `before` as it
    /// started, reports what it did (TASK-44.1): the objects it deleted, and the moves it wrote.
    fn check_report(
        &mut self,
        before: &BTreeSet<String>,
        report: Option<Result<collector::Report, Error>>,
    ) {
        let after: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        let moves =
            |key: &&String| matches!(crate::layout::kind(key), crate::layout::Kind::Moves { .. });
        let expected = collector::Report {
            deleted: before.difference(&after).count(),
            compacted: after.difference(before).filter(moves).count(),
        };
        match report {
            Some(Ok(report)) if report == expected => {}
            Some(Ok(report)) => {
                self.violation(format!("collector reported {report:?}, having done {expected:?}"))
            }
            Some(Err(error)) => self.violation(format!("collector: {error}")),
            None => self.violation("a collector alone did not finish".to_owned()),
        }
    }

    /// The places the way to `extent` at `location` goes through, of a bucket holding `keys`: as a
    /// point's goes for as long as it lives, as it has no writer to record moves in a checkpoint.
    fn way(
        direct: &Direct,
        maps: &mut crate::moves::MovesMaps,
        keys: &BTreeSet<String>,
        extent: ExtentId,
        location: &crate::Location,
    ) -> Vec<crate::Location> {
        let mut way = Vec::new();
        let mut at = location.clone();
        while keys.contains(&crate::layout::moves(&at.key)) {
            match block_on(maps.moved(direct, extent, &at.key)) {
                Ok(crate::moves::Moved::To(to)) => {
                    way.push(to.clone());
                    at = to;
                }
                _ => break,
            }
        }
        way
    }

    /// Checks that the collectors compacted what they could (TASK-44.1): no object a live branch
    /// names, which recovering none reads, has its live extents take less than `compact_below` of
    /// its data, unless its moves are written, which the branches' next checkpoints record.
    fn check_compacted(&mut self) {
        use crate::layout::{self, Kind};
        let direct = Direct(self.bucket.clone());
        let keys: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        let mut maps = crate::moves::MovesMaps::default();
        let mut touched = BTreeSet::new();
        let mut named: BTreeMap<String, BTreeMap<ExtentId, u64>> = BTreeMap::new();
        for branch in self.live() {
            let origin = self.branches[&branch].origin;
            let Ok(recovered) = block_on(recovery::recover(&direct, branch, origin)) else {
                continue;
            };
            touched.extend(recovered.touched);
            for (&extent, location) in &recovered.state.extents {
                named.entry(location.key.clone()).or_default().insert(extent, location.len);
                if self.branches[&branch].point {
                    for place in Self::way(&direct, &mut maps, &keys, extent, location) {
                        named.entry(place.key).or_default().insert(extent, place.len);
                    }
                }
            }
        }
        for (key, extents) in named {
            let compactable =
                matches!(layout::kind(&key), Kind::Segment { .. } | Kind::Moves { .. });
            if !compactable || touched.contains(&key) || keys.contains(&layout::moves(&key)) {
                continue;
            }
            let Ok(object) = block_on(crate::bucket::Bucket::get(&direct, &key)) else { continue };
            let Ok(data) = layout::data(&object.bytes) else { continue };
            let used: u64 = extents.values().sum();
            if (used as f64) < self.profile.options.compact_below * data.len() as f64 {
                self.violation(format!(
                    "left uncompacted: {key}, {used} of its {} bytes live",
                    data.len()
                ));
            }
        }
    }

    /// What the bucket holds and what the live branches name, for a failure's message.
    pub fn describe(&self) -> String {
        let direct = Direct(self.bucket.clone());
        let mut text = String::new();
        for branch in self.live() {
            let origin = self.branches[&branch].origin;
            match block_on(recovery::recover(&direct, branch, origin)) {
                Ok(recovered) => {
                    let keys: BTreeSet<_> =
                        recovered.state.extents.values().map(|location| &location.key).collect();
                    text += &format!(
                        "branch {branch:x} from {origin:?}: checkpoint {:?}, end {}, epoch {}, names {keys:?}\n",
                        recovered.checkpoint, recovered.end, recovered.state.epoch
                    );
                }
                Err(error) => text += &format!("branch {branch:x}: {error}\n"),
            }
        }
        for key in self.bucket.keys() {
            text += &format!("  {key}\n");
            if let crate::layout::Kind::Segment { .. } = crate::layout::kind(&key)
                && let Ok(object) = block_on(crate::bucket::Bucket::get(&direct, &key))
                && let Ok(footer) = crate::layout::Segment::footer(&object.bytes)
            {
                let short = |extents: &mut dyn Iterator<Item = ExtentId>| {
                    extents
                        .map(|extent| format!("{:x}", extent.sequence))
                        .collect::<Vec<_>>()
                        .join(",")
                };
                text += &format!(
                    "    epoch {} commits {:?} extents [{}] own [{}] frees [{}] forks {:?}\n",
                    footer.epoch,
                    footer.contents.commits,
                    short(&mut footer.contents.extents.iter().map(|e| e.0)),
                    short(&mut footer.own().map(|e| e.0)),
                    short(&mut footer.contents.frees.iter().copied()),
                    footer.contents.forks,
                );
            }
        }
        for (tag, truth) in &self.commits {
            let short = |extents: &mut dyn Iterator<Item = ExtentId>| {
                extents.map(|extent| format!("{:x}", extent.sequence)).collect::<Vec<_>>().join(",")
            };
            text += &format!(
                "commit {tag} of {:x} after {:?}: extents [{}] frees [{}]\n",
                truth.branch,
                truth.parent,
                short(&mut truth.extents.iter().map(|e| e.0)),
                short(&mut truth.frees.iter().copied()),
            );
        }
        text
    }

    /// Checks that the bucket holds only what live branches need: their records and leases, what
    /// recovering them reads, and the objects holding their extents.
    fn check_garbage(&mut self) {
        let direct = Direct(self.bucket.clone());
        let mut needed = BTreeSet::from([crate::layout::GC_LEASE.to_owned()]);
        let keys: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        let mut maps = crate::moves::MovesMaps::default();
        for branch in self.live() {
            needed.insert(crate::layout::record(branch));
            needed.insert(crate::layout::lease(branch));
            let origin = self.branches[&branch].origin;
            let Ok(recovered) = block_on(recovery::recover(&direct, branch, origin)) else {
                continue;
            };
            needed.extend(recovered.touched);
            for (extent, location) in &recovered.state.extents {
                needed.insert(location.key.clone());
                if !self.branches[&branch].point {
                    continue;
                }
                for place in Self::way(&direct, &mut maps, &keys, *extent, location) {
                    needed.insert(crate::layout::moves(&place.key));
                    needed.insert(place.key);
                }
            }
        }
        for key in self.bucket.keys() {
            // Deleted forks' records stay.
            if let crate::layout::Kind::Record { branch } = crate::layout::kind(&key)
                && let Ok(Some(record)) = block_on(branch::record(&direct, branch))
                && record.deleted
                && !record.may_drop()
            {
                continue;
            }
            if !needed.contains(&key) {
                self.violation(format!("garbage left: {key}"));
            }
        }
    }
}
