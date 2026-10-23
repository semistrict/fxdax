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
//! later one's, or a writer's, landed where one of its before it did not. A state recovered is
//! recovered on by the segments that came since ([`recover_on`]): a writer's, once it holds the
//! lease, from what it found before.
//!
//! The collector deletes what no reader starting now needs, segments only once a checkpoint covers
//! them. A reader that finds a segment missing checks that no checkpoint newer than the one it
//! started from has come: if one has, the segment may be one it covers, gone, and it starts again.

use crate::bucket::{self, Bucket};
use crate::layout::{self, Footer, Fork, Location, Origin, Record, Segment};
use crate::{Error, object, rules};
use futures::future::{BoxFuture, FutureExt as _, Shared};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use storage_device::ExtentId;

/// What a branch holds as of a segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub branch: u64,
    pub extents: BTreeMap<ExtentId, Location>,
    pub next_sequence: u64,
    pub root: Option<Vec<u8>>,
    /// The tag of the last commit, which may be the parent's, before the fork.
    pub last_commit: Option<u64>,
    /// The last segment applied, by its number and epoch, which the next follows.
    pub last_segment: Option<(u64, u64)>,
    pub forks: Vec<Fork>,
    /// The highest epoch of the segments applied.
    pub epoch: u64,
    /// The last segment applied.
    pub through: Option<u64>,
}

impl State {
    pub fn new(branch: u64) -> Self {
        Self {
            branch,
            extents: BTreeMap::new(),
            next_sequence: 0,
            root: None,
            last_commit: None,
            last_segment: None,
            forks: Vec::new(),
            epoch: 0,
            through: None,
        }
    }

    /// The state of `child`, forked from this one, but for the extents it inherits: its root, and
    /// its last commit.
    fn inheriting(&self, child: u64) -> Self {
        Self { root: self.root.clone(), last_commit: self.last_commit, ..Self::new(child) }
    }

    /// Applies the segment at `key`, unless it is void. Returns whether it applied it.
    pub fn apply(&mut self, key: &str, footer: &Footer) -> bool {
        self.through = Some(footer.seq);
        if footer.after != self.last_segment && rules().continuity {
            return false;
        }
        self.last_segment = Some((footer.seq, footer.epoch));
        self.epoch = self.epoch.max(footer.epoch);
        for &(extent, offset, len) in &footer.extents {
            self.extents.insert(extent, Location { key: key.to_owned(), offset, len });
        }
        for (extent, location) in &footer.own {
            self.extents.insert(*extent, location.clone());
        }
        for extent in &footer.frees {
            self.extents.remove(extent);
        }
        if let Some(root) = &footer.root {
            self.root = Some(root.clone());
        }
        if let Some(&commit) = footer.commits.last() {
            self.last_commit = Some(commit);
        }
        self.next_sequence = self.next_sequence.max(footer.next_sequence);
        self.forks.extend(footer.forks.iter().map(|&child| Fork { child, at: footer.seq }));
        true
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
    /// The keys it read, which the collector keeps.
    pub touched: BTreeSet<String>,
}

impl Recovered<Layers> {
    /// The branch's state, its layers in one.
    pub(crate) fn flatten(self) -> Recovered {
        let Self { state, end, checkpoint, touched } = self;
        Recovered { state: state.flatten(), end, checkpoint, touched }
    }
}

/// A branch's state in layers: what it inherited, if it has no checkpoint of its own, its
/// parent's state where it was forked, shared, as read, with the other forks of the parent there;
/// under its own, what its segments made of it since. The collector, recovering many forks of a
/// base, takes what they inherited together once, not each fork's copy (TASK-92.7).
#[derive(Clone, Debug)]
pub(crate) struct Layers {
    pub inherited: Option<Arc<State>>,
    /// Its state, but for the extents it inherited and holds still: all of them, with nothing
    /// inherited.
    pub own: State,
    /// The extents it inherited that it holds no longer where it inherited them: freed, or sealed
    /// again.
    pub shadowed: BTreeSet<ExtentId>,
}

impl Layers {
    /// `state`, in one layer.
    fn of(state: State) -> Self {
        Self { inherited: None, own: state, shadowed: BTreeSet::new() }
    }

    /// The state of `child`, forked from `inherited`, before its segments.
    fn inheriting(inherited: Arc<State>, child: u64) -> Self {
        let own = inherited.inheriting(child);
        Self { inherited: Some(inherited), own, shadowed: BTreeSet::new() }
    }

