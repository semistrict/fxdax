// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's writer: its commits, in segments, uploaded in parallel and acknowledged in order.
//!
//! A writer takes the branch's lease, recovers its state, and fences out the writer before it with
//! an empty segment of its own at the branch's end. Its segments are created where none is,
//! numbered on from there, each naming the one it follows; it acknowledges one once it and every
//! one before it are created and a read of the lease, after them, shows the lease still its. So a
//! segment it acknowledges was in the branch before any later writer, which took the lease after,
//! looked for the branch's end; and a segment of its that lands after a later writer's fence
//! follows none of the branch's, and is void.
//!
//! A writer makes segments of the commits it is given ([`Writer::commit`]), or takes segments a
//! local store closed, framed and with their footers, which it uploads as they are
//! ([`Writer::add_segment`]): a store's closed segment is the object (doc-7, decision 1).

use crate::bucket::{self, Bucket, Created};
use crate::layout::{self, Footer, Location, Origin, Segment};
use crate::reader::{self, MovesMaps, State};
use crate::{EPOCH_SHIFT, Error, branch, rules};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::ExtentId;

/// A commit: the extents it seals, which it minted, stored in its segment or, `own`, on their
/// own; the extents it frees; and the root, if it sets it. `tag` names it in footers.
#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub tag: u64,
    pub extents: Vec<(ExtentId, Vec<u8>)>,
    pub own: Vec<(ExtentId, Vec<u8>)>,
    pub frees: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
}

pub struct Writer<B: Bucket> {
    bucket: Arc<B>,
    branch: u64,
    epoch: u64,
    inner: Mutex<Inner>,
}

struct Inner {
    /// As of the last segment acknowledged.
    state: State,
    /// What the commits since the last segment closed make.
    open: Open,
    /// The number of the next segment.
    next_seq: u64,
    /// The last segment closed, which the next follows.
    last_segment: Option<(u64, u64)>,
    /// The segments closed and not yet acknowledged.
    pending: BTreeMap<u64, Pending>,
    /// The next sequence number to mint.
    next_sequence: u64,
    /// The newest checkpoint.
    checkpoint: Option<u64>,
    /// The forks whose children have their records.
    recorded: BTreeSet<u64>,
    /// Why it stopped, if it did.
    stopped: Option<Error>,
}

#[derive(Default)]
struct Open {
    data: Vec<u8>,
    commits: Vec<u64>,
    extents: Vec<(ExtentId, u64, u64)>,
    own: Vec<(ExtentId, Location, Vec<u8>)>,
    frees: Vec<ExtentId>,
    root: Option<Vec<u8>>,
    forks: Vec<u64>,
}

impl Open {
    fn is_empty(&self) -> bool {
        self.commits.is_empty() && self.forks.is_empty()
    }
}

struct Pending {
    /// Its object, and its footer.
    bytes: Arc<[u8]>,
    footer: Footer,
    /// The objects of its extents stored on their own, uploaded first.
    own: Vec<(String, Arc<[u8]>)>,
    uploaded: bool,
}

/// A segment of no commits, following `state`: a writer's fence, or one filling a gap.
fn empty(branch: u64, seq: u64, epoch: u64, state: &State) -> Segment {
    Segment {
        data: Vec::new(),
        footer: Footer {
            branch,
            seq,
            epoch,
            after: state.last_segment,
            commits: Vec::new(),
            extents: Vec::new(),
            own: Vec::new(),
            frees: Vec::new(),
            root: None,
            next_sequence: state.next_sequence,
            forks: Vec::new(),
        },
    }
}

