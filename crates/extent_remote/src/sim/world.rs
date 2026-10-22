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
use crate::layout::Origin;
use crate::writer::Writer;
use crate::{Error, branch, reader, rules};
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
}

/// A commit, as its guest made it.
struct Truth {
    branch: u64,
    parent: Option<u64>,
    /// Its extents: their bytes, and whether stored on their own.
    extents: Vec<(ExtentId, Vec<u8>, bool)>,
    frees: Vec<ExtentId>,
    root: Option<Vec<u8>>,
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
    Collected {
        id: u64,
        result: Result<collector::Report, Error>,
    },
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
            next_id: 1,
            faults: true,
            quiescing: false,
            violations: Vec::new(),
        };
        let root = world.id();
        block_on(branch::create(&Direct(world.bucket.clone()), root)).unwrap();
        world.branches.insert(
            root,
            Branch { origin: None, base: None, acked: BTreeSet::new(), deleting: false, taken: 0 },
        );
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
            4 => self.fork(),
            5 => self.open(false),
            6 => self.open(true),
            7 => self.crash(),
            8 => self.delete(false),
            9 => self.delete(true),
            10 => self.collect(),
            11 => self.stop_collector(),
            12 => self.reclaim(),
            13 => self.stale_delete(),
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
                Ok(()) => self.violation(format!("a placement cut off deleted branch {branch:x}")),
                Err(error) => self.violation(format!("branch {branch:x}'s stale delete: {error}")),
            },
            Event::Collected { id, result } => {
                self.collectors.remove(&id);
                if let Err(error) = result {
                    self.violation(format!("collector: {error}"));
                }
            }
        }
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
                ..Commit::default()
            };
            for (extent, bytes, own) in &truth.extents {
                (if *own { &mut commit.own } else { &mut commit.extents })
                    .push((*extent, bytes.clone()));
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
        let mut commit = Commit {
            tag,
            frees: frees.iter().copied().collect(),
            root: root.clone(),
            ..Commit::default()
        };
        for (extent, bytes, own) in &extents {
            (if *own { &mut commit.own } else { &mut commit.extents })
                .push((*extent, bytes.clone()));
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
            Truth { branch, parent, extents, frees: frees.into_iter().collect(), root },
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
    /// branch to `child` at its end if there is one, has the writer take its segment, and uploads
    /// it: returns its number, if there was a segment to close, or why the writer refused it.
    fn close_batch(
        &mut self,
        host: usize,
        branch: u64,
        child: Option<u64>,
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

    fn fork(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let child = self.id();
        let session = &self.hosts[host].sessions[&branch];
        let (id, base) = (session.id, session.guest.head);
        let Ok(Some(seq)) = self.close_batch(host, branch, Some(child)) else {
            return self.end(host, branch, id);
        };
        let origin = Origin { parent: branch, at: seq };
        self.branches.insert(
            child,
            Branch {
                origin: Some(origin),
                base,
                acked: BTreeSet::new(),
                deleting: false,
                taken: 0,
            },
        );
        if !rules().records_after_ack {
            let bucket = self.bucket.clone();
            self.spawn(async move {
                let _ = branch::record_fork(&*bucket, child, origin).await;
            });
        }
    }

    /// A writer takes a branch on a host: one with no writer, or, `takeover`, one with.
    fn open(&mut self, takeover: bool) {
        let candidates: Vec<(u64, ())> = self
            .branches
            .iter()
            .filter(|(_, found)| !found.deleting)
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
        let direct = Direct(self.bucket.clone());
        let mut violations = Vec::new();
        for (&id, branch) in &self.branches {
            let record = match block_on(branch::record(&direct, id)) {
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
            if record.origin != branch.origin {
                violations.push(format!("branch {id:x}'s record says {:?}", record.origin));
            }
            // No placement took it back from a later one.
            match block_on(crate::bucket::get(&direct, &crate::layout::lease(id))) {
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
            let recovered = match block_on(reader::recover(&direct, id, record.origin)) {
                Ok(recovered) => recovered,
                Err(error) => {
                    violations.push(format!("branch {id:x}: {error}"));
                    continue;
                }
            };
            let state = &recovered.state;
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
            let held: Vec<_> = state.extents.keys().collect();
            let made: Vec<_> = extents.keys().collect();
            if held != made {
                violations.push(format!("branch {id:x} holds {held:?}, its commits made {made:?}"));
                continue;
            }
            for (extent, location) in &state.extents {
                match block_on(reader::read(&direct, *extent, location)) {
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
                let lease = block_on(branch::lease_epoch(&direct, &crate::layout::lease(id)));
                if writer.stopped().is_some() || lease != Ok(writer.epoch()) {
                    continue;
                }
                for (extent, location) in &writer.state().extents {
                    let Some(bytes) = session.guest.held.get(extent) else { continue };
                    match block_on(reader::read(&direct, *extent, location)) {
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

    /// Requests complete, without failing, until none is left.
    fn settle(&mut self) {
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
                    if self.hosts.iter().any(|host| host.sessions.contains_key(&branch)) {
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
                    Truth { branch, parent, extents: Vec::new(), frees: Vec::new(), root: None },
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

    /// Checks that the collectors compacted what they could (TASK-44.1): no object a live branch
    /// names, which recovering none reads, has its live extents take less than `compact_below` of
    /// its data, unless its moves are written, which the branches' next checkpoints record.
    fn check_compacted(&mut self) {
        use crate::layout::{self, Kind};
        let direct = Direct(self.bucket.clone());
        let mut touched = BTreeSet::new();
        let mut named: BTreeMap<String, BTreeMap<ExtentId, u64>> = BTreeMap::new();
        for branch in self.live() {
            let origin = self.branches[&branch].origin;
            let Ok(recovered) = block_on(reader::recover(&direct, branch, origin)) else {
                continue;
            };
            touched.extend(recovered.touched);
            for (&extent, location) in &recovered.state.extents {
                named.entry(location.key.clone()).or_default().insert(extent, location.len);
            }
        }
        let keys: BTreeSet<String> = self.bucket.keys().into_iter().collect();
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
            match block_on(reader::recover(&direct, branch, origin)) {
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
                    footer.commits,
                    short(&mut footer.extents.iter().map(|e| e.0)),
                    short(&mut footer.own.iter().map(|e| e.0)),
                    short(&mut footer.frees.iter().copied()),
                    footer.forks,
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
        for branch in self.live() {
            needed.insert(crate::layout::record(branch));
            needed.insert(crate::layout::lease(branch));
            let origin = self.branches[&branch].origin;
            let Ok(recovered) = block_on(reader::recover(&direct, branch, origin)) else {
                continue;
            };
            needed.extend(recovered.touched);
            for location in recovered.state.extents.values() {
                needed.insert(location.key.clone());
            }
        }
        for key in self.bucket.keys() {
            // Deleted forks' records stay.
            if let crate::layout::Kind::Record { branch } = crate::layout::kind(&key)
                && let Ok(Some(record)) = block_on(branch::record(&direct, branch))
                && record.deleted
                && record.forked
            {
                continue;
            }
            if !needed.contains(&key) {
                self.violation(format!("garbage left: {key}"));
            }
        }
    }
}
