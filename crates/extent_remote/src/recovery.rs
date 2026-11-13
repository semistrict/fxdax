// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's state, recovered from its objects: what a writer taking the branch starts from, and
//! what the collector keeps. Extents are read from where it says they are by fetchers
//! ([`crate::fetch`]).
//!
//! A branch's state is its newest checkpoint, or, with none, its parent's state where it was
//! forked, then the footers of its segments after it, in order, to the first one missing: read by
//! their objects' tails, which hold them, several at once. A segment that does not follow the last
//! one applied, which its footer names, is void and skipped: an earlier writer's, landed after a
//! later one's, or a writer's, landed where one of its before it did not. fxdax: a commit may span
//! segments, each naming its runs of it, which a reader holds until the segment ending it, and
//! drops at one neither going on with it nor ending it, a later writer's fence ([`Begun`],
//! TASK-92.41). A state recovered is
//! recovered on by the segments that came since ([`recover_on`]): a writer's, once it holds the
//! lease, from what it found before.
//!
//! The collector deletes what no reader starting now needs, segments only once a checkpoint covers
//! them. A reader that finds a segment missing checks that no checkpoint newer than the one it
//! started from has come: if one has, the segment may be one it covers, gone, and it starts again.

use crate::bucket::{self, Bucket, Created, Meta, Version};
use crate::layout::{self, Branch, Cell, Chain, Contents, Footer, Fork, Location, Origin, Segment};
use crate::record::Record;
use crate::{Error, object, rules};
use extent_frame::spent::Spent;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use storage_device::ExtentId;

/// What a branch holds as of a segment: where its extents are, `P`, and the rest of its state.
/// One replay makes it, [`State::apply`], whoever reads the branch's history: its recovery, so its
/// writers, its collector, its points and its deletion; a writer acknowledging its segments; and a
/// local store rebuilding it from the footers of its closed segments (TASK-165.3). Each places the
/// extents as it keeps them ([`Places`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State<P = Extents> {
    pub branch: Branch,
    pub extents: P,
    pub next_sequence: u64,
    pub root: Option<Vec<u8>>,
    /// The tag of the last commit, which may be the parent's, before the fork.
    pub last_commit: Option<u64>,
    /// The last segment applied, by its number and epoch, which the next follows.
    pub last_segment: Option<(u64, u64)>,
    pub forks: Vec<Fork>,
    /// The highest epoch of the segments applied.
    pub epoch: u64,
    /// fxdax: the branch's fences, in order: each segment applied of an epoch past those before
    /// it, a writer's taking over (doc-7, Fencing). Its own, not its parent's: a fork's child
    /// starts with none. With its strays, what the collector pins ([`pinned`], TASK-388).
    ///
    /// [`pinned`]: crate::collector::pinned
    pub fences: Vec<u64>,
    /// fxdax: the branch's strays, in order: every segment read that is neither a fence nor of the
    /// epoch of the fence before it ([`Stray`], TASK-383). Its own, as its fences are.
    pub strays: Vec<Stray>,
    /// The last segment applied.
    pub through: Option<u64>,
    /// The sequence numbers of the branch's own extents it freed, which it writes no more: one
    /// rule, every free of them a footer names (TASK-79.6, TASK-150.6, TASK-165.3).
    pub spent: Spent,
    /// The most bytes its extents may hold, as its last commit to set it set it, if one did: its
    /// own, or, before its first, its parent's at the fork (TASK-165.3).
    pub capacity: Option<u64>,
    /// fxdax: the runs of the commit the segments applied last went on with, which a later segment
    /// ends, if they did ([`Begun`], TASK-92.41): none in a state a writer acknowledged, nor in
    /// any checkpoint, as writers acknowledge only segments ending their commits.
    pub begun: Option<Begun>,
}

/// fxdax: a segment a recovery read that is neither a fence nor its fence's writer's (TASK-383): a
/// writer taken over may still have a create in flight at its place, its successor having fenced
/// past it, which fails only while the stray is there ([`pinned`]).
///
/// [`pinned`]: crate::collector::pinned
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stray {
    pub seq: u64,
    /// Whether it follows the last segment applied, of an earlier epoch than it, as an earlier
    /// writer's fence made after its successor's does; else it is void, following none, as an
    /// earlier writer's segment landed late is.
    pub late: bool,
}

/// fxdax: the runs a commit sealed in segments applied that a later segment ends
/// (`extent_frame::segment::Continued`, TASK-92.41): sealed with it, where they are, once it does;
/// or never, if the next segment applied neither goes on with it nor ends it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Begun {
    pub commit: u64,
    pub runs: Vec<(ExtentId, Location)>,
}

/// Where a branch's extents are in the bucket.
pub type Extents = imbl::OrdMap<ExtentId, Location>;

/// Where a branch's extents are, as a replay of its segments places them ([`State::apply`]).
pub trait Places {
    /// Seals `extent` where `seal` says, in the segment applied.
    fn seal(&mut self, extent: ExtentId, seal: Seal);

    /// Frees `extent`.
    fn free(&mut self, extent: ExtentId);
}

/// Where a replay seals an extent, in the bucket: in a run of its segment's data, or in an object
/// of its own, which its writer uploaded before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seal {
    Run(Location),
    Own(Location),
}

impl Places for Extents {
    fn seal(&mut self, extent: ExtentId, (Seal::Run(location) | Seal::Own(location)): Seal) {
        self.insert(extent, location);
    }

    fn free(&mut self, extent: ExtentId) {
        self.remove(&extent);
    }
}

impl State {
    pub fn new(branch: Branch) -> Self {
        Self::of(branch, Extents::new())
    }
}

impl<P> State<P> {
    /// The state of `branch` before its first segment, its extents placed in `extents`.
    pub fn of(branch: Branch, extents: P) -> Self {
        Self {
            branch,
            extents,
            next_sequence: 0,
            root: None,
            last_commit: None,
            last_segment: None,
            forks: Vec::new(),
            epoch: 0,
            fences: Vec::new(),
            strays: Vec::new(),
            through: None,
            spent: Spent::default(),
            capacity: None,
            begun: None,
        }
    }