impl<B: Bucket> Writer<B> {
    /// Takes `branch` over: its lease, as `holder`, which must differ from every other taking's, of
    /// the placement whose fencing token is `token` ([`branch::take_lease`]).
    pub async fn open(bucket: Arc<B>, branch: u64, holder: u64, token: u64) -> Result<Self, Error> {
        let record = branch::record(&*bucket, branch).await?.ok_or(Error::NoBranch)?;
        if record.deleted {
            return Err(Error::Deleted);
        }
        let epoch = branch::take_lease(&*bucket, &layout::lease(branch), holder, token).await?;
        // Deleted meanwhile, and its lease removed: the collector removes the lease taken here.
        if branch::record(&*bucket, branch).await?.is_none_or(|record| record.deleted) {
            return Err(Error::Deleted);
        }
        let recovered = reader::recover(&*bucket, branch, record.origin).await?;
        let mut state = recovered.state;
        let mut seq = recovered.end;
        // Its fence: an empty segment of its at the branch's end, so that no earlier writer's
        // segment, landing late, changes the state it starts from; one that landed there first is
        // the branch's, and the fence goes after it. Segments past the end, earlier writers' landed
        // past a gap, are void, and empty segments fill the gaps between them, so that this
        // writer's follow without one.
        let last = bucket::list(&*bucket, &layout::segments(branch))
            .await
            .iter()
            .filter_map(|meta| match layout::kind(&meta.key) {
                layout::Kind::Segment { seq, .. } => Some(seq),
                _ => None,
            })
            .max();
        let mut fenced = !rules().fence;
        while !fenced || last.is_some_and(|last| seq <= last) {
            let key = layout::segment(branch, seq);
            let empty = empty(branch, seq, epoch, &state);
            let footer = match bucket::create(&*bucket, &key, empty.encode()).await {
                Created::Created => {
                    fenced = true;
                    empty.footer
                }
                Created::Exists(object) => {
                    let footer = reader::footer(&key, &object.bytes, branch, seq)?;
                    if footer.epoch > epoch {
                        return Err(Error::Fenced);
                    }
                    footer
                }
            };
            state.apply(&key, &footer);
            seq += 1;
        }
        let mut next_sequence = state.next_sequence;
        if rules().epoch_sequences {
            // Still its lease, no later writer's segment, minted from a later range, is in what
            // it recovered.
            if branch::lease_epoch(&*bucket, &layout::lease(branch)).await? != epoch {
                return Err(Error::Fenced);
            }
            next_sequence = next_sequence.max(epoch << EPOCH_SHIFT);
        }
        let last_segment = state.last_segment;
        let writer = Self {
            bucket,
            branch,
            epoch,
            inner: Mutex::new(Inner {
                state,
                open: Open::default(),
                next_seq: seq,
                last_segment,
                pending: BTreeMap::new(),
                next_sequence,
                checkpoint: recovered.checkpoint,
                recorded: BTreeSet::new(),
                stopped: None,
            }),
        };
        // Forks acknowledged by the writers before, whose children may lack their records.
        writer.record_forks().await?;
        Ok(writer)
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn branch(&self) -> u64 {
        self.branch
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The branch's state as of the last segment acknowledged.
    pub fn state(&self) -> State {
        self.inner().state.clone()
    }

    /// Where `extent` is, as of the last segment acknowledged, if the branch holds it.
    pub fn location(&self, extent: ExtentId) -> Option<Location> {
        self.inner().state.extents.get(&extent).cloned()
    }

    /// The next sequence number for the branch's extents: the guest mints from here on.
    pub fn next_sequence(&self) -> u64 {
        self.inner().next_sequence
    }

    /// The number of the next segment, and the segment it follows: where a store's segments for
    /// this writer go ([`Writer::add_segment`]).
    pub fn next_segment(&self) -> (u64, Option<(u64, u64)>) {
        let inner = self.inner();
        (inner.next_seq, inner.last_segment)
    }

    /// Why the writer stopped, if it did.
    pub fn stopped(&self) -> Option<Error> {
        self.inner().stopped.clone()
    }

    fn stop(&self, error: Error) -> Error {
        self.inner().stopped.get_or_insert(error).clone()
    }

    /// Adds `commit` to the open segment.
    pub fn commit(&self, commit: Commit) -> Result<(), Error> {
        let mut inner = self.inner();
        if let Some(error) = &inner.stopped {
            return Err(error.clone());
        }
        let inner = &mut *inner;
        for (extent, bytes) in commit.extents {
            let offset = inner.open.data.len() as u64;
            inner.open.data.extend_from_slice(&bytes);
            inner.open.extents.push((extent, offset, bytes.len() as u64));
            inner.next_sequence = inner.next_sequence.max(extent.sequence + 1);
        }
        for (extent, bytes) in commit.own {
            let key = layout::extent(self.epoch, extent);
            let location = Location { key, offset: 0, len: bytes.len() as u64 };
            inner.open.own.push((extent, location, bytes));
            inner.next_sequence = inner.next_sequence.max(extent.sequence + 1);
        }
        inner.open.frees.extend(commit.frees);
        if commit.root.is_some() {
            inner.open.root = commit.root;
        }
        inner.open.commits.push(commit.tag);
        Ok(())
    }

    /// Forks `child` from the branch as of its commits so far: closes their segment, which it
    /// returns. The child gets its record once the segment is acknowledged ([`Writer::acknowledge`]).
    pub fn fork(&self, child: u64) -> Result<u64, Error> {
        let mut inner = self.inner();
        if let Some(error) = &inner.stopped {
            return Err(error.clone());
        }
        inner.open.forks.push(child);
        drop(inner);
        Ok(self.close().expect("a segment with a fork closes"))
    }

    /// Closes the open segment, if any commits are in it, to be uploaded ([`Writer::upload`]).
    pub fn close(&self) -> Option<u64> {
        let mut inner = self.inner();
        let inner = &mut *inner;
        if inner.open.is_empty() || inner.stopped.is_some() {
            return None;
        }
        let open = std::mem::take(&mut inner.open);
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let after = inner.last_segment.replace((seq, self.epoch));
        let segment = Segment {
            data: open.data,
            footer: Footer {
                branch: self.branch,
                seq,
                epoch: self.epoch,
                after,
                commits: open.commits,
                extents: open.extents,
                own: open
                    .own
                    .iter()
                    .map(|(extent, location, _)| (*extent, location.clone()))
                    .collect(),
                frees: open.frees,
                root: open.root,
                next_sequence: inner.next_sequence,
                forks: open.forks,
            },
        };
        let own = (open.own.into_iter())
            .map(|(_, location, bytes)| (location.key, layout::with_footer(&bytes, &()).into()))
            .collect();
        let bytes = segment.encode().into();
        inner.pending.insert(seq, Pending { bytes, footer: segment.footer, own, uploaded: false });
        Some(seq)
    }

    /// Takes a segment a store closed, its object `bytes`, as the next to upload, with the objects
    /// of the extents its footer says are stored on their own, by their keys. Its footer must name
    /// this writer, and follow on from its last segment: the store numbered it so.
    pub fn add_segment(
        &self,
        bytes: Arc<[u8]>,
        own: Vec<(String, Arc<[u8]>)>,
    ) -> Result<u64, Error> {
        let footer: Footer = layout::footer(&bytes)?;
        let mut inner = self.inner();
        if let Some(error) = &inner.stopped {
            return Err(error.clone());
        }
        let wanted = (self.branch, inner.next_seq, self.epoch, inner.last_segment);
        if (footer.branch, footer.seq, footer.epoch, footer.after) != wanted {
            return Err(Error::Corrupt(format!(
                "a segment of branch {:x}, number {}, epoch {}, after {:?}, not {wanted:?}",
                footer.branch, footer.seq, footer.epoch, footer.after
            )));
        }
        let keys: BTreeSet<&String> = own.iter().map(|(key, _)| key).collect();
        for (extent, location) in &footer.own {
            if location.key != layout::extent(self.epoch, *extent) || !keys.contains(&location.key)
            {
                return Err(Error::Corrupt(format!("{extent:?} is stored at {}", location.key)));
            }
        }
        let seq = footer.seq;
        inner.next_seq += 1;
        inner.last_segment = Some((seq, self.epoch));
        inner.next_sequence = inner.next_sequence.max(footer.next_sequence);
        inner.pending.insert(seq, Pending { bytes, footer, own, uploaded: false });
        Ok(seq)
    }

    /// Uploads segment `seq`: its extents stored on their own, at once, then it.
    pub async fn upload(&self, seq: u64) -> Result<(), Error> {
        let Some((key, bytes, own)) = self.inner().pending.get(&seq).map(|pending| {
            (layout::segment(self.branch, seq), pending.bytes.clone(), pending.own.clone())
        }) else {
            return Ok(());
        };
        // Its extents of their own at once, then it.
        let creates = own.into_iter().map(|(key, bytes)| async move {
            match bucket::create(&*self.bucket, &key, bytes.to_vec()).await {
                Created::Created => Ok(()),
                // Uploaded before, by this writer: its key names its epoch.
                Created::Exists(object) if *object.bytes == *bytes => Ok(()),
                Created::Exists(_) => Err(Error::Corrupt(format!("{key} holds another extent"))),
            }
        });
        if let Err(error) = futures::future::try_join_all(creates).await {
            return Err(self.stop(error));
        }
        match bucket::create(&*self.bucket, &key, bytes.to_vec()).await {
            Created::Created => {
                if let Some(pending) = self.inner().pending.get_mut(&seq) {
                    pending.uploaded = true;
                }
                Ok(())
            }
            Created::Exists(object) => {
                let footer = reader::footer(&key, &object.bytes, self.branch, seq)?;
                Err(self.stop(if footer.epoch > self.epoch {
                    Error::Fenced
                } else {
                    Error::Reopen
                }))
            }
        }
    }

    /// Acknowledges the segments uploaded with every one before them, once the lease shows them
    /// the branch's. Returns the last segment acknowledged.
    pub async fn acknowledge(&self) -> Result<Option<u64>, Error> {
        let through = {
            let inner = self.inner();
            if let Some(error) = &inner.stopped {
                return Err(error.clone());
            }
            inner
                .pending
                .iter()
                .take_while(|(_, pending)| pending.uploaded)
                .map(|(seq, _)| *seq)
                .last()
        };
        let Some(through) = through else {
            return Ok(self.inner().state.through);
        };
        if rules().acks_after_lease
            && branch::lease_epoch(&*self.bucket, &layout::lease(self.branch)).await? != self.epoch
        {
            return Err(self.stop(Error::Fenced));
        }
        {
            let mut inner = self.inner();
            let inner = &mut *inner;
            while let Some(entry) = inner.pending.first_entry() {
                if *entry.key() > through {
                    break;
                }
                let (seq, pending) = entry.remove_entry();
                inner.state.apply(&layout::segment(self.branch, seq), &pending.footer);
            }
        }
        self.record_forks().await?;
        Ok(self.inner().state.through)
    }

    /// Gives the children of acknowledged forks their records.
    async fn record_forks(&self) -> Result<(), Error> {
        let forks: Vec<_> = {
            let inner = self.inner();
            inner
                .state
                .forks
                .iter()
                .filter(|fork| !inner.recorded.contains(&fork.child))
                .copied()
                .collect()
        };
        for fork in forks {
            let origin = Origin { parent: self.branch, at: fork.at };
            branch::record_fork(&*self.bucket, fork.child, origin).await?;
            self.inner().recorded.insert(fork.child);
        }
        Ok(())
    }

    /// Writes a checkpoint as of the last segment acknowledged, with the extents the collector
    /// moved where they went.
    pub async fn checkpoint(&self) -> Result<Option<u64>, Error> {
        let mut state = {
            let inner = self.inner();
            if inner.state.through.is_none() || inner.state.through <= inner.checkpoint {
                return Ok(inner.checkpoint);
            }
            inner.state.clone()
        };
        let seq = state.through.unwrap();
        let mut maps = MovesMaps::default();
        let mut moved = Vec::new();
        for (extent, old) in &state.extents {
            let location = maps.follow(&*self.bucket, *extent, old).await?;
            if location != *old {
                moved.push((*extent, old.clone(), location));
            }
        }
        for (extent, _, location) in &moved {
            state.extents.insert(*extent, location.clone());
        }
        // Forks whose children were deleted. Their records stay; their forks need not.
        let mut gone = Vec::new();
        for fork in &state.forks {
            if branch::record(&*self.bucket, fork.child).await?.is_some_and(|record| record.deleted)
            {
                gone.push(fork.child);
            }
        }
        state.forks.retain(|fork| !gone.contains(&fork.child));
        let key = layout::checkpoint(self.branch, seq);
        // Another writer's of the same segment holds the same extents, perhaps elsewhere.
        bucket::create(&*self.bucket, &key, crate::checkpoint::encode(&state)).await;
        let mut inner = self.inner();
        inner.state.forks.retain(|fork| !gone.contains(&fork.child));
        for (extent, old, new) in moved {
            if inner.state.extents.get(&extent) == Some(&old) {
                inner.state.extents.insert(extent, new);
            }
        }
        inner.checkpoint = inner.checkpoint.max(Some(seq));
        Ok(inner.checkpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use object_store::memory::InMemory;

    fn memory() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), ""))
    }