    /// Applies the segment at `key`, as [`State::apply`] does.
    fn apply(&mut self, key: &str, footer: &Footer) {
        if self.own.apply(key, footer)
            && let Some(inherited) = &self.inherited
        {
            let sealed = footer.extents.iter().map(|(extent, ..)| extent);
            let sealed = sealed.chain(footer.own.iter().map(|(extent, _)| extent));
            let changed = sealed.chain(&footer.frees);
            let shadowed = changed.filter(|extent| inherited.extents.contains_key(extent));
            self.shadowed.extend(shadowed);
        }
    }

    /// The state, in one: what it inherited and holds still, and its own.
    fn flatten(self) -> State {
        let Self { inherited, mut own, shadowed } = self;
        if let Some(inherited) = inherited {
            let mut extents = Arc::unwrap_or_clone(inherited).extents;
            for extent in &shadowed {
                extents.remove(extent);
            }
            extents.append(&mut own.extents);
            own.extents = extents;
        }
        own
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
    branch: u64,
    origin: Option<Origin>,
) -> Result<Recovered, Error> {
    Ok(recover_layers(&Checkpoints::new(bucket, 0), branch, origin).await?.flatten())
}

/// The state of `branch`, forked from `origin`, as [`recover`] finds it, in layers: reading the
/// checkpoint it starts from through `checkpoints`, which other recoveries share.
pub(crate) async fn recover_layers<B: Bucket>(
    checkpoints: &Checkpoints<'_, B>,
    branch: u64,
    origin: Option<Origin>,
) -> Result<Recovered<Layers>, Error> {
    for _ in 0..RESTARTS {
        match attempt(checkpoints, branch, origin).await {
            Ok(recovered) => return Ok(recovered),
            Err(Attempt::Restart) => {}
            Err(Attempt::Failed(error)) => return Err(error),
        }
    }
    Err(Error::Lost(format!("branch {branch:x}: what its recovery starts from keeps going")))
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

    /// The state checkpoint `key` of `branch` holds, if it is there: read now, or by the read of
    /// it in flight or kept. One gone, or that failed, is read anew the next time.
    async fn read(&self, key: &str, branch: u64) -> Result<Option<Arc<State>>, Error> {
        let flight = {
            let mut read = self.read.lock().unwrap();
            match read.flights.get(key) {
                Some(flight) => flight.clone(),
                None => {
                    let (bucket, read_key) = (self.bucket, key.to_owned());
                    let flight = async move {
                        let Some(object) = bucket::get(bucket, &read_key).await else {
                            return Ok(None);
                        };
                        Ok(Some(Arc::new(checkpoint(&read_key, &object.bytes, branch)?)))
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

/// The state of the branch `before` recovered, forked from `origin`, recovered on from it: its
/// segments from `before`'s end, those that came since, applied to its state, rather than its
/// state read anew. As [`recover`] would find, unless a checkpoint newer than `before`'s has come,
/// which may cover segments gone since: then it recovers anew.
pub async fn recover_on(
    bucket: &impl Bucket,
    origin: Option<Origin>,
    before: Recovered,
) -> Result<Recovered, Error> {
    let Recovered { state, end, checkpoint, touched } = before;
    let branch = state.branch;
    let before = Recovered { state: Layers::of(state), end, checkpoint, touched };
    match follow(bucket, before).await {
        Ok(recovered) => Ok(recovered.flatten()),
        Err(Attempt::Restart) => recover(bucket, branch, origin).await,
        Err(Attempt::Failed(error)) => Err(error),
    }
}

async fn attempt<B: Bucket>(
    checkpoints: &Checkpoints<'_, B>,
    branch: u64,
    origin: Option<Origin>,
) -> Result<Recovered<Layers>, Attempt> {
    let bucket = checkpoints.bucket;
    let mut touched = BTreeSet::new();
    let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
    let (state, checkpoint) = match listed.last() {
        Some(newest) => {
            let Some(state) = checkpoints.read(&newest.key, branch).await? else {
                return Err(Attempt::Restart);
            };
            touched.insert(newest.key.clone());
            let seq = state.through.ok_or_else(|| Error::Corrupt(newest.key.clone()))?;
            (Layers::of(Arc::unwrap_or_clone(state)), Some(seq))
        }
        None => match origin {
            None => (Layers::of(State::new(branch)), None),
            Some(origin) => {
                let at = state_at(checkpoints, origin.parent, origin.at, &mut touched).await?;
                (Layers::inheriting(at, branch), None)
            }
        },
    };
    let end = checkpoint.map_or(0, |seq| seq + 1);
    follow(bucket, Recovered { state, end, checkpoint, touched }).await
}

/// `recovered`, its segments from its end on applied, to the first missing: the branch's end,
/// unless a checkpoint newer than the one it started from has come, which may cover the segment,
/// gone; then it starts again.
async fn follow(
    bucket: &impl Bucket,
    mut recovered: Recovered<Layers>,
) -> Result<Recovered<Layers>, Attempt> {
    let Recovered { state, end, touched, .. } = &mut recovered;
    let branch = state.own.branch;
    *end = footers(bucket, branch, *end, u64::MAX, |key, footer| {
        state.apply(&key, &footer);
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
    branch: u64,
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
    branch: u64,
    seq: u64,
) -> Result<Option<Footer>, Error> {
    let key = layout::segment(branch, seq);
    let Some(tail) = object::tail(bucket, &key).await? else { return Ok(None) };
    Ok(Some(placed(&key, layout::decode(tail.footer())?, branch, seq)?))
}

/// The footer of segment `seq` of `branch`, whose object holds `bytes`.
pub(crate) fn footer(key: &str, bytes: &[u8], branch: u64, seq: u64) -> Result<Footer, Error> {
    placed(key, Segment::footer(bytes)?, branch, seq)
}

/// `footer`, that of the object at `key`, which holds segment `seq` of `branch`, if it says so.
fn placed(key: &str, footer: Footer, branch: u64, seq: u64) -> Result<Footer, Error> {
    if footer.branch != branch || footer.seq != seq {
        return Err(Error::Corrupt(format!(
            "{key} holds segment {:x} of {:x}",
            footer.seq, footer.branch
        )));
    }
    Ok(footer)
}

/// The state checkpoint `key` of `branch`, whose object holds `bytes`, holds.
fn checkpoint(key: &str, bytes: &[u8], branch: u64) -> Result<State, Error> {
    let state = crate::checkpoint::decode(bytes)?;
    if state.branch != branch {
        return Err(Error::Corrupt(format!("{key} holds a checkpoint of {:x}", state.branch)));
    }
    Ok(state)
}

/// The state of `branch` as of the end of its segment `at`, which is acknowledged: its newest
/// checkpoint up to there, or its origin's state, then its segments up to there. A checkpoint at
/// `at` itself is the state, as read, which the forks made there share.
fn state_at<'a, B: Bucket>(
    checkpoints: &'a Checkpoints<'_, B>,
    branch: u64,
    at: u64,
    touched: &'a mut BTreeSet<String>,
) -> BoxFuture<'a, Result<Arc<State>, Attempt>> {
    Box::pin(async move {
        let bucket = checkpoints.bucket;
        let listed = bucket::list(bucket, &layout::checkpoints(branch)).await;
        let newest = listed.iter().rev().find(|meta| {
            matches!(layout::kind(&meta.key), layout::Kind::Checkpoint { seq, .. } if seq <= at)
        });
        let mut state = match newest {
            Some(meta) => {
                let Some(state) = checkpoints.read(&meta.key, branch).await? else {
                    return Err(Attempt::Restart);
                };
                touched.insert(meta.key.clone());
                if state.through == Some(at) {
                    return Ok(state);
                }
                Layers::of(Arc::unwrap_or_clone(state))
            }
            None => {
                let key = layout::record(branch);
                let Some(object) = bucket::get(bucket, &key).await else {
                    return Err(Attempt::Failed(Error::Lost(key)));
                };
                touched.insert(key);
                let record: Record = layout::decode(&object.bytes)?;
                match record.origin {
                    None => Layers::of(State::new(branch)),
                    Some(origin) => {
                        let inherited =
                            state_at(checkpoints, origin.parent, origin.at, touched).await?;
                        Layers::inheriting(inherited, branch)
                    }
                }
            }
        };
        let from = state.own.through.map_or(0, |seq| seq + 1);
        let applied = footers(bucket, branch, from, at, |key, footer| {
            state.apply(&key, &footer);
            touched.insert(key);
        });
        // Acknowledged segments go only once a checkpoint covers them.
        if applied.await? <= at {
            return Err(Attempt::Restart);
        }
        Ok(Arc::new(state.flatten()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::branch;
    use crate::sim::bucket::SimBucket;
    use crate::store::Store;
    use crate::writer::Writer;
    use object_store::memory::InMemory;
    use std::sync::Arc;

    /// A segment's object holds the segment its key names: of its branch, of its number.
    #[test]
    fn segments_are_where_their_keys_say() {
        let footer = Footer {
            branch: 1,
            seq: 2,
            epoch: 1,
            after: None,
            commits: vec![],
            extents: vec![],
            own: vec![],
            frees: vec![],
            root: None,
            next_sequence: 0,
            forks: vec![],
        };
        let bytes = Segment { data: Vec::new(), footer: footer.clone() }.encode();
        assert_eq!(super::footer("here", &bytes, 1, 2), Ok(footer));
        for (branch, seq) in [(1, 3), (4, 2)] {
            let corrupt = Error::Corrupt("here holds segment 2 of 1".into());
            assert_eq!(super::footer("here", &bytes, branch, seq), Err(corrupt));
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
            branch::create(&*bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        // Its writer's fence, and five segments of a commit each.
        committed(&bucket, &writer, 1, 5);
        let tails = |seqs: std::ops::Range<u64>| -> Vec<String> {
            seqs.map(|seq| format!("get {} last {}", layout::segment(1, seq), object::TAIL))
                .collect()
        };
        let checkpoints = vec![format!("list {}", layout::checkpoints(1))];
        let (recovered, rounds) = bucket.served_in_rounds(recover(&*bucket, 1, None));
        let recovered = recovered.unwrap();
        assert_eq!((recovered.end, recovered.state.last_commit), (6, Some(5)));
        assert_eq!(
            rounds,
            [checkpoints.clone(), tails(0..1), tails(1..3), tails(3..7), checkpoints.clone()]
        );
        let keys: Vec<_> = (0..6).map(|seq| layout::segment(1, seq)).collect();
        assert_eq!(recovered.touched.iter().cloned().collect::<Vec<_>>(), keys);

        committed(&bucket, &writer, 6, 2);
        let (on, rounds) = bucket.served_in_rounds(recover_on(&*bucket, None, recovered));
        let on = on.unwrap();
        assert_eq!(rounds, [tails(6..7), tails(7..9), checkpoints]);
        let anew = bucket.served(recover(&*bucket, 1, None)).0.unwrap();
        assert_eq!((on.state, on.end, on.touched), (anew.state, anew.end, anew.touched));
    }

    /// TASK-92.6: a recovery recovered on once a checkpoint newer than the one it started from has
    /// come recovers anew, from it: the segments it covers may be gone.
    #[test]
    fn recoveries_recovered_on_past_a_newer_checkpoint_recover_anew() {
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        committed(&bucket, &writer, 1, 2);
        let recovered = bucket.served(recover(&*bucket, 1, None)).0.unwrap();
        committed(&bucket, &writer, 3, 1);
        let seq = bucket.served(writer.checkpoint()).0.unwrap().unwrap();
        let (on, rounds) = bucket.served_in_rounds(recover_on(&*bucket, None, recovered));
        let on = on.unwrap();
        assert_eq!((on.checkpoint, on.end, on.state.last_commit), (Some(seq), seq + 1, Some(3)));
        let tails = |seqs: std::ops::Range<u64>| -> Vec<String> {
            seqs.map(|seq| format!("get {} last {}", layout::segment(1, seq), object::TAIL))
                .collect()
        };
        let checkpoints = vec![format!("list {}", layout::checkpoints(1))];
        let checkpoint = vec![format!("get {}", layout::checkpoint(1, seq))];
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

    /// A fork's checkpoints hold its own state, from which it recovers; another branch's
    /// checkpoint in its place is corrupt.
    #[test]
    fn forks_checkpoint_their_own_states() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, 1).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, parent.chain().next_sequence());
            let extents = vec![(extent, b"one".to_vec())];
            let batch = Batch::from(Commit { tag: 1, extents, ..Commit::default() });
            let at = parent.add_segment(batch.fork(2, &mut parent.chain()).frame()).unwrap();
            parent.upload(at).await.unwrap();
            parent.acknowledge().await.unwrap();
            let origin = Some(Origin { parent: 1, at });
            assert_eq!(branch::record(&*bucket, 2).await.unwrap().unwrap().origin, origin);

            let child = Writer::open(bucket.clone(), 2, 11, 0).await.unwrap();
            let seq = child.checkpoint().await.unwrap().unwrap();
            let recovered = recover(&*bucket, 2, origin).await.unwrap();
            assert_eq!(recovered.checkpoint, Some(seq));
            assert_eq!(recovered.state.branch, 2);
            assert_eq!(recovered.state.extents.keys().collect::<Vec<_>>(), [&extent]);

            let parents = parent.checkpoint().await.unwrap().unwrap();
            let object = bucket::get(&*bucket, &layout::checkpoint(1, parents)).await.unwrap();
            let key = layout::checkpoint(2, seq + 1);
            bucket::create(&*bucket, &key, object.bytes).await;
            let corrupt = format!("{key} holds a checkpoint of 1");
            assert_eq!(recover(&*bucket, 2, origin).await.unwrap_err(), Error::Corrupt(corrupt));
        });
    }
}