    /// The state of `child`, forked from this one, before its first segment, its extents placed
    /// in `extents`: what it inherits but its extents, its root, its last commit and its capacity.
    fn inheriting<Q>(&self, child: Branch, extents: Q) -> State<Q> {
        State {
            root: self.root.clone(),
            last_commit: self.last_commit,
            capacity: self.capacity,
            ..State::of(child, extents)
        }
    }

    /// The state, its extents placed in what `place` makes of them.
    pub fn placed<Q>(self, place: impl FnOnce(P) -> Q) -> State<Q> {
        let Self {
            branch,
            extents,
            next_sequence,
            root,
            last_commit,
            last_segment,
            forks,
            epoch,
            fences,
            strays,
            through,
            spent,
            capacity,
            begun,
        } = self;
        let extents = place(extents);
        State {
            branch,
            extents,
            next_sequence,
            root,
            last_commit,
            last_segment,
            forks,
            epoch,
            fences,
            strays,
            through,
            spent,
            capacity,
            begun,
        }
    }
}

impl<P: Places> State<P> {
    /// Applies `footer`'s segment, unless it is void: one not following the last applied, which
    /// its footer names (doc-7, Fencing). Returns whether it applied it.
    pub fn apply(&mut self, footer: &Footer) -> bool {
        self.through = Some(footer.seq);
        let void = footer.after != self.last_segment && rules().continuity;
        if void || footer.epoch < self.epoch {
            self.strays.push(Stray { seq: footer.seq, late: !void });
        }
        if void {
            return false;
        }
        self.last_segment = Some((footer.seq, footer.epoch));
        if footer.epoch > self.epoch {
            self.fences.push(footer.seq);
        }
        self.epoch = self.epoch.max(footer.epoch);
        self.apply_contents(footer.seq, footer.epoch, &footer.contents);
        true
    }

    /// Applies `contents`, those of the branch's segment `seq`, of the writer of lease epoch
    /// `epoch`: what its commits sealed, where they sealed it, then what they freed; the root they
    /// set, their last commit, the branch's next sequence number, the largest named, the forks at
    /// its end, and the capacity they set. Every free of the branch's own extents spends it, as the store that freed it
    /// spent it: a footer frees only what a commit held, not what was freed while open
    /// (TASK-160.15), and one sealed and freed within the segment is a free and no seal (TASK-165.3).
    /// fxdax: first, the runs of a commit the segments before began ([`Begun`]), if this one ends
    /// it, or holding them on if it goes on with it, or dropping them; then, after its own, holding
    /// those of the commit it goes on with, if any (TASK-92.41).
    pub fn apply_contents(&mut self, seq: u64, epoch: u64, contents: &Contents) {
        let key = layout::segment(self.branch, seq);
        match self.begun.take() {
            Some(begun) if contents.ends(begun.commit) => {
                for (extent, location) in begun.runs {
                    self.extents.seal(extent, Seal::Run(location));
                }
            }
            Some(begun) if contents.continues(begun.commit) => self.begun = Some(begun),
            // Never ended: its writer's, never acknowledged, a later writer fenced.
            Some(_) | None => {}
        }
        for &(extent, offset, len) in &contents.extents {
            self.extents.seal(extent, Seal::Run(Location { key: key.clone(), offset, len }));
        }
        for (extent, location) in contents.own_at(self.branch, epoch, seq) {
            self.extents.seal(extent, Seal::Own(location));
        }
        for &extent in &contents.frees {
            self.extents.free(extent);
            if extent.branch == self.branch.id {
                self.spent.insert(extent.sequence);
            }
        }
        if let Some(root) = &contents.root {
            self.root = Some(root.clone());
        }
        if let Some(&commit) = contents.commits.last() {
            self.last_commit = Some(commit);
        }
        self.next_sequence = self.next_sequence.max(contents.next_sequence);
        self.forks.extend(contents.forks.iter().map(|child| Fork::at(seq, child)));
        if contents.capacity.is_some() {
            self.capacity = contents.capacity;
        }
        if let Some(continued) = &contents.continued {
            let runs = continued
                .extents
                .iter()
                .map(|&(extent, offset, len)| (extent, Location { key: key.clone(), offset, len }));
            match &mut self.begun {
                Some(begun) => begun.runs.extend(runs),
                None => {
                    let runs = runs.collect();
                    self.begun = Some(Begun { commit: continued.commit, runs });
                }
            }
        }
    }
}

/// A branch's state, and what reading it took: its [`State`], or its [`Layers`].
#[derive(Clone, Debug)]
pub struct Recovered<S = State> {
    pub state: S,
    /// The first segment missing: where the next writer's go.
    pub end: u64,
    /// The newest checkpoint, if it started from one.
    pub checkpoint: Option<u64>,
    /// The lease epoch of the writer of that checkpoint: the highest of the segments it covers.
    /// The collector goes by it, to tell whether the branch's writer may name its extents where
    /// they were before the moves it followed (TASK-175).
    pub checkpointer: Option<u64>,
    /// The keys it read, which the collector keeps.
    pub touched: BTreeSet<String>,
}

impl Recovered<Layers> {
    /// The branch's state, its layers in one.
    pub(crate) fn flatten(self) -> Recovered {
        let Self { state, end, checkpoint, checkpointer, touched } = self;
        let state = state.placed(Layered::flatten);
        Recovered { state, end, checkpoint, checkpointer, touched }
    }
}

/// A branch's state in layers ([`Layered`]).
pub(crate) type Layers = State<Layered>;

/// Where a branch's extents are, in layers: what it inherited, if it has no checkpoint of its own,
/// its parent's state where it was forked, shared, as read, with the other forks of the parent
/// there; over it, what its segments made of it since. The collector, recovering many forks of a
/// base, takes what they inherited together once, not each fork's copy (TASK-92.7).
#[derive(Clone, Debug)]
pub(crate) struct Layered {
    pub inherited: Option<Arc<State>>,
    /// Where its extents are but those it inherited and holds still: all of them, with nothing
    /// inherited.
    pub own: Extents,
    /// The extents it inherited that it holds no longer where it inherited them: freed, or sealed
    /// again.
    pub shadowed: BTreeSet<ExtentId>,
}

