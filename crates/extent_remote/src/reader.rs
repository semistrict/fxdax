// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's state, recovered from its objects, and extents read from where it says they are.
//!
//! A branch's state is its newest checkpoint, or, with none, its parent's state where it was
//! forked, then the footers of its segments after it, in order, to the first one missing. A
//! segment that does not follow the last one applied, which its footer names, is void and
//! skipped: an earlier writer's, landed after a later one's, or a writer's, landed where
//! one of its before it did not.
//!
//! The collector deletes what no reader starting now needs, segments only once a checkpoint covers
//! them. A reader that finds a segment missing checks that no checkpoint newer than the one it
//! started from has come: if one has, the segment may be one it covers, gone, and it starts again.

use crate::bucket::{self, Bucket};
use crate::layout::{self, Footer, Fork, Location, Moves, Origin, Record, Segment};
use crate::{Error, rules};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

    /// The state of `child`, forked from this one.
    fn forked(self, child: u64) -> Self {
        Self {
            branch: child,
            next_sequence: 0,
            last_segment: None,
            forks: Vec::new(),
            epoch: 0,
            through: None,
            ..self
        }
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

/// A branch's state, and what reading it took.
#[derive(Clone, Debug)]
pub struct Recovered {
    pub state: State,
    /// The first segment missing: where the next writer's go.
    pub end: u64,
    /// The newest checkpoint, if it started from one.
    pub checkpoint: Option<u64>,
    /// The keys it read, which the collector keeps.
    pub touched: BTreeSet<String>,
}

/// How often recovery starts again before it gives up on a branch as lost.
const RESTARTS: usize = 64;

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
    for _ in 0..RESTARTS {
        match attempt(bucket, branch, origin).await {
            Ok(recovered) => return Ok(recovered),
            Err(Attempt::Restart) => {}
            Err(Attempt::Failed(error)) => return Err(error),
        }
    }
    Err(Error::Lost(format!("branch {branch:x}: what its recovery starts from keeps going")))
}

async fn attempt(
    bucket: &impl Bucket,
    branch: u64,
    origin: Option<Origin>,
) -> Result<Recovered, Attempt> {
    let mut touched = BTreeSet::new();
    let checkpoints = bucket::list(bucket, &layout::checkpoints(branch)).await;
    let (mut state, checkpoint) = match checkpoints.last() {
        Some(newest) => {
            let Some(object) = bucket::get(bucket, &newest.key).await else {
                return Err(Attempt::Restart);
            };
            touched.insert(newest.key.clone());
            let state = checkpoint(&newest.key, &object.bytes, branch)?;
            let seq = state.through.ok_or_else(|| Error::Corrupt(newest.key.clone()))?;
            (state, Some(seq))
        }
        None => match origin {
            None => (State::new(branch), None),
            Some(origin) => (
                state_at(bucket, origin.parent, origin.at, &mut touched).await?.forked(branch),
                None,
            ),
        },
    };
    let mut seq = checkpoint.map_or(0, |seq| seq + 1);
    loop {
        let key = layout::segment(branch, seq);
        let Some(object) = bucket::get(bucket, &key).await else {
            if rules().newest_checkpoint {
                // The end, unless a newer checkpoint covers the segment, which the collector then
                // removed.
                let newest =
                    bucket::list(bucket, &layout::checkpoints(branch)).await.last().map(|meta| {
                        match layout::kind(&meta.key) {
                            layout::Kind::Checkpoint { seq, .. } => seq,
                            _ => unreachable!(),
                        }
                    });
                if newest != checkpoint {
                    return Err(Attempt::Restart);
                }
            }
            return Ok(Recovered { state, end: seq, checkpoint, touched });
        };
        let footer = footer(&key, &object.bytes, branch, seq)?;
        touched.insert(key.clone());
        state.apply(&key, &footer);
        seq += 1;
    }
}