    /// A writer that finds a later writer's segment past the branch's end, where its fence and the
    /// segments filling the gap go, is fenced.
    #[test]
    fn writers_finding_a_later_writers_segment_are_fenced() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            for _ in 0..2 {
                branch::take_lease(&*bucket, &layout::lease(1), 9, 0).await.unwrap();
            }
            let later = empty(1, 1, 5, &State::new(1));
            bucket::create(&*bucket, &layout::segment(1, 1), later.encode()).await;
            let opened = Writer::open(bucket.clone(), 1, 10, 0).await;
            assert_eq!(opened.err(), Some(Error::Fenced));
        });
    }

    /// A writer another took the branch from, whose fence takes its segment's place, stops, and
    /// says why; its commits fail.
    #[test]
    fn writers_taken_over_stop() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let first = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let _second = Writer::open(bucket.clone(), 1, 11, 0).await.unwrap();
            first.commit(Commit { tag: 1, ..Commit::default() }).unwrap();
            let seq = first.close().unwrap();
            assert_eq!(first.upload(seq).await, Err(Error::Fenced));
            assert_eq!(first.stopped(), Some(Error::Fenced));
            assert_eq!(first.acknowledge().await, Err(Error::Fenced));
            let commit = first.commit(Commit { tag: 2, ..Commit::default() });
            assert_eq!(commit, Err(Error::Fenced));
        });
    }

    /// The sequence numbers a writer mints from are past every extent its commits sealed, in
    /// its segments and on their own.
    #[test]
    fn writers_mint_past_what_their_commits_seal() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket, 1, 10, 0).await.unwrap();
            let first = writer.next_sequence();
            let extents = vec![(ExtentId::new(1, first + 3), b"in".to_vec())];
            writer.commit(Commit { tag: 1, extents, ..Commit::default() }).unwrap();
            assert_eq!(writer.next_sequence(), first + 4);
            let own = vec![(ExtentId::new(1, first + 7), b"own".to_vec())];
            writer.commit(Commit { tag: 2, own, ..Commit::default() }).unwrap();
            assert_eq!(writer.next_sequence(), first + 8);
        });
    }

    /// A writer takes a segment a store closed for it, and uploads it as it is; one numbered
    /// otherwise, of another writer, or naming its extents of their own elsewhere, it refuses.
    #[test]
    fn writers_upload_segments_stores_closed() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let (seq, after) = writer.next_segment();
            let (small, large) = (ExtentId::new(1, 2), ExtentId::new(1, 3));
            let own_key = layout::extent(writer.epoch(), large);
            let footer = Footer {
                branch: 1,
                seq,
                epoch: writer.epoch(),
                after,
                commits: vec![7],
                extents: vec![(small, 0, 3)],
                own: vec![(large, Location { key: own_key.clone(), offset: 0, len: 5 })],
                frees: vec![],
                root: Some(b"root".to_vec()),
                next_sequence: 4,
                forks: vec![],
            };
            let segment = |footer: &Footer| -> Arc<[u8]> {
                Segment { data: b"abc".to_vec(), footer: footer.clone() }.encode().into()
            };
            let own: Arc<[u8]> = layout::with_footer(b"large", &()).into();
            let owns = || vec![(own_key.clone(), own.clone())];
            for wrong in [
                Footer { seq: seq + 1, ..footer.clone() },
                Footer { epoch: writer.epoch() + 1, ..footer.clone() },
                Footer { after: Some((seq, 99)), ..footer.clone() },
            ] {
                assert!(writer.add_segment(segment(&wrong), owns()).is_err(), "{wrong:?}");
            }
            assert!(writer.add_segment(segment(&footer), vec![]).is_err());
            let bytes = segment(&footer);
            assert_eq!(writer.add_segment(bytes.clone(), owns()), Ok(seq));
            assert_eq!(writer.next_segment(), (seq + 1, Some((seq, writer.epoch()))));
            writer.upload(seq).await.unwrap();
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));
            let object = bucket::get(&*bucket, &layout::segment(1, seq)).await.unwrap();
            assert_eq!(*object.bytes, *bytes);
            let object = bucket::get(&*bucket, &own_key).await.unwrap();
            assert_eq!(*object.bytes, *own);
            let state = writer.state();
            assert_eq!((state.last_commit, state.root), (Some(7), Some(b"root".to_vec())));
            assert_eq!(crate::read(&*bucket, small, &state.extents[&small]).await.unwrap(), b"abc");
            assert_eq!(
                crate::read(&*bucket, large, &state.extents[&large]).await.unwrap(),
                b"large"
            );
        });
    }
}