impl Layered {
    /// `state`, in one layer.
    fn of(state: State) -> Layers {
        state.placed(|own| Self { inherited: None, own, shadowed: BTreeSet::new() })
    }

    /// The state of `child`, forked from `inherited`, before its segments.
    fn inheriting(inherited: Arc<State>, child: Branch) -> Layers {
        let extents = Self { inherited: None, own: Extents::new(), shadowed: BTreeSet::new() };
        let mut state = inherited.inheriting(child, extents);
        state.extents.inherited = Some(inherited);
        state
    }

    /// Shadows `extent` if it inherited it.
    fn shadow(&mut self, extent: ExtentId) {
        let inherited = self.inherited.as_ref();
        if inherited.is_some_and(|inherited| inherited.extents.contains_key(&extent)) {
            self.shadowed.insert(extent);
        }
    }

    /// Where its extents are, in one: what it inherited and holds still, and its own. What it
    /// inherited is shared, not copied: the map is persistent, so this costs what its own layer
    /// holds, not what it inherited (TASK-92.55).
    fn flatten(self) -> Extents {
        let Self { inherited, own, shadowed } = self;
        let Some(inherited) = inherited else { return own };
        let mut extents = inherited.extents.clone();
        for extent in &shadowed {
            extents.remove(extent);
        }
        own.union(extents)
    }
}

impl Places for Layered {
    fn seal(&mut self, extent: ExtentId, seal: Seal) {
        self.shadow(extent);
        self.own.seal(extent, seal);
    }

    fn free(&mut self, extent: ExtentId) {
        self.shadow(extent);
        self.own.free(extent);
    }
}

/// How often recovery starts again before it gives up on a branch as lost.
const RESTARTS: usize = 64;

/// How many segments' footers recovery reads at once, at most: one, then twice as many each time
/// all it read were there, so that it reads few past a branch's end, and many at once of a long
/// branch.
const WINDOW: u64 = 16;

enum Attempt {
    Restart,
    Failed(Error),
}

impl From<Error> for Attempt {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

/// The state of `branch`, forked from `origin`.
pub async fn recover(
    bucket: &impl Bucket,
    branch: Branch,
    origin: Option<Origin>,
) -> Result<Recovered, Error> {
    Ok(recover_layers(&Checkpoints::new(bucket, 0), branch, origin).await?.flatten())
}

/// The state of `branch`, forked from `origin`, as [`recover`] finds it, in layers: reading the
/// checkpoint it starts from through `checkpoints`, which other recoveries share.
pub(crate) async fn recover_layers<B: Bucket>(
    checkpoints: &Checkpoints<'_, B>,
    branch: Branch,
    origin: Option<Origin>,
) -> Result<Recovered<Layers>, Error> {
    for _ in 0..RESTARTS {
        match attempt(checkpoints, branch, origin).await {
            Ok(recovered) => return Ok(recovered),
            Err(Attempt::Restart) => {}
            Err(Attempt::Failed(error)) => return Err(error),
        }
    }
    Err(Error::Lost(format!("branch {branch}: what its recovery starts from keeps going")))
}

/// fxdax: the state of `branch` as of the end of its segment `at`, which is acknowledged: a fork
/// point's, as a fork made there inherits it; and what reading it read, which the collector keeps
/// for the forks referring to it (doc-7, A pass over a cell; TASK-395.4). Its newest checkpoint up
/// to there, or its origin's state, then its segments to there, starting again as [`recover`]
/// does where it finds one gone that a newer checkpoint covers: [`Error::Lost`] once it keeps
/// going, or what it needs is gone.
pub(crate) async fn recover_at<B: Bucket>(
    checkpoints: &Checkpoints<'_, B>,
    branch: Branch,
    at: u64,
) -> Result<(Arc<State>, BTreeSet<String>), Error> {
    for _ in 0..RESTARTS {
        let mut touched = BTreeSet::new();
        match state_at(checkpoints, branch, at, &mut touched, branch.cell).await {
            Ok(state) => return Ok((state, touched)),
            Err(Attempt::Restart) => {}
            Err(Attempt::Failed(error)) => return Err(error),
        }
    }
    Err(Error::Lost(format!(
        "branch {branch} at {at:x}: what its recovery starts from keeps going"
    )))
}

/// Checkpoints read whole, for recoveries, each read once for those that start from it at once,
/// and for those after while it is among the last few read: a base's recovery and its forks',
/// those without checkpoints of their own starting from the base's, share one, as a pass of the
/// collector's recovering them all does (TASK-92.7).
pub(crate) struct Checkpoints<'a, B: Bucket> {
    bucket: &'a B,
    /// How many of those read it keeps, the last read.
    keep: usize,
    read: Mutex<Read<'a>>,
}

/// The reads of checkpoints [`Checkpoints`] keeps, or that are in flight, by their keys; and the
/// keys in the order they were read.
#[derive(Default)]
struct Read<'a> {
    flights: HashMap<String, Flight<'a>>,
    order: VecDeque<String>,
}

/// A read of a checkpoint, or what it found: the state it holds, if it is there.
type Flight<'a> = Shared<BoxFuture<'a, Result<Option<Arc<State>>, Error>>>;

impl<'a, B: Bucket> Checkpoints<'a, B> {
    /// Reads checkpoints from `bucket`, keeping the last `keep` read.
    pub(crate) fn new(bucket: &'a B, keep: usize) -> Self {
        Self { bucket, keep, read: Mutex::default() }
    }

    /// The state the checkpoint `listed` of `branch` holds, if it is there: read now, or by the
    /// read of it in flight or kept, or as the bucket decoded it before ([`Bucket::decoded`]). One
    /// gone, or that failed, is read anew the next time.
    async fn read(&self, listed: &Meta, branch: Branch) -> Result<Option<Arc<State>>, Error> {
        self.read_object(&listed.key, Some(listed.clone()), branch).await
    }

