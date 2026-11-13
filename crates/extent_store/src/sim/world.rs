// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The simulated world: hosts whose stores keep branches for their guests, each a [`Store`] over a
//! directory of the host's disk in memory, which does its I/O in the order its calls make
//! ([`MemDir::serial`]), and its uploader, `upload`'s, uploading its write-ahead log to the bucket;
//! forks, takeovers, deletions and collectors; driven by a seeded scheduler that also decides when,
//! in what order and how each request to the bucket completes, and when each uploader's interval
//! runs out. After every step it checks every live branch against what its guests did and were
//! told (TASK-88: the segments, their footers and the logs are the store's, as a host's are).

use crate::upload::{self, Clock, Failure, Shared};
use crate::{Description, MemDir, Place, Recovering, Store};
use extent_frame::spent::Spent;
use extent_remote::collector::{self, Options};
use extent_remote::fetch::{Budget, Limits};
use extent_remote::layout::{self, Child, Origin, epoch_of, writer_of};
use extent_remote::record::Kind;
use extent_remote::testing::{Direct, Fault, SimBucket};
use extent_remote::{Error, Recovered, Writer, branch, recovery, rules};
use futures::channel::oneshot;
use futures::executor::{LocalPool, LocalSpawner, block_on};
use futures::future::{AbortHandle, FutureExt as _, abortable};
use futures::task::LocalSpawnExt as _;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use storage_device::ExtentId;

/// How likely each action is, per step.
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    /// A request to the bucket completes.
    pub complete: u32,
    /// A guest writes an extent, which its next commit seals.
    pub write: u32,
    pub commit: u32,
    /// A store's uploader is asked for its commits, as a commit waiting for its upload asks, or
    /// its interval runs out: either closes the store's open segment.
    pub close: u32,
    pub checkpoint: u32,
    pub fork: u32,
    /// A writer takes a branch that has none.
    pub open: u32,
    /// A writer takes a branch from the one it has.
    pub takeover: u32,
    /// A host crashes, its disk keeping what was durable, or lost, its stores with it.
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
    /// A root takes the ID of a fork's child whose segment is not yet acknowledged, as a guest may
    /// name any, and is deleted (TASK-174).
    pub usurp: u32,
    /// A writer whose session stopped, a later one taking its branch or its host cut off, which
    /// runs on unaware, checkpoints: late, past where the next writer recovered from (TASK-175).
    pub stale_checkpoint: u32,
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
    /// Whether a host that crashes, its disk kept, starts again at once, taking back the branches
    /// it had, as extentd opens its stores at boot: while the creates its writers had in flight
    /// land.
    pub restarts: bool,
    /// fxdax: the chance that a collector's pass is over one branch's cell alone, not a sweep over
    /// every cell (TASK-395.4): none draws nothing of the seed's.
    pub one_cell: f64,
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
            restarts: false,
            one_cell: 0.5,
            weights: Weights {
                complete: 60,
                write: 6,
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
                usurp: 0,
                stale_checkpoint: 0,
                tick: 3,
            },
            options: Options { compact_below: 0.5, grace: 4 },
        }
    }
}

/// The block size of the hosts' stores: small, so that a few blocks make an extent, and segments
/// of a few extents close, full, between commits.
/// fxdax: the simulation's branch `id`: the one volume of a cell of its own, as a store made alone,
/// or a fork, is (TASK-364).
fn sim_branch(id: u64) -> extent_remote::layout::Branch {
    let cell = extent_remote::layout::Cell { tenant: 1, id };
    extent_remote::layout::Branch { cell, id }
}

const BLOCK: u64 = 512;

/// How the hosts' stores keep their branches: in segments of a few extents, those of three blocks
/// or more in objects of their own. No segment closes for its age, which the store's clock, not
/// the world's, would decide.
fn store_options() -> crate::Options {
    crate::Options {
        max_extent: 4 * BLOCK,
        segment_size: 6 * BLOCK,
        segment_age: Duration::from_secs(1 << 30),
        large_extent: 3 * BLOCK,
        open_files: 4,
        view_lag: None,
        view_kept_bytes: None,
        // Flushes of either kind: records holding their runs, and checking them (TASK-301).
        held_most: 2 * BLOCK,
    }
}

/// The capacity of the hosts' stores: what a commit gives a branch that has none yet.
const CAPACITY: u64 = 1 << 40;

/// The uploaders' interval, which only the world's [`Intervals`] run out.
const INTERVAL: Duration = Duration::from_secs(1);

/// Says what happened, to debug a seed, if `SIM_TRACE` is set.
macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("SIM_TRACE").is_some() {
            eprintln!($($arg)*);
        }
    };
}

/// How many of the writers of the sessions stopped the world keeps ([`World::stale`]).
const STALE_WRITERS: usize = 4;

/// How many PUTs a host's writers have in flight at once ([`Host::uploads`]).
const UPLOADS_AT_ONCE: usize = 2;

type Uploader = Shared<MemDir, SimBucket>;

/// When a session's uploader's interval runs out: only when the world says ([`World::close`]), so
/// that a run's seed alone decides what its uploaders do.
#[derive(Default)]
struct Intervals {
    waiting: Mutex<Vec<oneshot::Sender<()>>>,
}

impl Intervals {
    /// The uploader's clock: each interval it waits out runs out at the next [`Intervals::run_out`].
    fn clock(self: &Arc<Self>) -> Clock {
        let intervals = self.clone();
        Arc::new(move |_| {
            let (ran_out, running) = oneshot::channel();
            let mut waiting = intervals.waiting.lock().unwrap();
            waiting.retain(|waiting| !waiting.is_canceled());
            waiting.push(ran_out);
            running.map(drop).boxed()
        })
    }

    /// Runs out the intervals waited out now.
    fn run_out(&self) {
        for ran_out in std::mem::take(&mut *self.waiting.lock().unwrap()) {
            let _ = ran_out.send(());
        }
    }
}

struct Branch {
    origin: Option<Origin>,
    /// The parent's commit the fork made, which the child's first follows.
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
    /// A branch forked from `origin`, its parent's fork commit `base`, a point if `point`, of
    /// `owner`.
    fn forked(origin: Option<Origin>, base: Option<u64>, point: bool, owner: u64) -> Self {
        Self { origin, base, acked: BTreeSet::new(), deleting: false, taken: 0, point, owner }
    }
}

/// A commit, as its guest made it.
struct Truth {
    branch: u64,
    parent: Option<u64>,
    /// The extents it sealed, and their bytes.
    extents: Vec<(ExtentId, Vec<u8>)>,
    frees: Vec<ExtentId>,
    root: Option<Vec<u8>>,
    /// The branch's capacity, if it sets it (TASK-165.3).
    capacity: Option<u64>,
}

/// A branch's store's directory on a host's disk: its write-ahead log, open segment and index, as
/// a crash leaves them.
struct Disk {
    dir: MemDir,
    /// Whether its store was made whole ([`Store::made`]).
    made: bool,
    /// The history of commits its store holds ([`World::lineages`]): a new one once its log is
    /// dropped.
    lineage: u64,
}

struct Host {
    /// Each branch's store's directory (doc-14), by branch.
    disks: HashMap<u64, Disk>,
    sessions: BTreeMap<u64, Session>,
    /// The PUTs its writers upload objects with, as a host's share one budget
    /// (`extent_store::upload`): a few at once, so that uploads wait for each other's to be done,
    /// and are woken as they are (TASK-149.32).
    uploads: Arc<Budget>,
}

impl Default for Host {
    fn default() -> Self {
        let limits = Limits { requests: UPLOADS_AT_ONCE, bytes: u64::MAX };
        Self { disks: HashMap::new(), sessions: BTreeMap::new(), uploads: Budget::new(limits) }
    }
}

/// A branch's store on a host, its uploader, and its guest.
struct Session {
    id: u64,
    /// The fencing token of the placement it is.
    token: u64,
    /// The history of commits its store holds ([`Disk::lineage`]).
    lineage: u64,
    uploader: Arc<Uploader>,
    intervals: Arc<Intervals>,
    /// What opening the store gave, once it did.
    started: mpsc::Receiver<Result<Arc<Store<MemDir>>, Failure>>,
    /// The store, once open.
    store: Option<Arc<Store<MemDir>>>,
    /// The uploader's task.
    running: AbortHandle,
    guest: Guest,
}