/// The footer of segment `seq` of `branch`, whose object holds `bytes`.
pub(crate) fn footer(key: &str, bytes: &[u8], branch: u64, seq: u64) -> Result<Footer, Error> {
    let footer = Segment::footer(bytes)?;
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
/// checkpoint up to there, or its origin's state, then its segments up to there.
fn state_at<'a>(
    bucket: &'a impl Bucket,
    branch: u64,
    at: u64,
    touched: &'a mut BTreeSet<String>,
) -> BoxFuture<'a, Result<State, Attempt>> {
    Box::pin(async move {
        let checkpoints = bucket::list(bucket, &layout::checkpoints(branch)).await;
        let newest = checkpoints.iter().rev().find(|meta| {
            matches!(layout::kind(&meta.key), layout::Kind::Checkpoint { seq, .. } if seq <= at)
        });
        let mut state = match newest {
            Some(meta) => {
                let Some(object) = bucket::get(bucket, &meta.key).await else {
                    return Err(Attempt::Restart);
                };
                touched.insert(meta.key.clone());
                checkpoint(&meta.key, &object.bytes, branch)?
            }
            None => {
                let key = layout::record(branch);
                let Some(object) = bucket::get(bucket, &key).await else {
                    return Err(Attempt::Failed(Error::Lost(key)));
                };
                touched.insert(key);
                let record: Record = layout::decode(&object.bytes)?;
                match record.origin {
                    None => State::new(branch),
                    Some(origin) => {
                        state_at(bucket, origin.parent, origin.at, touched).await?.forked(branch)
                    }
                }
            }
        };
        let mut seq = state.through.map_or(0, |seq| seq + 1);
        while seq <= at {
            let key = layout::segment(branch, seq);
            let Some(object) = bucket::get(bucket, &key).await else {
                // Acknowledged segments go only once a checkpoint covers them.
                return Err(Attempt::Restart);
            };
            let footer = footer(&key, &object.bytes, branch, seq)?;
            touched.insert(key.clone());
            state.apply(&key, &footer);
            seq += 1;
        }
        Ok(state)
    })
}

/// How many times an extent may have moved.
pub(crate) const MOVES: usize = 64;

/// The bytes of `extent`, at `location` or where the collector moved it from there: a ranged read
/// of its object's tail, and of each part it is in, checked and decompressed. A host reads extents
/// by parts it keeps ([`crate::fetch`]).
pub async fn read(
    bucket: &impl Bucket,
    extent: ExtentId,
    location: &Location,
) -> Result<Vec<u8>, Error> {
    let mut maps = MovesMaps::default();
    let mut location = location.clone();
    for _ in 0..MOVES {
        let key = &location.key;
        if let Some((parts, _)) = layout::read_tail(bucket, key).await? {
            let range = location.offset..location.offset + location.len;
            if range.end > parts.frame.data_len {
                return Err(Error::Corrupt(format!("{extent:?} past the end of {key}")));
            }
            if let Some(bytes) = layout::read_data(bucket, key, &parts, range).await? {
                return Ok(bytes);
            }
        }
        location = match maps.moved(bucket, extent, &location.key).await? {
            Moved::To(to) => to,
            Moved::No | Moved::Dropped => {
                return Err(Error::Lost(format!("{extent:?} in {}", location.key)));
            }
        };
    }
    Err(Error::Corrupt(format!("{extent:?} moved too often")))
}

/// Whether the collector moved an extent when it compacted the object holding it.
pub(crate) enum Moved {
    /// It did not compact the object.
    No,
    To(Location),
    /// It compacted the object and left the extent out: no live branch held it.
    Dropped,
}

/// The maps of compacted objects' moves, each read once.
#[derive(Default)]
pub(crate) struct MovesMaps {
    maps: HashMap<String, Option<BTreeMap<ExtentId, Location>>>,
}

impl MovesMaps {
    /// Whether the collector moved `extent` from the object at `key`, and where to.
    pub async fn moved(
        &mut self,
        bucket: &impl Bucket,
        extent: ExtentId,
        key: &str,
    ) -> Result<Moved, Error> {
        if !self.maps.contains_key(key) {
            let moves: Option<Moves> = layout::read_footer(bucket, &layout::moves(key)).await?;
            let map = moves.map(|moves| moves.extents.into_iter().collect());
            self.maps.insert(key.to_owned(), map);
        }
        Ok(match &self.maps[key] {
            None => Moved::No,
            Some(map) => map.get(&extent).cloned().map_or(Moved::Dropped, Moved::To),
        })
    }

    /// Where `extent` at `location` is now, following its moves as far as they go.
    pub async fn follow(
        &mut self,
        bucket: &impl Bucket,
        extent: ExtentId,
        location: &Location,
    ) -> Result<Location, Error> {
        let mut location = location.clone();
        // An extent the collector dropped no live branch held.
        while let Moved::To(to) = self.moved(bucket, extent, &location.key).await? {
            location = to;
        }
        Ok(location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branch;
    use crate::store::Store;
    use crate::writer::{Commit, Writer};
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

    /// A fork's checkpoints hold its own state, from which it recovers; another branch's
    /// checkpoint in its place is corrupt.
    #[test]
    fn forks_checkpoint_their_own_states() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, 1).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, parent.next_sequence());
            let extents = vec![(extent, b"one".to_vec())];
            parent.commit(Commit { tag: 1, extents, ..Commit::default() }).unwrap();
            let at = parent.fork(2).unwrap();
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