    /// fxdax: the state the checkpoint at `key` of `branch` holds, if it is there, read by its key,
    /// listed nowhere ([`state_at`] of another cell's branch, TASK-395.4): as [`Checkpoints::read`]
    /// does, but for the bucket's decoded states, which go by a listing's version.
    async fn read_key(&self, key: &str, branch: Branch) -> Result<Option<Arc<State>>, Error> {
        self.read_object(key, None, branch).await
    }

    /// The state the checkpoint at `key` of `branch` holds, as [`Checkpoints::read`] says, the
    /// object as `listed`, if it was.
    async fn read_object(
        &self,
        key: &str,
        listed: Option<Meta>,
        branch: Branch,
    ) -> Result<Option<Arc<State>>, Error> {
        let flight = {
            let mut read = self.read.lock().unwrap();
            match read.flights.get(key) {
                Some(flight) => flight.clone(),
                None => {
                    let bucket = self.bucket;
                    let owned = key.to_owned();
                    let flight = async move {
                        let key = owned;
                        let decoded = bucket.decoded();
                        let known = listed.as_ref().zip(decoded);
                        if let Some(state) = known.and_then(|(listed, decoded)| decoded.get(listed))
                        {
                            return Ok(Some(state));
                        }
                        let Some(object) = bucket::get(bucket, &key).await else {
                            return Ok(None);
                        };
                        let state = Arc::new(checkpoint(&key, &object.bytes, branch)?);
                        if let Some(decoded) = decoded {
                            decoded.keep(&object.meta, &state);
                        }
                        Ok(Some(state))
                    };
                    let flight = flight.boxed().shared();
                    if self.keep > 0 {
                        read.flights.insert(key.to_owned(), flight.clone());
                        read.order.push_back(key.to_owned());
                        while read.order.len() > self.keep {
                            let oldest = read.order.pop_front().unwrap();
                            read.flights.remove(&oldest);
                        }
                    }
                    flight
                }
            }
        };
        let state = flight.await;
        if !matches!(state, Ok(Some(_))) {
            let mut read = self.read.lock().unwrap();
            read.flights.remove(key);
            read.order.retain(|kept| kept != key);
        }
        state
    }
}

/// fxdax: the states checkpoints hold, decoded, which the recoveries reading through a bucket
/// share ([`Bucket::decoded`]), by the objects they were decoded from: a checkpoint's key with its
/// version, so that an object written at the key again is not taken for the one before, and none
/// of an object the bucket names no version of. A state's extents are a persistent map, which a
/// recovery starting from it takes for nothing; the last [`Decoded::KEEP`] decoded are kept: those
/// the forks being opened start from (TASK-92.55).
#[derive(Default)]
pub struct Decoded {
    kept: Mutex<Kept>,
}

/// The states [`Decoded`] keeps, by their objects, and those, oldest first.
#[derive(Default)]
struct Kept {
    states: HashMap<(String, Version), Arc<State>>,
    order: VecDeque<(String, Version)>,
}

impl Decoded {
    /// How many states it keeps: a host's forks open from a few bases' newest checkpoints at once.
    const KEEP: usize = 16;

    /// The state the object `meta` names holds, if it kept it.
    fn get(&self, meta: &Meta) -> Option<Arc<State>> {
        let kept = self.kept.lock().unwrap();
        kept.states.get(&(meta.key.clone(), meta.version.clone())).cloned()
    }

    /// Keeps `state`, which the object `meta` names holds, if the bucket names its version.
    fn keep(&self, meta: &Meta, state: &Arc<State>) {
        if meta.version == Version::default() {
            return;
        }
        let object = (meta.key.clone(), meta.version.clone());
        let mut kept = self.kept.lock().unwrap();
        if kept.states.insert(object.clone(), state.clone()).is_none() {
            kept.order.push_back(object);
        }
        while kept.order.len() > Self::KEEP {
            let oldest = kept.order.pop_front().unwrap();
            kept.states.remove(&oldest);
        }
    }
}

/// The state of the branch `before` recovered, forked from `origin`, recovered on from it: its
/// segments from `before`'s end, those that came since, applied to its state, rather than its
/// state read anew. As [`recover`] would find, unless a checkpoint newer than `before`'s has come,
/// which may cover segments gone since: then it recovers anew.
pub async fn recover_on(
    bucket: &impl Bucket,
    origin: Option<Origin>,
    before: Recovered,
) -> Result<Recovered, Error> {
    let Recovered { state, end, checkpoint, checkpointer, touched } = before;
    let branch = state.branch;
    let before = Recovered { state: Layered::of(state), end, checkpoint, checkpointer, touched };
    match follow(bucket, before).await {
        Ok(recovered) => Ok(recovered.flatten()),
        Err(Attempt::Restart) => recover(bucket, branch, origin).await,
        Err(Attempt::Failed(error)) => Err(error),
    }
}

async fn attempt<B: Bucket>(
    checkpoints: &Checkpoints<'_, B>,
    branch: Branch,
    origin: Option<Origin>,
) -> Result<Recovered<Layers>, Attempt> {
    let bucket = checkpoints.bucket;
    let mut touched = BTreeSet::new();
    let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
    let (state, checkpoint, checkpointer) = match listed.last() {
        Some(newest) => {
            let Some(state) = checkpoints.read(newest, branch).await? else {
                return Err(Attempt::Restart);
            };
            touched.insert(newest.key.clone());
            let seq = state.through.ok_or_else(|| Error::Corrupt(newest.key.clone()))?;
            let epoch = state.epoch;
            (Layered::of(Arc::unwrap_or_clone(state)), Some(seq), Some(epoch))
        }
        None => match origin {
            None => (Layered::of(State::new(branch)), None, None),
            Some(origin) => {
                let at = match state_at(
                    checkpoints,
                    origin.parent,
                    origin.at,
                    &mut touched,
                    branch.cell,
                )
                .await
                {
                    Ok(at) => at,
                    Err(failed) => {
                        return Err(unless_checkpointed(bucket, branch, u64::MAX, failed).await);
                    }
                };
                (Layered::inheriting(at, branch), None, None)
            }
        },
    };
    let end = checkpoint.map_or(0, |seq| seq + 1);
    follow(bucket, Recovered { state, end, checkpoint, checkpointer, touched }).await
}