#[derive(Default)]
struct Guest {
    head: Option<u64>,
    held: BTreeMap<ExtentId, Vec<u8>>,
    /// What it wrote since its last commit, which the next seals.
    written: BTreeMap<ExtentId, Vec<u8>>,
    next_sequence: u64,
}

enum Event {
    /// A writer's checkpoint failed.
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
    /// What a root of `owner` taking the ID of the child of a fork, whose segment was not yet
    /// acknowledged ([`World::usurp`]), gave, and deleting it after.
    Usurped {
        child: u64,
        owner: u64,
        created: Result<(), Error>,
        deleted: Option<Result<(), Error>>,
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
    /// The fork whose child's ID its restore took ([`Restore::taken`]): the child's, if the fork's
    /// record came first.
    taken: Option<Branch>,
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
    /// The fork whose child's ID it took, a guest having named it, that fork's segment not yet
    /// acknowledged: the child's, if that fork's record comes first (TASK-173).
    taken: Option<Branch>,
}

/// What a segment's footer says of the branch's history: the segment it follows, by its number and
/// epoch, and its commits' tags.
type Applied = (Option<(u64, u64)>, Vec<u64>);

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
    /// The commit, or write, that minted each extent.
    minted: HashMap<ExtentId, u64>,
    /// Each history of commits a store held, by the store's epoch of each: what the tags of the
    /// commits its writers upload name ([`World::truth_of`]).
    lineages: HashMap<u64, BTreeMap<u64, u64>>,
    /// The history the commits each writer uploads are of, by its branch and lease epoch: a writer
    /// uploads one store's.
    stamps: HashMap<(u64, u64), u64>,
    /// The forks the guests made, by their children: of which branch, as each guest named it. Its
    /// origin is where the uploader that took its segment put it, again after a takeover.
    forking: BTreeMap<u64, (u64, Child)>,
    collectors: BTreeMap<u64, AbortHandle>,
    /// The writers of the sessions stopped, the latest few, which may run on unaware
    /// ([`World::stale_checkpoint`]).
    stale: Vec<Arc<Writer<SimBucket>>>,
    unforks: BTreeMap<u64, Unfork>,
    restores: BTreeMap<u64, Restore>,
    /// The forks whose children's IDs a root is taking ([`World::usurp`]), until it is done: the
    /// child's, if the fork's record comes first.
    usurped: BTreeMap<u64, Branch>,
    /// What the footer of each segment the history check read says, by its branch, number and
    /// epoch, one writer stamping one footer at a place.
    footers: RefCell<HashMap<(u64, u64, u64), Applied>>,
    next_id: u64,
    /// Whether requests fail.
    faults: bool,
    /// Whether everything is finishing: then every live branch must open.
    quiescing: bool,
    /// The tasks spawned and not done, aborted or dropped, by number, each with where it was
    /// spawned: one still here once every request is done waits for a wake nothing will give
    /// (TASK-149.32). The uploaders, which wait for commits for as long as their stores are open,
    /// are not among them: that each uploads what it is asked to is checked of it
    /// ([`World::check_uploaded`]).
    tasks: Rc<RefCell<BTreeMap<u64, &'static std::panic::Location<'static>>>>,
    next_task: u64,
    pub violations: Vec<String>,
}