/// `recovered`, its segments from its end on applied, to the first missing: the branch's end,
/// unless a checkpoint newer than the one it started from has come, which may cover the segment,
/// gone; then it starts again.
async fn follow(
    bucket: &impl Bucket,
    mut recovered: Recovered<Layers>,
) -> Result<Recovered<Layers>, Attempt> {
    let Recovered { state, end, touched, .. } = &mut recovered;
    let branch = state.branch;
    *end = footers(bucket, branch, *end, u64::MAX, |key, footer| {
        state.apply(&footer);
        touched.insert(key);
    })
    .await?;
    if rules().newest_checkpoint {
        // The end, unless a newer checkpoint covers the segment, which the collector then removed.
        let newest = bucket::list(bucket, &layout::checkpoints(branch)).await.last().map(|meta| {
            match layout::kind(&meta.key) {
                layout::Kind::Checkpoint { seq, .. } => seq,
                _ => unreachable!(),
            }
        });
        if newest != recovered.checkpoint {
            return Err(Attempt::Restart);
        }
    }
    Ok(recovered)
}

/// Gives `each` the footers of the segments of `branch` from `seq` on, with their keys, in order,
/// up to the first missing and to `last` at most: read by their objects' tails, a [`WINDOW`] of
/// them at most at once. Returns the first it did not give: the first missing, or the one after
/// `last`. What it read past the first missing it leaves, as a recovery reading them one at a time
/// would never read it.
async fn footers(
    bucket: &impl Bucket,
    branch: Branch,
    mut seq: u64,
    last: u64,
    mut each: impl FnMut(String, Footer),
) -> Result<u64, Error> {
    let mut window = 1;
    while seq <= last {
        let reads = (seq..seq.saturating_add(window).min(last.saturating_add(1)))
            .map(|seq| segment_footer(bucket, branch, seq));
        for read in futures::future::join_all(reads).await {
            let Some(footer) = read? else { return Ok(seq) };
            each(layout::segment(branch, seq), footer);
            seq += 1;
        }
        window = (window * 2).min(WINDOW);
    }
    Ok(seq)
}

/// The footer of segment `seq` of `branch`, if it is there: read by its object's tail, a ranged
/// read of its last bytes, which hold it ([`object::tail`]), not by the segment whole.
async fn segment_footer(
    bucket: &impl Bucket,
    branch: Branch,
    seq: u64,
) -> Result<Option<Footer>, Error> {
    let key = layout::segment(branch, seq);
    let Some(tail) = object::tail(bucket, &key).await? else { return Ok(None) };
    Ok(Some(placed(&key, layout::decode(tail.footer())?, branch, seq)?))
}

/// Puts an empty segment of the writer of lease epoch `epoch` at `seq`, the place after what
/// `state` applied, and applies it; or, if a segment landed there first, applies that, returning its
/// footer. A writer taking the branch over fences it so, at the end it recovered to and past the
/// gaps after it; and so does a deletion, of the final epoch, at the end it recovered to (doc-7,
/// Fencing; TASK-183).
pub(crate) async fn fence(
    bucket: &impl Bucket,
    state: &mut State,
    epoch: u64,
    seq: u64,
) -> Result<Option<Footer>, Error> {
    let branch = state.branch;
    let key = layout::segment(branch, seq);
    let mut chain = Chain::new(branch, epoch, seq, state.last_segment, state.next_sequence);
    let empty = Segment { data: Vec::new(), footer: chain.close(Contents::default()) };
    let found = match bucket::create(bucket, &key, empty.encode()).await {
        Created::Created => None,
        Created::Exists(object) => Some(footer(&key, &object.bytes, branch, seq)?),
    };
    state.apply(found.as_ref().unwrap_or(&empty.footer));
    Ok(found)
}

/// The footer of segment `seq` of `branch`, whose object holds `bytes`.
pub(crate) fn footer(key: &str, bytes: &[u8], branch: Branch, seq: u64) -> Result<Footer, Error> {
    placed(key, Segment::footer(bytes)?, branch, seq)
}

/// `footer`, that of the object at `key`, which holds segment `seq` of `branch`, if it says so.
fn placed(key: &str, footer: Footer, branch: Branch, seq: u64) -> Result<Footer, Error> {
    if footer.branch != branch || footer.seq != seq {
        return Err(Error::Corrupt(format!(
            "{key} holds segment {:x} of {}",
            footer.seq, footer.branch
        )));
    }
    Ok(footer)
}

/// The state checkpoint `key` of `branch`, whose object holds `bytes`, holds.
fn checkpoint(key: &str, bytes: &[u8], branch: Branch) -> Result<State, Error> {
    let state = crate::checkpoint::decode(bytes)?;
    if state.branch != branch {
        return Err(Error::Corrupt(format!("{key} holds a checkpoint of {}", state.branch)));
    }
    Ok(state)
}

/// The state of `branch` as of the end of its segment `at`, which is acknowledged: its newest
/// checkpoint up to there, or its origin's state, then its segments up to there. A checkpoint at
/// `at` itself is the state, as read, which the forks made there share.
///
/// fxdax: for a fork in `from`, a cell of its own, of `branch` in another, its checkpoint at `at`
/// alone, read by its key, listing nothing of the other cell's (TASK-395.4): whoever records a fork
/// across cells writes it first ([`checkpoint_at`]), and the parent's cell keeps it,
/// the fork point's state, while the fork's reference stands. Gone, what the fork inherited is.
fn state_at<'a, B: Bucket>(
    checkpoints: &'a Checkpoints<'_, B>,
    branch: Branch,
    at: u64,
    touched: &'a mut BTreeSet<String>,
    from: Cell,
) -> BoxFuture<'a, Result<Arc<State>, Attempt>> {
    Box::pin(async move {
        let bucket = checkpoints.bucket;
        if branch.cell != from {
            let key = layout::checkpoint(branch, at);
            let Some(state) = checkpoints.read_key(&key, branch).await? else {
                return Err(Attempt::Failed(Error::Lost(key)));
            };
            if state.through != Some(at) {
                return Err(Attempt::Failed(Error::Corrupt(format!("{key} holds another state"))));
            }
            touched.insert(key);
            return Ok(state);
        }
        let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
        let newest = listed.iter().rev().find(|meta| {
            matches!(layout::kind(&meta.key), layout::Kind::Checkpoint { seq, .. } if seq <= at)
        });
        let mut state = match newest {
            Some(meta) => {
                let Some(state) = checkpoints.read(meta, branch).await? else {
                    return Err(Attempt::Restart);
                };
                touched.insert(meta.key.clone());
                if state.through == Some(at) {
                    return Ok(state);
                }
                Layered::of(Arc::unwrap_or_clone(state))
            }
            None => {
                let key = layout::record(branch);
                let inherited = match bucket::get(bucket, &key).await {
                    None => Err(Attempt::Failed(Error::Lost(key))),
                    Some(object) => {
                        touched.insert(key);
                        match Record::decode(branch, &object.bytes)?.origin() {
                            None => Ok(None),
                            Some(origin) => {
                                let from = branch.cell;
                                state_at(checkpoints, origin.parent, origin.at, touched, from)
                                    .await
                                    .map(Some)
                            }
                        }
                    }
                };
                match inherited {
                    Ok(None) => Layered::of(State::new(branch)),
                    Ok(Some(inherited)) => Layered::inheriting(inherited, branch),
                    Err(failed) => {
                        return Err(unless_checkpointed(bucket, branch, at, failed).await);
                    }
                }
            }
        };
        let from = state.through.map_or(0, |seq| seq + 1);
        let applied = footers(bucket, branch, from, at, |key, footer| {
            state.apply(&footer);
            touched.insert(key);
        });
        // Acknowledged segments go only once a checkpoint covers them.
        if applied.await? <= at {
            return Err(Attempt::Restart);
        }
        Ok(Arc::new(state.placed(Layered::flatten)))
    })
}

/// fxdax: makes sure `branch` has a checkpoint as of its segment `at`, which is acknowledged: the
/// state a fork made there to another cell inherits, which the fork's recovery reads by its key
/// alone, so that a pass over the fork's cell lists nothing of `branch`'s ([`state_at`],
/// TASK-395.4). Written, create-only, by whoever records such a fork, before its reference and
/// its record ([`crate::branch::record_fork`]), from the newest checkpoint up to there and the
/// segments after: the state any reader finds there, its extents named where they were sealed,
/// never further along their moves than a reader names them. One there stands, any writer's.
pub(crate) async fn checkpoint_at(
    bucket: &impl Bucket,
    branch: Branch,
    at: u64,
) -> Result<(), Error> {
    let key = layout::checkpoint(branch, at);
    let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
    if listed.iter().any(|meta| meta.key == key) {
        return Ok(());
    }
    let checkpoints = Checkpoints::new(bucket, 0);
    for _ in 0..RESTARTS {
        match state_at(&checkpoints, branch, at, &mut BTreeSet::new(), branch.cell).await {
            Ok(state) => {
                bucket::create(bucket, &key, crate::checkpoint::encode(&*state)).await;
                return Ok(());
            }
            Err(Attempt::Restart) => {}
            Err(Attempt::Failed(error)) => return Err(error),
        }
    }
    Err(Error::Lost(format!(
        "branch {branch} at {at:x}: what its recovery starts from keeps going"
    )))
}

/// What `failed` gives, a recovery of `branch` that found no checkpoint of it up to its segment
/// `at` and went to what it inherited instead: one that found that lost starts again if a
/// checkpoint of the branch up to there has come since, which the collector, going by it, may have
/// let what it found lost go for: the record of a parent deleted, which no other recovery reads
/// (doc-7, Checkpoints; TASK-177).
async fn unless_checkpointed(
    bucket: &impl Bucket,
    branch: Branch,
    at: u64,
    failed: Attempt,
) -> Attempt {
    if !matches!(failed, Attempt::Failed(Error::Lost(_))) || !rules().newest_checkpoint {
        return failed;
    }
    let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
    let checkpointed = listed.iter().any(
        |meta| matches!(layout::kind(&meta.key), layout::Kind::Checkpoint { seq, .. } if seq <= at),
    );
    if checkpointed { Attempt::Restart } else { failed }
}