/// A task's place in [`World::tasks`], which it gives up as it is dropped: done, or aborted.
struct Running {
    tasks: Rc<RefCell<BTreeMap<u64, &'static std::panic::Location<'static>>>>,
    number: u64,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.tasks.borrow_mut().remove(&self.number);
    }
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
            lineages: HashMap::new(),
            stamps: HashMap::new(),
            forking: BTreeMap::new(),
            collectors: BTreeMap::new(),
            stale: Vec::new(),
            unforks: BTreeMap::new(),
            restores: BTreeMap::new(),
            usurped: BTreeMap::new(),
            footers: RefCell::default(),
            next_id: 1,
            faults: true,
            quiescing: false,
            tasks: Rc::default(),
            next_task: 0,
            violations: Vec::new(),
        };
        let (root, owner) = (world.id(), world.id());
        block_on(branch::create(&Direct(world.bucket.clone()), sim_branch(root), owner)).unwrap();
        world.branches.insert(root, Branch::forked(None, None, false, owner));
        world
    }

    /// What the run asked of the bucket, in order ([`SimBucket::journal`]).
    pub fn journal(&self) -> Vec<String> {
        self.bucket.journal()
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
            w.usurp,
            w.write,
            w.stale_checkpoint,
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
            20 => self.usurp(),
            21 => self.write(),
            22 => self.stale_checkpoint(),
            _ => {
                let ticks = self.rng.random_range(1..=self.profile.options.grace);
                self.bucket.advance(ticks);
            }
        }
        self.settle_tasks();
    }

    /// Runs tasks until they wait for requests, handling what they report and taking in what the
    /// uploaders did.
    fn settle_tasks(&mut self) {
        loop {
            self.pool.run_until_stalled();
            let events = std::mem::take(&mut *self.events.borrow_mut());
            let observed = self.observe();
            if events.is_empty() && !observed {
                return;
            }
            for event in events {
                self.handle(event);
            }
        }
    }

    // Tasks.

    /// Spawns an uploader's `task`, which waits for its store's commits for as long as it is
    /// open: counted among no [`World::tasks`].
    fn spawn_uploader(&mut self, task: impl Future<Output = ()> + 'static) -> AbortHandle {
        let (task, handle) = abortable(task);
        self.spawner
            .spawn_local(async move {
                let _ = task.await;
            })
            .unwrap();
        handle
    }

    fn handle(&mut self, event: Event) {
        match event {
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
            // A restore's that took a fork's child's ID, the fork's record coming first: the
            // fork's child, as it was.
            Event::Unforked { child, result: Ok(false) }
                if self.unforks[&child].taken.is_some() =>
            {
                let taken = self.unforks.remove(&child).unwrap().taken.unwrap();
                self.branches.insert(child, taken);
            }
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
                [(owner, Ok(())), (lost, Err(Error::IdTaken(_)))]
                | [(lost, Err(Error::IdTaken(_))), (owner, Ok(()))] => {
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
                let Some(mut restore) = self.restores.remove(&child) else { return };
                let (point, origin, owner) = (restore.point, Some(restore.origin), restore.owner);
                let base = self.branches[&point].base;
                match result {
                    // Told to its VM's manager: a branch from now on, whatever comes of the point.
                    Ok(()) => {
                        let found = Branch::forked(origin, base, false, owner);
                        drop(self.branches.insert(child, found))
                    }
                    // The fork whose child's ID it took recorded it first: the fork's child, as it
                    // was.
                    Err(Error::IdTaken(_)) if restore.taken.is_some() => {
                        drop(self.branches.insert(child, restore.taken.take().unwrap()))
                    }
                    // The point went, or the fork was given up on: its host deletes it.
                    Err(Error::Deleted) => self.fail_restore(child, restore, base),
                    Err(error) => self.violation(format!("forking point {point:x}: {error}")),
                }
            }
            Event::Usurped { child, owner, created, deleted } => match (created, deleted) {
                // The root took the ID, and is gone.
                (Ok(()), Some(Ok(()))) => {
                    self.usurped.remove(&child);
                    let mut found = Branch::forked(None, None, false, owner);
                    found.deleting = true;
                    drop(self.branches.insert(child, found))
                }
                // The fork's record came first: its child, as it was.
                (Err(Error::Corrupt(_)), None) => {
                    let fork = self.usurped.remove(&child).unwrap();
                    drop(self.branches.insert(child, fork))
                }
                (created, deleted) => self.violation(format!(
                    "a root taking the ID of the fork to {child:x} gave {created:?}, deleting it \
                     {deleted:?}"
                )),
            },
            Event::PointTaken { point, result } => match result {
                // A fork's child is no branch until it has its record, nor a point.
                Err(Error::Point | Error::Deleted | Error::NoBranch) => {}
                result => self.violation(format!("a writer of point {point:x}: {result:?}")),
            },
        }
    }

    // The stores and their uploaders.

    /// Takes in what each session's uploader did since: its store opened, or refused to; its
    /// uploader stopped; the lease epoch of its writer, which the tags of the commits it uploads
    /// name; where it put the forks of the segments it took; and the commits it uploaded, which
    /// their guests are told are durable. Returns whether a session opened, ended or started anew,
    /// which the next run of the tasks may go on with.
    fn observe(&mut self) -> bool {
        let mut changed = false;
        for host in 0..self.hosts.len() {
            let branches: Vec<u64> = self.hosts[host].sessions.keys().copied().collect();
            for branch in branches {
                let session = &self.hosts[host].sessions[&branch];
                let id = session.id;
                if session.store.is_none() {
                    match session.started.try_recv() {
                        Ok(Ok(store)) => self.opened(host, branch, store),
                        Ok(Err(failure)) => self.refused(host, branch, failure),
                        Err(mpsc::TryRecvError::Empty) => continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            unreachable!("an uploader stopped without saying how its store opened")
                        }
                    }
                    changed = true;
                    continue;
                }
                if let Some(failure) = session.uploader.failed() {
                    trace!("host {host}: session {id} of {branch:x}'s uploader stopped: {failure}");
                    if let Failure::Remote(Error::Lost(_) | Error::Corrupt(_)) = failure {
                        self.violation(format!("branch {branch:x}'s uploader: {failure}"));
                    }
                    self.end(host, branch, id);
                    changed = true;
                    continue;
                }
                let (lineage, uploader) = (session.lineage, session.uploader.clone());
                if let Some(writer) = uploader.writer() {
                    self.stamps.insert((branch, writer.epoch()), lineage);
                }
                self.forks_taken(branch, &uploader);
                // Its guest is told of what was uploaded, its own commits.
                let uploaded = uploader.last_uploaded();
                let commits = &self.commits;
                let acked: Vec<u64> = (self.lineages[&lineage].range(..=uploaded))
                    .map(|(_, &tag)| tag)
                    .filter(|tag| commits[tag].branch == branch)
                    .collect();
                if let Some(found) = self.branches.get_mut(&branch) {
                    found.acked.extend(acked);
                }
            }
        }
        changed
    }

    /// Notes where `uploader`, of `branch`'s store, put the forks of the segments it took: their
    /// children's origins, as the uploader would delete them by.
    fn forks_taken(&mut self, branch: u64, uploader: &Uploader) {
        let forks: Vec<(Child, Origin)> = (self.forking.values())
            .filter(|(parent, _)| *parent == branch)
            .filter_map(|(_, child)| Some((*child, uploader.fork_origin(child.branch)?)))
            .collect();
        for (child, origin) in forks {
            let Some(found) = self.fork_child(&child) else { continue };
            if found.origin == Some(origin) {
                continue;
            }
            found.origin = Some(origin);
            if !rules().records_after_ack {
                let bucket = self.bucket.clone();
                self.spawn(async move {
                    let _ = branch::record_fork(&*bucket, child, origin).await;
                });
            }
        }
    }

    /// The directory of `branch`'s store on `host`, made if its disk has none.
    fn disk(&mut self, host: usize, branch: u64) -> &mut Disk {
        if !self.hosts[host].disks.contains_key(&branch) {
            let dir = MemDir::serial();
            let description = Description {
                block_size: BLOCK as u32,
                capacity: CAPACITY,
                branch: sim_branch(branch),
            };
            Store::make(&dir, description, Some("sim:///"), None).unwrap();
            let lineage = self.id();
            self.lineages.insert(lineage, BTreeMap::new());
            self.hosts[host].disks.insert(branch, Disk { dir, made: false, lineage });
        }
        self.hosts[host].disks.get_mut(&branch).unwrap()
    }

    /// Starts a session of `branch` on `host`: its store opened, its writer taking the branch, of
    /// the placement whose fencing token is `token`, or, with none, of a new one, the latest.
    fn start_session(&mut self, host: usize, branch: u64, token: Option<u64>) {
        let disk = self.disk(host, branch);
        let dir = disk.dir.clone();
        let recovering = match disk.made {
            true => Store::recover(dir),
            false => Store::recover_unmade(dir),
        };
        match recovering {
            Ok(recovering) => self.run_session(host, branch, token, recovering),
            Err(err) => self.violation(format!("recovering branch {branch:x}'s store: {err:#}")),
        }
    }

    /// Starts a session of `branch` on `host` that opens the store `recovering` recovered, as
    /// [`World::start_session`] does.
    fn run_session(
        &mut self,
        host: usize,
        branch: u64,
        token: Option<u64>,
        recovering: Recovering<MemDir>,
    ) {
        let (id, holder) = (self.id(), self.id());
        let token = token.unwrap_or(holder);
        let intervals = Arc::<Intervals>::default();
        let options = upload::Options {
            interval: INTERVAL,
            max_lag: u64::MAX,
            max_lag_bytes: u64::MAX,
            cache: None,
            token,
            checkpoint_every: upload::CHECKPOINT_EVERY,
            // The interval closes a segment filled at once, as the first full one closed between
            // commits, its commits uploading, is what the world's clock can't wait for
            // (TASK-92.41).
            filling_wait: Duration::ZERO,
        };
        let uploads = self.hosts[host].uploads.clone();
        let uploader = Shared::new(options, uploads, intervals.clock());
        let (started, start) = mpsc::channel();
        let (running, bucket) = (uploader.clone(), self.bucket.clone());
        let running = self.spawn_uploader(async move {
            running.run(recovering, store_options(), bucket, holder, started).await;
        });
        let lineage = self.hosts[host].disks[&branch].lineage;
        trace!("host {host}: session {id} of {branch:x} starts, token {token}, history {lineage}");
        let session = Session {
            id,
            token,
            lineage,
            uploader,
            intervals,
            started: start,
            store: None,
            running,
            guest: Guest::default(),
        };
        self.hosts[host].sessions.insert(branch, session);
    }

    /// `branch`'s store on `host` opened, its writer having taken the branch: its log put after
    /// the branch's end, what it holds that landed acknowledged, the rest stamped anew. Its guest
    /// starts from its last commit, which the store's history holds, or the bucket's.
    fn opened(&mut self, host: usize, branch: u64, store: Arc<Store<MemDir>>) {
        let disk = self.hosts[host].disks.get_mut(&branch).unwrap();
        if !disk.made {
            if let Err(err) = store.made() {
                return self.violation(format!("making branch {branch:x}'s store: {err:#}"));
            }
            disk.made = true;
        }
        let session = &self.hosts[host].sessions[&branch];
        let (token, lineage, uploader) = (session.token, session.lineage, session.uploader.clone());
        let writer = uploader.writer().expect("a writer took the branch");
        let epoch = match store.epoch() {
            Ok(epoch) => epoch,
            Err(err) => return self.violation(format!("branch {branch:x}'s store: {err:#}")),
        };
        // Its last commit: its log's, past the branch's end, or the branch's, its log holding
        // nothing more: the store's history then, if it holds that commit, or the bucket's, the
        // commits its history holds past it lost, a crash taking them before they were durable.
        let last = writer.state().last_commit;
        let history = &self.lineages[&lineage];
        let head = match last.map_or(0, epoch_of) == epoch {
            false => history.get(&epoch).copied(),
            true => match self.truth_of(branch, last) {
                Ok(head) => head,
                Err(violation) => return self.violation(violation),
            },
        };
        let lineage = if head.is_some() && history.get(&epoch).copied() != head {
            let lineage = self.id();
            self.lineages.insert(lineage, BTreeMap::new());
            self.hosts[host].disks.get_mut(&branch).unwrap().lineage = lineage;
            lineage
        } else {
            lineage
        };
        self.stamps.insert((branch, writer.epoch()), lineage);
        let history = self.lineages.get_mut(&lineage).unwrap();
        history.retain(|&at, _| at <= epoch);
        if let Some(head) = head {
            history.insert(epoch, head);
        }
        let found = self.branches.get_mut(&branch).unwrap();
        found.taken = found.taken.max(token);
        let (held, _) = self.expected(head);
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        let next_sequence = uploader.next_sequence();
        trace!(
            "host {host}: session {} of {branch:x} opened by writer {} at epoch {epoch}, the \
             branch's last {last:?}: head {head:?}, history {lineage}",
            session.id,
            writer.epoch()
        );
        session.lineage = lineage;
        session.guest = Guest { head, held, written: BTreeMap::new(), next_sequence };
        session.store = Some(store);
    }

    /// `branch`'s store on `host` did not open, for `failure`: its log diverged, another writer's
    /// commits past it, which `fxdaxctl take` drops, and it opens again with none; else its guest
    /// goes. fxdax: a log diverged must have: the branch's last commit is neither one of its
    /// commits nor its base. Before, every divergence was taken as one, which hid a log whose
    /// segment ending one commit and going on with the next landed alone, its next commit, durable,
    /// dropped (TASK-345).
    fn refused(&mut self, host: usize, branch: u64, failure: Failure) {
        trace!("host {host}: {branch:x}'s store refused: {failure}");
        let session = self.hosts[host].sessions.remove(&branch).unwrap();
        let token = session.token;
        self.stop(host, branch, session);
        if let Failure::Remote(Error::Diverged) = failure {
            let disk = self.hosts[host].disks.get_mut(&branch).unwrap();
            let mut recovering = match Store::recover(disk.dir.clone()) {
                Ok(recovering) => recovering,
                Err(err) => return self.violation(format!("branch {branch:x}'s store: {err:#}")),
            };
            let origin = self.branches[&branch].origin;
            let direct = Direct(self.bucket.clone());
            if let Ok(recovered) = block_on(recovery::recover(&direct, sim_branch(branch), origin))
                && recovering.holds_commit(recovered.state.last_commit)
            {
                let last = recovered.state.last_commit;
                return self.violation(format!(
                    "branch {branch:x}'s log diverged, though the branch's last commit, {last:x?}, \
                     is the log's"
                ));
            }
            if let Err(err) = recovering.drop_wal() {
                return self.violation(format!("dropping branch {branch:x}'s log: {err:#}"));
            }
            let lineage = self.id();
            self.lineages.insert(lineage, BTreeMap::new());
            self.hosts[host].disks.get_mut(&branch).unwrap().lineage = lineage;
            return self.run_session(host, branch, Some(token), recovering);
        }
        if self.quiescing {
            self.violation(format!("branch {branch:x} does not open: {failure}"));
        }
    }

    /// The commit the guests made that `tag`, a commit's in a footer or a state of `branch`'s,
    /// names: the tag names the lease epoch of the writer that uploaded it, which says which
    /// store's history it is of, and the store's epoch of it there. A fork's child names its
    /// parent's commits until it has its own.
    fn truth_of(&self, branch: u64, tag: Option<u64>) -> Result<Option<u64>, String> {
        let Some(tag) = tag else { return Ok(None) };
        let mut at = Some(branch);
        while let Some(of) = at {
            if let Some(lineage) = self.stamps.get(&(of, writer_of(tag)))
                && let Some(&commit) = self.lineages[lineage].get(&epoch_of(tag))
            {
                return Ok(Some(commit));
            }
            at = self
                .branches
                .get(&of)
                .and_then(|found| found.origin)
                .map(|origin| origin.parent.id);
        }
        Err(format!("branch {branch:x} names commit {tag:#x}, which no guest made"))
    }

    /// Ends a session: its guest stops.
    fn end(&mut self, host: usize, branch: u64, session: u64) {
        let sessions = &mut self.hosts[host].sessions;
        if sessions.get(&branch).is_some_and(|found| found.id == session) {
            let ended = sessions.remove(&branch).unwrap();
            self.stop(host, branch, ended);
        }
    }

    /// Stops `session`, of `branch` on `host`, taken out of the host's sessions: its uploader, as
    /// its host dying stops it, and its store, which closes as it goes, making its last commits
    /// durable, before anything opens its directory again; a crash took what its disk keeps
    /// before ([`World::crash`]).
    fn stop(&mut self, host: usize, branch: u64, session: Session) {
        trace!("host {host}: session {} of {branch:x} stops", session.id);
        session.running.abort();
        self.forks_taken(branch, &session.uploader);
        if let Some(writer) = session.uploader.writer() {
            self.stale.push(writer);
            if self.stale.len() > STALE_WRITERS {
                self.stale.remove(0);
            }
        }
        let Session { uploader, store, .. } = session;
        let closing = store.as_ref().map(Arc::downgrade);
        let going = Arc::downgrade(&uploader);
        drop((uploader, store));
        // The uploader's task, aborted, lets go of the store as it is next polled.
        self.pool.run_until_stalled();
        assert!(
            closing.as_ref().is_none_or(|store| store.strong_count() == 0),
            "branch {branch:x}'s store on host {host} is open still, its session stopped: {} \
             holds of it, {} of its uploader",
            closing.as_ref().map_or(0, std::sync::Weak::strong_count),
            going.strong_count(),
        );
    }

    /// `branch`'s store on `host` failed doing what its guest asked, for `err`: which it may only
    /// once its uploader stopped.
    fn store_failed(&mut self, host: usize, branch: u64, err: anyhow::Error) {
        let session = &self.hosts[host].sessions[&branch];
        let (id, stopped) = (session.id, session.uploader.failed().is_some());
        if !stopped {
            self.violation(format!("branch {branch:x}'s store on host {host}: {err:#}"));
        }
        self.end(host, branch, id);
    }

    /// Checks that every session's uploader uploads what its store committed, asked to, as
    /// everything settles (TASK-149.32): one that doesn't waits for a wake nothing will give.
    fn check_uploaded(&mut self) {
        trace!("checking uploads");
        let mut asked = Vec::new();
        for host in &self.hosts {
            for (&branch, session) in &host.sessions {
                let Some(store) = &session.store else { continue };
                let epoch = store.epoch().unwrap();
                let _ = session.uploader.ask(epoch, false);
                asked.push((branch, Arc::downgrade(&session.uploader), epoch));
            }
        }
        self.settle();
        for (branch, uploader, epoch) in asked {
            // Its session ended meanwhile, which observing its uploader's failure ends.
            let Some(uploader) = uploader.upgrade() else { continue };
            if uploader.failed().is_none() && uploader.last_uploaded() < epoch {
                self.violation(format!(
                    "branch {branch:x}'s commits through {epoch} wait to be uploaded, everything \
                     settled: uploaded through {}",
                    uploader.last_uploaded()
                ));
            }
        }
    }

    // Actions.

    /// A random session whose store is open, and its writer.
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
                        session.store.is_some()
                            && session.uploader.failed().is_none()
                            && session.uploader.writer().is_some_and(|w| w.stopped().is_none())
                    })
                    .map(move |(branch, _)| (host, *branch))
            })
            .collect();
        (!ready.is_empty()).then(|| ready[self.rng.random_range(0..ready.len())])
    }

    fn write(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let seed = self.id();
        self.write_extent(host, branch, seed);
    }

    /// The guest of `branch`'s store on `host` writes an extent of its own, its bytes of `seed`:
    /// whether it could.
    fn write_extent(&mut self, host: usize, branch: u64, seed: u64) -> bool {
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        let extent = ExtentId::new(branch, session.guest.next_sequence);
        session.guest.next_sequence += 1;
        let len = self.rng.random_range(1..=4) * BLOCK as usize;
        let bytes = Self::content(seed, extent, len);
        let store = session.store.clone().unwrap();
        if let Err(err) = store.write(extent, 0, &bytes) {
            self.store_failed(host, branch, err);
            return false;
        }
        session.guest.written.insert(extent, bytes);
        if let Some(other) = self.minted.insert(extent, seed) {
            self.violation(format!("{extent:?} minted by {other} and {seed}"));
        }
        true
    }

    /// The store's epoch of the commit `store` just made, which follows `before`'s.
    fn committed(&mut self, branch: u64, store: &Store<MemDir>, before: u64) -> Option<u64> {
        let epoch = store.epoch().ok()?;
        if epoch <= before {
            self.violation(format!("branch {branch:x}'s commit after {before} made epoch {epoch}"));
            return None;
        }
        Some(epoch)
    }

    /// The guest of `branch`'s store on `host` made the commit `tag`, sealing what it wrote,
    /// freeing `frees`, the store's epoch of it `epoch`.
    fn made(&mut self, host: usize, branch: u64, tag: u64, epoch: u64, mut made: Truth) {
        trace!("host {host}: {branch:x} commits {tag} at epoch {epoch}, after {:?}", made.parent);
        // A branch with no capacity yet takes its store's.
        if made.capacity.is_none() && self.capacity(made.parent).is_none() {
            made.capacity = Some(CAPACITY);
        }
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        self.lineages.get_mut(&session.lineage).unwrap().insert(epoch, tag);
        for (extent, bytes) in &made.extents {
            session.guest.held.insert(*extent, bytes.clone());
        }
        for extent in &made.frees {
            session.guest.held.remove(extent);
        }
        session.guest.head = Some(tag);
        self.commits.insert(tag, made);
    }

    fn commit(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let tag = self.id();
        for _ in 0..self.rng.random_range(0..3) {
            if !self.write_extent(host, branch, tag) {
                return;
            }
        }
        let session = &self.hosts[host].sessions[&branch];
        let held: Vec<ExtentId> =
            session.guest.held.keys().chain(session.guest.written.keys()).copied().collect();
        let mut frees = BTreeSet::new();
        for _ in 0..self.rng.random_range(0..3) {
            if !held.is_empty() {
                frees.insert(held[self.rng.random_range(0..held.len())]);
            }
        }
        let root = self.rng.random_bool(0.2).then(|| tag.to_le_bytes().to_vec());
        // Every fifth by its tag resizes the branch first, a commit of its own, which seals none
        // of what the guest wrote: by the tag, not the generator (TASK-165.3).
        if tag.is_multiple_of(5) && !self.resize(host, branch, tag << 20) {
            return;
        }
        let session = &self.hosts[host].sessions[&branch];
        // Durable at once, or later, with a later commit's, or a closing's.
        let durable = self.rng.random_bool(0.5);
        let store = session.store.clone().unwrap();
        let parent = session.guest.head;
        let committing = || -> anyhow::Result<u64> {
            let before = store.epoch()?;
            for extent in &frees {
                store.free(*extent)?;
            }
            store.commit_ordered(root.as_deref())?;
            if durable {
                store.flush()?;
            }
            Ok(before)
        };
        let before = match committing() {
            Ok(before) => before,
            Err(err) => return self.store_failed(host, branch, err),
        };
        let Some(epoch) = self.committed(branch, &store, before) else { return };
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        let extents = std::mem::take(&mut session.guest.written).into_iter().collect();
        let frees = frees.into_iter().collect();
        let made = Truth { branch, parent, extents, frees, root, capacity: None };
        self.made(host, branch, tag, epoch, made);
    }

    /// The guest of `branch`'s store on `host` sets its capacity to `capacity`: a commit of its
    /// own, which seals none of what it wrote. Whether it could.
    fn resize(&mut self, host: usize, branch: u64, capacity: u64) -> bool {
        let tag = self.id();
        let session = &self.hosts[host].sessions[&branch];
        let (store, parent) = (session.store.clone().unwrap(), session.guest.head);
        let resizing = || -> anyhow::Result<u64> {
            let before = store.epoch()?;
            store.set_capacity(capacity)?;
            Ok(before)
        };
        let before = match resizing() {
            Ok(before) => before,
            Err(err) => {
                self.store_failed(host, branch, err);
                return false;
            }
        };
        let Some(epoch) = self.committed(branch, &store, before) else { return false };
        let capacity = Some(capacity);
        let made =
            Truth { branch, parent, extents: Vec::new(), frees: Vec::new(), root: None, capacity };
        self.made(host, branch, tag, epoch, made);
        true
    }

    /// A store's uploader is asked for its commits, as a commit waiting for its upload asks, or its
    /// interval runs out: its open segment closes, and it uploads.
    fn close(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let session = &self.hosts[host].sessions[&branch];
        if self.rng.random_bool(0.5) {
            return session.intervals.run_out();
        }
        let (store, uploader) = (session.store.clone().unwrap(), session.uploader.clone());
        let asked =
            store.epoch().map_err(Failure::from).and_then(|epoch| uploader.ask(epoch, false));
        if let Err(failure) = asked {
            self.store_failed(host, branch, anyhow::Error::new(failure));
        }
    }

    /// A writer whose session stopped checkpoints, as one running on unaware of it would: a later
    /// writer's, or none, holding its branch's lease.
    fn stale_checkpoint(&mut self) {
        if self.stale.is_empty() {
            return;
        }
        let writer = self.stale[self.rng.random_range(0..self.stale.len())].clone();
        self.spawn(async move {
            let _ = writer.checkpoint().await;
        });
    }

    fn checkpoint(&mut self) {
        let Some((host, branch)) = self.ready() else { return };
        let session = &self.hosts[host].sessions[&branch];
        let (id, writer) = (session.id, session.uploader.writer().unwrap());
        let events = self.events.clone();
        self.spawn(async move {
            if let Err(error) = writer.checkpoint().await {
                events.borrow_mut().push(Event::Failed { host, branch, session: id, error });
            }
        });
    }

    /// Forks a branch with a writer, as that writer's store's commit: to a point if `point`. Its
    /// uploader is asked for the commit, as a fork waits for it to be uploaded.
    fn fork(&mut self, point: bool) {
        let Some((host, branch)) = self.ready() else { return };
        let (child, owner, tag) = (self.id(), self.id(), self.id());
        let session = &self.hosts[host].sessions[&branch];
        let (store, uploader, parent) =
            (session.store.clone().unwrap(), session.uploader.clone(), session.guest.head);
        let forked = Child { branch: sim_branch(child), owner, point };
        let before = match store.epoch() {
            Ok(before) => before,
            Err(err) => return self.store_failed(host, branch, err),
        };
        if let Err(err) = store.commit_fork(forked) {
            return self.store_failed(host, branch, err);
        }
        let Some(epoch) = self.committed(branch, &store, before) else { return };
        let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
        let extents = std::mem::take(&mut session.guest.written).into_iter().collect();
        let made = Truth { branch, parent, extents, frees: Vec::new(), root: None, capacity: None };
        self.made(host, branch, tag, epoch, made);
        self.branches.insert(child, Branch::forked(None, Some(tag), point, owner));
        self.forking.insert(child, (branch, forked));
        let _ = uploader.ask(epoch, false);
    }

    /// A writer takes a branch on a host: one with no writer, or, `takeover`, one with. Only a
    /// branch with a record: as a host opens the stores of the branches it was told of, a fork's
    /// child once its fork is made.
    fn open(&mut self, takeover: bool) {
        let direct = Direct(self.bucket.clone());
        let candidates: Vec<u64> = self
            .branches
            .iter()
            .filter(|(_, found)| !found.deleting && !found.point)
            .filter(|(branch, _)| {
                let open = self.hosts.iter().any(|host| host.sessions.contains_key(branch));
                open == takeover
            })
            .filter(|&(&branch, _)| {
                matches!(block_on(branch::record(&direct, sim_branch(branch))), Ok(Some(_)))
            })
            .map(|(&branch, _)| branch)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let branch = candidates[self.rng.random_range(0..candidates.len())];
        let elsewhere: Vec<usize> = (0..self.hosts.len())
            .filter(|&host| !self.hosts[host].sessions.contains_key(&branch))
            .collect();
        if elsewhere.is_empty() {
            return;
        }
        // Mostly where its store is.
        let with_disk: Vec<usize> = elsewhere
            .iter()
            .copied()
            .filter(|&host| self.hosts[host].disks.contains_key(&branch))
            .collect();
        let pool =
            if !with_disk.is_empty() && self.rng.random_bool(0.7) { with_disk } else { elsewhere };
        let host = pool[self.rng.random_range(0..pool.len())];
        if takeover && self.profile.hold > 0.0 && self.rng.random_bool(self.profile.hold) {
            let writers = self.hosts.iter().filter_map(|host| {
                host.sessions.get(&branch)?.uploader.writer().map(|writer| writer.epoch())
            });
            if let Some(epoch) = writers.max() {
                self.bucket.hold(sim_branch(branch), epoch);
            }
        }
        self.start_session(host, branch, None);
    }

    /// A host crashes: its uploaders stop where they are, what they had in flight landing or not;
    /// its disk keeps what its stores made durable, and some of what they did since, or is lost.
    fn crash(&mut self) {
        let host = self.rng.random_range(0..self.hosts.len());
        trace!("host {host} crashes");
        let sessions = std::mem::take(&mut self.hosts[host].sessions);
        for session in sessions.values() {
            session.running.abort();
        }
        if self.rng.random_bool(0.3) {
            self.hosts[host].disks.clear();
        } else {
            let disks = &mut self.hosts[host].disks;
            let mut keys: Vec<u64> = disks.keys().copied().collect();
            keys.sort_unstable();
            for branch in keys {
                let seed = self.rng.random();
                let disk = disks.get_mut(&branch).unwrap();
                disk.dir = disk.dir.crash(seed);
            }
        }
        let branches: Vec<u64> = sessions.keys().copied().collect();
        // Their stores close on what the crash left behind.
        for (branch, session) in sessions {
            self.stop(host, branch, session);
        }
        if self.profile.restarts {
            for branch in branches {
                let kept = self.hosts[host].disks.contains_key(&branch);
                if kept && !self.branches[&branch].deleting {
                    self.start_session(host, branch, None);
                }
            }
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

    /// The fork's child `child` as its making's guest knows it, wherever it is: a branch, or set
    /// aside while a restore or a root takes its ID; none once one of those took it.
    fn fork_child(&mut self, child: &Child) -> Option<&mut Branch> {
        let id = child.branch.id;
        let restore = self.restores.get_mut(&id).and_then(|restore| restore.taken.as_mut());
        let unfork = self.unforks.get_mut(&id).and_then(|unfork| unfork.taken.as_mut());
        [self.branches.get_mut(&id), restore, unfork, self.usurped.get_mut(&id)]
            .into_iter()
            .flatten()
            .find(|found| found.owner == child.owner)
    }

    /// `child`, a fork of a point that failed, or a crash left, goes as its host's cleanup deletes
    /// it, or an operator does what no host held: which nothing relies on.
    fn fail_restore(&mut self, child: u64, restore: Restore, base: Option<u64>) {
        let Restore { point, origin, owner, taken, .. } = restore;
        let mut found = Branch::forked(Some(origin), base, false, owner);
        found.deleting = true;
        self.branches.insert(child, found);
        let forked = Kind::Forking { origin, point: sim_branch(point) };
        self.unforks.insert(child, Unfork { forked, owner, running: None, taken });
        self.spawn_unfork(child);
    }

    /// The points with records, as a host took its copy of one: deleted since, perhaps, which the
    /// copy can't tell.
    fn points(&self) -> Vec<(u64, Origin)> {
        let direct = Direct(self.bucket.clone());
        (self.branches.iter())
            .filter(|(_, found)| found.point)
            .filter_map(|(&point, _)| {
                let record = block_on(branch::record(&direct, sim_branch(point))).ok()??;
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
        // Or, now and then, a child's ID a guest named that a fork's guest named too, its
        // segment not yet acknowledged: whichever's record comes first takes it (TASK-173).
        let (child, taken) = match self.pending_forks() {
            pending if !pending.is_empty() && self.rng.random_bool(0.5) => {
                let child = pending[self.rng.random_range(0..pending.len())];
                (child, self.branches.remove(&child))
            }
            _ => (child, None),
        };
        let host = self.rng.random_range(0..self.hosts.len());
        let restore = Restore { point, origin, host, owner, running: None, taken };
        self.restores.insert(child, restore);
        self.spawn_restore(child);
    }

    /// The children of forks at segments, made and not yet recorded, nor acknowledged, opened or
    /// deleted.
    fn pending_forks(&self) -> Vec<u64> {
        let direct = Direct(self.bucket.clone());
        let held = |branch| self.hosts.iter().any(|host| host.sessions.contains_key(branch));
        (self.branches.iter())
            .filter(|(_, found)| found.origin.is_some() && !found.point && !found.deleting)
            .filter(|(child, found)| found.acked.is_empty() && found.taken == 0 && !held(child))
            .filter(|&(&child, _)| {
                matches!(block_on(branch::record(&direct, sim_branch(child))), Ok(None))
            })
            .map(|(&child, _)| child)
            .collect()
    }

    /// Makes the fork `child`, or goes on with one a crash stopped, as opening its handle does.
    fn spawn_restore(&mut self, child: u64) {
        let Restore { point, origin, owner, .. } = self.restores[&child];
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        let running = self.spawn(async move {
            let result =
                branch::fork_point(&*bucket, sim_branch(child), sim_branch(point), origin, owner)
                    .await;
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
            let result = Writer::open(bucket, sim_branch(point), holder, holder).await.map(drop);
            events.borrow_mut().push(Event::PointTaken { point, result });
        });
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
                !self.branches.values().any(|child| forked(child.origin, sim_branch(branch)))
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
        self.unforks.insert(child, Unfork { forked, owner, running: None, taken: None });
        self.spawn_unfork(child);
    }

    /// Deletes what a crash left of the fork of a point to `child`.
    fn give_up_restore(&mut self, child: u64) {
        let restore = self.restores.remove(&child).unwrap();
        let base = self.branches[&restore.point].base;
        self.fail_restore(child, restore, base);
    }

    /// Deletes `child`, a failed fork's, as its host does.
    fn spawn_unfork(&mut self, child: u64) {
        let Unfork { forked, owner, .. } = self.unforks[&child];
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        let running = self.spawn(async move {
            let result =
                branch::delete_fork(&*bucket, sim_branch(child), forked, owner, holder, 0).await;
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
            let deleting =
                branch::delete_fork(&*bucket, sim_branch(child), Kind::Own, forks, holder, 0);
            let taking = async {
                branch::create(&*bucket, sim_branch(child), owner).await?;
                branch::take_lease(&*bucket, &layout::lease(sim_branch(child)), other, 0).await
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
                branch::create_forked(&*bucket, sim_branch(child), first),
                branch::create_forked(&*bucket, sim_branch(child), second)
            );
            events
                .borrow_mut()
                .push(Event::Twinned { child, taken: [(first, taken), (second, also)] });
        });
    }

    /// A root takes the ID of a fork's child whose segment is not yet acknowledged, as a guest may
    /// name any, and is deleted: the fork's parent's writer, slow to record the fork, finds the
    /// root's record, deleted (TASK-174).
    fn usurp(&mut self) {
        let pending = self.pending_forks();
        if pending.is_empty() {
            return;
        }
        let child = pending[self.rng.random_range(0..pending.len())];
        let fork = self.branches.remove(&child).unwrap();
        self.usurped.insert(child, fork);
        let (owner, holder) = (self.id(), self.id());
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let created = branch::create(&*bucket, sim_branch(child), owner).await;
            let deleted = match created {
                Ok(()) => Some(branch::delete(&*bucket, sim_branch(child), holder, 0).await),
                Err(_) => None,
            };
            events.borrow_mut().push(Event::Usurped { child, owner, created, deleted });
        });
    }

    /// Deletes the fork of `owner` to `child`, whose segment never went, as its host's cleanup does
    /// once its child found another fork's record: which must leave that be.
    fn untwin(&mut self, child: u64, owner: u64) {
        let holder = self.id();
        let (bucket, events) = (self.bucket.clone(), self.events.clone());
        self.spawn(async move {
            let result =
                branch::delete_fork(&*bucket, sim_branch(child), Kind::Own, owner, holder, 0).await;
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
                        live(child)
                            && child
                                .origin
                                .is_some_and(|origin| origin.parent == sim_branch(branch))
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
            let result = match branch::delete(&*bucket, sim_branch(branch), holder, token).await {
                // A fork whose segment never made it: no branch.
                Err(Error::NoBranch) => Ok(()),
                result => result,
            };
            events.borrow_mut().push(Event::Deleted { branch, stale, result });
        });
    }

    /// A sweep of the collector over every cell, or, as often as [`Profile::one_cell`] says, a pass
    /// over one branch's cell alone (TASK-395.4): passes over one cell and another overlap, as over
    /// one cell.
    fn collect(&mut self) {
        if self.collectors.len() >= 2 {
            return;
        }
        let id = self.id();
        let (bucket, events, options) =
            (self.bucket.clone(), self.events.clone(), self.profile.options);
        let branches: Vec<u64> = self.branches.keys().copied().collect();
        let one_cell = self.profile.one_cell;
        let cell = (!branches.is_empty() && one_cell > 0.0 && self.rng.random_bool(one_cell))
            .then(|| sim_branch(branches[self.rng.random_range(0..branches.len())]).cell);
        let handle = self.spawn(async move {
            let result = match cell {
                Some(cell) => collector::collect_cell(&*bucket, cell, id, &options).await,
                None => collector::collect(&*bucket, id, &options).await,
            };
            events.borrow_mut().push(Event::Collected { id, result });
        });
        self.collectors.insert(id, handle);
    }

    fn stop_collector(&mut self) {
        if let Some((&id, _)) = self.collectors.iter().next() {
            self.collectors.remove(&id).unwrap().abort();
        }
    }

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
            for (extent, bytes) in &truth.extents {
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

    /// Requests complete, without failing, until none is left, those held too.
    fn settle(&mut self) {
        self.bucket.release();
        loop {
            self.settle_tasks();
            if self.bucket.waiting() == 0 {
                return;
            }
            self.bucket.complete(0, Fault::None);
            trace!("{}", self.bucket.journal().last().unwrap());
        }
    }

    /// The live branches, by their records.
    fn live(&self) -> Vec<u64> {
        let direct = Direct(self.bucket.clone());
        self.branches
            .keys()
            .copied()
            .filter(|&id| {
                let record = block_on(branch::record(&direct, sim_branch(id)));
                matches!(record, Ok(Some(record)) if !record.deleted)
            })
            .collect()
    }

    /// Checks that every task is done once everything settled, every request done and nothing
    /// failing (TASK-149.32): an upload, an acknowledgement, a checkpoint, a fork, a deletion or a
    /// collection still waiting then waits for a wake nothing will give. Its run would only end,
    /// its check reading nothing of what it waited to do.
    fn check_woken(&mut self) {
        self.settle();
        let waiting: Vec<String> = (self.tasks.borrow().values())
            .map(|spawned| format!("{}:{}", spawned.file(), spawned.line()))
            .collect();
        if !waiting.is_empty() {
            self.violation(format!("tasks still wait, everything settled: spawned at {waiting:?}"));
        }
    }

    /// Checks that a sweep of the collector that ran alone, the bucket holding `before` as it
    /// started, reports what it did (TASK-44.1): the objects it deleted, and the moves it wrote.
    /// fxdax: and no more references gone than it retired, which it sends again for as long as a
    /// cell is dead, whether or not they are there (TASK-395.4).
    fn check_report(
        &mut self,
        before: &BTreeSet<String>,
        report: Option<Result<collector::Report, Error>>,
    ) {
        let after: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        let moves = |key: &&String| matches!(layout::kind(key), layout::Kind::Moves { .. });
        let reference = |key: &&String| matches!(layout::kind(key), layout::Kind::Reference { .. });
        let gone = || before.difference(&after);
        let retired = gone().filter(reference).count();
        let expected = |reported: &collector::Report| collector::Report {
            deleted: gone().filter(|key| !reference(key)).count(),
            compacted: after.difference(before).filter(moves).count(),
            retired: reported.retired.max(retired),
        };
        match report {
            Some(Ok(report)) if report == expected(&report) => {}
            Some(Ok(report)) => {
                let expected = expected(&report);
                self.violation(format!("collector reported {report:?}, having done {expected:?}"))
            }
            Some(Err(error)) => self.violation(format!("collector: {error}")),
            None => self.violation("a collector alone did not finish".to_owned()),
        }
    }

    /// What the bucket holds and what the live branches name, for a failure's message.
    pub fn describe(&self) -> String {
        let direct = Direct(self.bucket.clone());
        let mut text = String::new();
        for branch in self.live() {
            let origin = self.branches[&branch].origin;
            match block_on(recovery::recover(&direct, sim_branch(branch), origin)) {
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
            if let layout::Kind::Segment { .. } = layout::kind(&key)
                && let Ok(object) = block_on(extent_remote::bucket::Bucket::get(&direct, &key))
                && let Ok(footer) = layout::Segment::footer(&object.bytes)
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
        let mut stamps: Vec<_> = self.stamps.iter().collect();
        stamps.sort();
        for ((branch, writer), lineage) in stamps {
            text += &format!("writer {writer} of {branch:x} uploads history {lineage}\n");
        }
        let mut lineages: Vec<_> = self.lineages.iter().collect();
        lineages.sort();
        for (lineage, commits) in lineages {
            text += &format!("history {lineage}: commits by the store's epochs {commits:?}\n");
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

    /// Checks that the bucket holds only what live branches need, compacted as the collectors
    /// could (TASK-44.1): [`extent_remote::testing::uncollected`], its live branches by their records.
    fn check_collected(&mut self) {
        let direct = Direct(self.bucket.clone());
        let compact_below = self.profile.options.compact_below;
        for left in block_on(extent_remote::testing::uncollected(&direct, compact_below)) {
            self.violation(left);
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
        trace!("{}", self.bucket.journal().last().unwrap());
    }

    /// Spawns `task`, counted among the [`World::tasks`] until it is done or aborted, by where
    /// it was spawned.
    #[track_caller]
    fn spawn(&mut self, task: impl Future<Output = ()> + 'static) -> AbortHandle {
        let (task, handle) = abortable(task);
        self.next_task += 1;
        let number = self.next_task;
        self.tasks.borrow_mut().insert(number, std::panic::Location::caller());
        let running = Running { tasks: self.tasks.clone(), number };
        self.spawner
            .spawn_local(async move {
                let _running = running;
                let _ = task.await;
            })
            .unwrap();
        handle
    }

    /// The bytes of `extent`, `len` of them, as the write `tag` writes them: a pattern of both,
    /// which compresses, as a guest's data mostly does.
    fn content(tag: u64, extent: ExtentId, len: usize) -> Vec<u8> {
        let seed = tag.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ extent.sequence.rotate_left(17)
            ^ extent.branch;
        (0..len).map(|i| (seed.rotate_left(i as u32 % 64) >> 3) as u8 ^ i as u8).collect()
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn violation(&mut self, violation: String) {
        self.violations.push(violation);
    }

    // The truth.

    /// The commits the segments of the branch `recovered` is since its newest checkpoint make, in
    /// the order recovering it applied them: each applied segment found by the footer of the one
    /// after it, from the last.
    fn applied(&self, direct: &Direct, recovered: &Recovered) -> Result<Vec<u64>, String> {
        let branch = recovered.state.branch;
        let mut footers = self.footers.borrow_mut();
        let mut applied = Vec::new();
        let mut at = recovered.state.last_segment;
        while let Some((seq, epoch)) = at
            && recovered.checkpoint.is_none_or(|checkpoint| seq > checkpoint)
        {
            let (after, commits) = match footers.entry((branch.id, seq, epoch)) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unknown) => {
                    let key = layout::segment(branch, seq);
                    let Some(object) = block_on(extent_remote::bucket::get(direct, &key)) else {
                        break;
                    };
                    let Ok(footer) = layout::Segment::footer(&object.bytes) else { break };
                    if footer.epoch != epoch {
                        break;
                    }
                    unknown.insert((footer.after, footer.contents.commits))
                }
            };
            applied.push(commits.clone());
            at = *after;
        }
        applied.reverse();
        let tags = applied.concat().into_iter();
        tags.map(|tag| Ok(self.truth_of(branch.id, Some(tag))?.unwrap())).collect()
    }

    /// Checks that every live branch holds what its commits made, up to one of its own, past
    /// everything its guests were told was durable, and that the current writers, and their
    /// stores, read what their guests hold.
    pub fn check(&mut self) {
        let direct = Arc::new(Direct(self.bucket.clone()));
        let mut violations = Vec::new();
        let keys: BTreeSet<String> = self.bucket.keys().into_iter().collect();
        // Nothing writes a point: no segment of its, no checkpoint, no lease but its deletion's.
        for (&id, _) in self.branches.iter().filter(|(_, branch)| branch.point) {
            let written = |prefix: String| keys.iter().any(|key| key.starts_with(&prefix));
            if written(layout::segments(sim_branch(id)))
                || written(layout::checkpoints(sim_branch(id)))
            {
                violations.push(format!("point {id:x} was written"));
            }
            if let Some(object) =
                block_on(extent_remote::bucket::get(&*direct, &layout::lease(sim_branch(id))))
                && layout::Lease::decode(&object.bytes)
                    .is_ok_and(|lease| lease.epoch != layout::Lease::FINAL)
            {
                violations.push(format!("point {id:x} has a writer's lease"));
            }
        }
        for (&id, branch) in &self.branches {
            // A failed fork's, being deleted: no guest knows of it.
            if self.unforks.contains_key(&id) {
                continue;
            }
            let record = match block_on(branch::record(&*direct, sim_branch(id))) {
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
            // No placement took it back from a later one. fxdax: a placement takes the branch's cell,
            // and its lease with the cell's epoch, which no taking of the cell by a later
            // placement lowers (TASK-364).
            let cell = block_on(extent_remote::cell::read(&*direct, sim_branch(id).cell));
            let lease =
                block_on(extent_remote::branch::lease(&*direct, &layout::lease(sim_branch(id))));
            match (cell, lease) {
                (Ok(Some(cell)), Ok(Some(lease))) => {
                    if cell.token < branch.taken {
                        violations.push(format!(
                            "branch {id:x}'s cell is an earlier placement's: token {} after {}",
                            cell.token, branch.taken
                        ));
                    }
                    // A deletion's lease, final, is taken with the highest token there is.
                    if lease.epoch != layout::Lease::FINAL && lease.token > cell.epoch {
                        violations.push(format!(
                            "branch {id:x}'s lease was taken at epoch {} of its cell, of epoch {}",
                            lease.token, cell.epoch
                        ));
                    }
                }
                (Ok(_), Ok(None)) if branch.taken > 0 => {
                    violations.push(format!("branch {id:x}, taken, has no lease"));
                }
                (Ok(_), Ok(_)) => {}
                (Err(error), _) | (_, Err(error)) => {
                    violations.push(format!("branch {id:x}'s cell or lease: {error}"));
                }
            }
            let recovered =
                match block_on(recovery::recover(&*direct, sim_branch(id), record.origin())) {
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
                let moves = layout::moves(&location.key);
                if !keys.contains(&location.key) && !keys.contains(&moves) {
                    violations.push(format!("{extent:?} of {id:x}: {} is gone", location.key));
                }
            }
            let head = match self.truth_of(id, state.last_commit) {
                Ok(head) => head,
                Err(violation) => {
                    violations.push(violation);
                    continue;
                }
            };
            let own = self.own_path(id, head);
            let start = own.first().map_or(head, |first| self.commits[first].parent);
            if start != branch.base {
                violations.push(format!("branch {id:x} holds commit {head:?}, not of its history"));
                continue;
            }
            // Its segments since its newest checkpoint apply each commit after the one before, of
            // its history: each once, in order, none another writer's history put after it. A
            // store's segment names the last of the commits it holds.
            let mut before = recovered.checkpoint.is_none().then_some(branch.base);
            let applied = match self.applied(&direct, &recovered) {
                Ok(applied) => applied,
                Err(violation) => {
                    violations.push(violation);
                    continue;
                }
            };
            for commit in applied {
                let follows = |before: Option<u64>| {
                    let parent = self.commits[&commit].parent;
                    before.is_none_or(|before| self.precedes(before, parent))
                };
                if let Some(before) = before.filter(|&before| !follows(before)) {
                    violations.push(format!(
                        "branch {id:x} applies commit {commit} after {before:?}, not of its history"
                    ));
                    break;
                }
                before = Some(Some(commit));
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
                match block_on(extent_remote::read(&direct, *extent, location)) {
                    Ok(bytes) if bytes == extents[extent] => {}
                    Ok(_) => violations.push(format!("branch {id:x}: {extent:?} reads wrong")),
                    Err(error) => violations.push(format!("branch {id:x}: {error}")),
                }
            }
        }
        // The writers no later one fenced read what their guests hold, and their stores.
        for host in &self.hosts {
            for (&id, session) in &host.sessions {
                let (Some(writer), Some(store)) = (session.uploader.writer(), &session.store)
                else {
                    continue;
                };
                let lease = block_on(branch::lease_epoch(&*direct, &layout::lease(sim_branch(id))));
                if writer.stopped().is_some() || lease != Ok(writer.epoch()) {
                    continue;
                }
                if session.uploader.failed().is_some() {
                    continue;
                }
                for (extent, location) in &writer.state().extents {
                    // Not one its guest freed: the writer's state lags what landed, unacknowledged
                    // yet, whose free a collector may already have reclaimed (seed 4116 of
                    // `everything` at 25x, TASK-149.32).
                    let Some(bytes) = session.guest.held.get(extent) else { continue };
                    match block_on(extent_remote::read(&direct, *extent, location)) {
                        Ok(read) if read == *bytes => {}
                        Ok(_) => violations
                            .push(format!("branch {id:x}'s writer: {extent:?} reads wrong")),
                        Err(error) => violations.push(format!("branch {id:x}'s writer: {error}")),
                    }
                }
                // And its store, where it names them in the bucket (TASK-195).
                for (extent, bytes) in &session.guest.held {
                    let location = match store.place(*extent) {
                        Ok(Some(Place::Remote(location))) => location,
                        Ok(Some(_)) => continue,
                        Ok(None) => {
                            violations.push(format!("branch {id:x}'s store lost {extent:?}"));
                            continue;
                        }
                        Err(err) => {
                            violations.push(format!("branch {id:x}'s store: {err:#}"));
                            continue;
                        }
                    };
                    match block_on(extent_remote::read(&direct, *extent, &location)) {
                        Ok(read) if read == *bytes => {}
                        Ok(_) => violations
                            .push(format!("branch {id:x}'s store: {extent:?} reads wrong")),
                        Err(error) => violations.push(format!("branch {id:x}'s store: {error}")),
                    }
                }
            }
        }
        self.violations.extend(violations);
    }

    // The end.

    /// Runs a sweep of the collector over every cell, alone, past the grace period, and checks what
    /// it reports it did ([`World::check_report`]): what it reported, if it did.
    fn sweep_alone(&mut self) -> Option<collector::Report> {
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
        let reported = reported.take();
        let report = reported.clone().and_then(Result::ok);
        self.check_report(&before, reported);
        report
    }

    /// Stops the guests, lets every request and task finish, takes every live branch over with a
    /// new writer, then has each commit, upload and checkpoint while collectors run, a few times
    /// over.
    fn quiesce(&mut self) {
        trace!("quiescing");
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
                for (branch, session) in std::mem::take(&mut self.hosts[host].sessions) {
                    self.stop(host, branch, session);
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
                        .position(|host| host.disks.contains_key(&branch))
                        .unwrap_or(0);
                    self.start_session(host, branch, None);
                }
                self.settle();
                self.check_uploaded();
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
                let Some(store) = session.store.clone() else { continue };
                let parent = session.guest.head;
                let tag = self.id();
                let before = store.epoch().unwrap();
                if let Err(err) = store.commit(None) {
                    self.violation(format!("branch {branch:x}'s last store: {err:#}"));
                    return;
                }
                let Some(epoch) = self.committed(branch, &store, before) else { return };
                let session = self.hosts[host].sessions.get_mut(&branch).unwrap();
                let extents = std::mem::take(&mut session.guest.written).into_iter().collect();
                let made = Truth {
                    branch,
                    parent,
                    extents,
                    frees: Vec::new(),
                    root: None,
                    capacity: None,
                };
                self.made(host, branch, tag, epoch, made);
            }
            self.check_uploaded();
            for &(host, branch) in &sessions {
                let Some(session) = self.hosts[host].sessions.get(&branch) else { continue };
                if let Some(writer) = session.uploader.writer() {
                    self.spawn(async move {
                        let _ = writer.checkpoint().await;
                    });
                }
            }
            self.settle();
            self.sweep_alone();
        }
        // fxdax: then sweeps until two in a row delete and compact nothing: a dead cell's pass
        // retires the references its parents' cells keep fork points for, which their next passes
        // reclaim, a sweep a generation of a chain (TASK-395.4).
        let mut quiet = 0;
        for _ in 0..16 {
            let Some(report) = self.sweep_alone() else { break };
            quiet = if report.deleted == 0 && report.compacted == 0 { quiet + 1 } else { 0 };
            if quiet == 2 {
                break;
            }
        }
        if quiet < 2 {
            self.violation("16 sweeps of the collector alone, which kept doing something".into());
        }
        self.check_woken();
        self.check();
        self.check_collected();
    }
}