/// Whether `branch` holds its segment `at`, as a fork's recovery from there needs: the state as of
/// it reads whole, none of the segments up to it missing. Asked once: what is going, as a pass of
/// the collector takes it, reads as not held.
pub(crate) async fn holds(bucket: &impl Bucket, branch: Branch, at: u64) -> Result<bool, Error> {
    let checkpoints = Checkpoints::new(bucket, 0);
    match state_at(&checkpoints, branch, at, &mut BTreeSet::new(), branch.cell).await {
        Ok(_) => Ok(true),
        Err(Attempt::Restart | Attempt::Failed(Error::Lost(_))) => Ok(false),
        Err(Attempt::Failed(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::branch;
    use crate::layout::test_branch;
    use crate::layout::{Child, Continued, child};
    use crate::store::Store;
    use crate::testing::SimBucket;
    use crate::writer::{Objects, Writer};
    use object_store::memory::InMemory;
    use std::sync::Arc;

    /// A segment's object holds the segment its key names: of its branch, of its number.
    #[test]
    fn segments_are_where_their_keys_say() {
        let footer = Footer {
            branch: test_branch(1),
            seq: 2,
            epoch: 1,
            after: None,
            contents: Contents::default(),
        };
        let bytes = Segment { data: Vec::new(), footer: footer.clone() }.encode();
        assert_eq!(super::footer("here", &bytes, test_branch(1), 2), Ok(footer));
        for (branch, seq) in [(1, 3), (4, 2)] {
            let corrupt = Error::Corrupt("here holds segment 2 of 1/1/1".into());
            assert_eq!(super::footer("here", &bytes, test_branch(branch), seq), Err(corrupt));
        }
    }

    /// Commits `count` commits, tagged on from `tag`, each in a segment of its own, uploaded and
    /// acknowledged, its requests served by `bucket`.
    fn committed(bucket: &SimBucket, writer: &Writer<SimBucket>, tag: u64, count: u64) {
        for tag in tag..tag + count {
            let objects =
                Batch::from(Commit { tag, ..Commit::default() }).close(&mut writer.chain()).frame();
            let seq = writer.add_segment(objects).unwrap();
            bucket.served(writer.upload(seq)).0.unwrap();
            bucket.served(writer.acknowledge()).0.unwrap();
        }
    }

    /// TASK-92.6: recovery reads the footers of a branch's segments by their objects' tails, one,
    /// then two, then four at once, and so on, to the first missing, then lists the checkpoints;
    /// leaving what it read past the first missing. Recovered on, it reads only the segments
    /// since, from where it stopped, finding what recovering anew finds.
    #[test]
    fn recoveries_read_segments_tails_a_window_at_once() {
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        // Its writer's fence, and five segments of a commit each.
        committed(&bucket, &writer, 1, 5);
        let tails = |seqs: std::ops::Range<u64>| -> Vec<String> {
            seqs.map(|seq| {
                format!("get {} last {}", layout::segment(test_branch(1), seq), object::TAIL)
            })
            .collect()
        };
        let checkpoints = vec![format!("list {}", layout::checkpoints(test_branch(1)))];
        let (recovered, rounds) = bucket.served_in_rounds(recover(&*bucket, test_branch(1), None));
        let recovered = recovered.unwrap();
        assert_eq!((recovered.end, recovered.state.last_commit), (6, Some(5)));
        assert_eq!(
            rounds,
            [checkpoints.clone(), tails(0..1), tails(1..3), tails(3..7), checkpoints.clone()]
        );
        let keys: Vec<_> = (0..6).map(|seq| layout::segment(test_branch(1), seq)).collect();
        assert_eq!(recovered.touched.iter().cloned().collect::<Vec<_>>(), keys);

        committed(&bucket, &writer, 6, 2);
        let (on, rounds) = bucket.served_in_rounds(recover_on(&*bucket, None, recovered));
        let on = on.unwrap();
        assert_eq!(rounds, [tails(6..7), tails(7..9), checkpoints]);
        let anew = bucket.served(recover(&*bucket, test_branch(1), None)).0.unwrap();
        assert_eq!((on.state, on.end, on.touched), (anew.state, anew.end, anew.touched));
    }

    /// TASK-92.6: a recovery recovered on once a checkpoint newer than the one it started from has
    /// come recovers anew, from it: the segments it covers may be gone.
    #[test]
    fn recoveries_recovered_on_past_a_newer_checkpoint_recover_anew() {
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        committed(&bucket, &writer, 1, 2);
        let recovered = bucket.served(recover(&*bucket, test_branch(1), None)).0.unwrap();
        committed(&bucket, &writer, 3, 1);
        let seq = bucket.served(writer.checkpoint()).0.unwrap().unwrap();
        let (on, rounds) = bucket.served_in_rounds(recover_on(&*bucket, None, recovered));
        let on = on.unwrap();
        assert_eq!((on.checkpoint, on.end, on.state.last_commit), (Some(seq), seq + 1, Some(3)));
        let tails = |seqs: std::ops::Range<u64>| -> Vec<String> {
            seqs.map(|seq| {
                format!("get {} last {}", layout::segment(test_branch(1), seq), object::TAIL)
            })
            .collect()
        };
        let checkpoints = vec![format!("list {}", layout::checkpoints(test_branch(1)))];
        let checkpoint = vec![format!("get {}", layout::checkpoint(test_branch(1), seq))];
        assert_eq!(
            rounds,
            [
                tails(3..4),
                tails(4..6),
                checkpoints.clone(),
                checkpoints.clone(),
                checkpoint,
                tails(seq + 1..seq + 2),
                checkpoints,
            ]
        );
    }

    /// fxdax: a commit spanning segments (TASK-92.41): the runs each segment going on with it names
    /// are sealed, where they are, once the segment ending it is applied, and not before; its
    /// writer acknowledges none of its segments until then. A writer taking the branch with a commit
    /// begun at its end fences it unended: its runs never sealed, its writer's segments after the
    /// fence void, the next writer's state and recovery's agreeing.
    #[test]
    fn commits_spanning_segments_seal_once_ended() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let extent = |sequence| ExtentId::new(1, sequence);
            let tag = |epoch| layout::tag(writer.epoch(), epoch);
            let landed = async |writer: &Writer<Store>, contents: Contents| {
                let footer = writer.chain().close(contents);
                let data = vec![7; 8192];
                let segment = Segment { data, footer: footer.clone() }.encode().into();
                let seq = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
                writer.upload(seq).await.unwrap();
                (seq, writer.acknowledge().await.unwrap())
            };
            let continuing = |commit, sequence| Contents {
                continued: Some(Continued {
                    commit,
                    extents: vec![(extent(sequence), 4096, 4096)],
                }),
                ..Contents::default()
            };
            let fence = writer.state().through;
            let (first, acked) = landed(&writer, continuing(tag(1), 1)).await;
            assert_eq!(acked, fence, "a segment going on with a commit, acknowledged");
            let (second, acked) = landed(&writer, continuing(tag(1), 2)).await;
            assert_eq!(acked, fence, "a segment going on with a commit, acknowledged");
            let begun = recover(&*bucket, test_branch(1), None).await.unwrap().state;
            assert_eq!((begun.extents.len(), begun.last_commit), (0, None), "{begun:?}");
            let ending = Contents {
                commits: vec![tag(1)],
                extents: vec![(extent(3), 0, 4096)],
                next_sequence: 4,
                ..Contents::default()
            };
            let (ends, acked) = landed(&writer, ending).await;
            assert_eq!(acked, Some(ends));
            let at = |seq, offset| Location {
                key: layout::segment(test_branch(1), seq),
                offset,
                len: 4096,
            };
            let sealed: Vec<_> = writer.state().extents.into_iter().collect();
            let expected = vec![
                (extent(1), at(first, 4096)),
                (extent(2), at(second, 4096)),
                (extent(3), at(ends, 0)),
            ];
            assert_eq!(sealed, expected);
            assert_eq!(writer.state().begun, None);
            let recovered = recover(&*bucket, test_branch(1), None).await.unwrap().state;
            assert_eq!(recovered.extents.into_iter().collect::<Vec<_>>(), expected);
            assert_eq!(recovered.last_commit, Some(tag(1)));

            // Begun, its writer gone: the next fences it unended.
            let (begun, _) = landed(&writer, continuing(tag(2), 4)).await;
            let next = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
            assert_eq!(next.state().through, Some(begun + 1), "the fence after it");
            assert_eq!(next.state().extents.into_iter().collect::<Vec<_>>(), expected);
            assert_eq!((next.state().begun, next.state().last_commit), (None, Some(tag(1))));
            let recovered = recover(&*bucket, test_branch(1), None).await.unwrap().state;
            assert_eq!(recovered, next.state());
        });
    }

    /// TASK-165.3: a branch's capacity is its state, set by its commits, recovered from its
    /// segments and its checkpoints wherever it is taken; a fork's child has its parent's at the
    /// fork, until it sets its own, and a point keeps it as it was, whatever the parent sets after.
    #[test]
    fn capacities_are_set_by_commits() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let parent = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            assert_eq!(parent.state().capacity, None);
            let resize = |tag, capacity: u64| {
                Batch::from(Commit { tag, capacity: Some(capacity), ..Commit::default() })
            };
            let landed = async |writer: &Writer<Store>, objects| {
                let seq = writer.add_segment(objects).unwrap();
                writer.upload(seq).await.unwrap();
                writer.acknowledge().await.unwrap();
                seq
            };
            let forked =
                resize(1, 1 << 20).fork(child(test_branch(2)), &mut parent.chain()).frame();
            let forked = Origin { parent: test_branch(1), at: landed(&parent, forked).await };
            let point = Child { branch: test_branch(3), owner: 0, point: true };
            let pointed = Batch::default().fork(point, &mut parent.chain()).frame();
            let pointed = Origin { parent: test_branch(1), at: landed(&parent, pointed).await };
            landed(&parent, resize(2, 2 << 20).close(&mut parent.chain()).frame()).await;
            let capacity = async |branch, origin| {
                let recovered = recover(&*bucket, branch, origin).await.unwrap();
                (recovered.checkpoint.is_some(), recovered.state.capacity)
            };
            assert_eq!(parent.state().capacity, Some(2 << 20));
            assert_eq!(capacity(test_branch(1), None).await, (false, Some(2 << 20)));
            assert_eq!(capacity(test_branch(2), Some(forked)).await, (false, Some(1 << 20)));
            assert_eq!(capacity(test_branch(3), Some(pointed)).await, (false, Some(1 << 20)));
            parent.checkpoint().await.unwrap();
            assert_eq!(capacity(test_branch(1), None).await, (true, Some(2 << 20)));

            let child = Writer::open(bucket.clone(), test_branch(2), 11, 0).await.unwrap();
            assert_eq!(child.state().capacity, Some(1 << 20));
            landed(&child, resize(3, 4 << 20).close(&mut child.chain()).frame()).await;
            child.checkpoint().await.unwrap();
            assert_eq!(capacity(test_branch(2), Some(forked)).await, (true, Some(4 << 20)));
            assert_eq!(capacity(test_branch(1), None).await, (true, Some(2 << 20)));
            assert_eq!(capacity(test_branch(3), Some(pointed)).await, (false, Some(1 << 20)));
        });
    }

    /// A fork's checkpoints hold its own state, from which it recovers; another branch's
    /// checkpoint in its place is corrupt.
    #[test]
    fn forks_checkpoint_their_own_states() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let parent = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let extent = ExtentId::new(1, parent.chain().next_sequence());
            let extents = vec![(extent, b"one".to_vec())];
            let batch = Batch::from(Commit { tag: 1, extents, ..Commit::default() });
            let at = parent
                .add_segment(batch.fork(child(test_branch(2)), &mut parent.chain()).frame())
                .unwrap();
            parent.upload(at).await.unwrap();
            parent.acknowledge().await.unwrap();
            let origin = Some(Origin { parent: test_branch(1), at });
            assert_eq!(
                branch::record(&*bucket, test_branch(2)).await.unwrap().unwrap().origin(),
                origin
            );

            let child = Writer::open(bucket.clone(), test_branch(2), 11, 0).await.unwrap();
            let seq = child.checkpoint().await.unwrap().unwrap();
            let recovered = recover(&*bucket, test_branch(2), origin).await.unwrap();
            assert_eq!(recovered.checkpoint, Some(seq));
            assert_eq!(recovered.state.branch, test_branch(2));
            assert_eq!(recovered.state.extents.keys().collect::<Vec<_>>(), [&extent]);

            let parents = parent.checkpoint().await.unwrap().unwrap();
            let object =
                bucket::get(&*bucket, &layout::checkpoint(test_branch(1), parents)).await.unwrap();
            let key = layout::checkpoint(test_branch(2), seq + 1);
            bucket::create(&*bucket, &key, object.bytes).await;
            let corrupt = format!("{key} holds a checkpoint of 1/1/1");
            assert_eq!(
                recover(&*bucket, test_branch(2), origin).await.unwrap_err(),
                Error::Corrupt(corrupt)
            );
        });
    }
}
