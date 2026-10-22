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
//! follows none of the branch's, and is void. A writer that finds another's segment where its own
//! goes, or the lease another's, stops for good ([`Writer::stopped`]): fenced, or displaced by an
//! earlier writer's segment landed late ([`Error::Displaced`]). Only a writer taking the branch
//! anew goes on with it.
//!
//! Its segments' footers are stamped by its chain ([`Writer::chain`]): those a local store closed,
//! framed, which it uploads as they are (a store's closed segment is the object, doc-7, decision
//! 1), and those of batches of commits it is given ([`Batch`](crate::Batch)) alike. It takes each
//! one way ([`Writer::add_segment`]), admitting it only if its chain would have stamped it so.

use crate::bucket::{self, Bucket, Created};
use crate::layout::{self, Chain, Contents, Footer, Location, Origin, Segment};
use crate::moves::MovesMaps;
use crate::recovery::{self, State};
use crate::{EPOCH_SHIFT, Error, branch, rules};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::ExtentId;

/// A segment's objects, as a writer uploads them ([`Writer::add_segment`]): the segment's, and
/// those of the extents its footer says are stored on their own, by their keys, which go first.
pub struct Objects {
    pub segment: Arc<[u8]>,
    pub own: Vec<(String, Arc<[u8]>)>,
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
    /// Where its next segment goes: its number, the segment it follows, the last it took, and the
    /// branch's next sequence number as of that.
    chain: Chain,
    /// The segments taken and not yet acknowledged.
    pending: BTreeMap<u64, Pending>,
    /// The newest checkpoint.
    checkpoint: Option<u64>,
    /// The forks whose children have their records.
    recorded: BTreeSet<u64>,
    /// Why it stopped, if it did.
    stopped: Option<Error>,
}

struct Pending {
    /// Its object, and its footer.
    bytes: Arc<[u8]>,
    footer: Footer,
    /// The objects of its extents stored on their own, uploaded first.
    own: Vec<(String, Arc<[u8]>)>,
    uploaded: bool,
}

/// The chain of the writer of lease epoch `epoch` whose next segment is `seq`, after `state`: its
/// next segment follows the state's last, and names the branch's next sequence number as the state
/// has it, or `floor`, at least.
fn chain_after(state: &State, epoch: u64, seq: u64, floor: u64) -> Chain {
    Chain::new(state.branch, epoch, seq, state.last_segment, state.next_sequence.max(floor))
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
        let recovered = recovery::recover(&*bucket, branch, record.origin).await?;
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
            let footer = chain_after(&state, epoch, seq, 0).close(Contents::default());
            let empty = Segment { data: Vec::new(), footer };
            let footer = match bucket::create(&*bucket, &key, empty.encode()).await {
                Created::Created => {
                    fenced = true;
                    empty.footer
                }
                Created::Exists(object) => {
                    let footer = recovery::footer(&key, &object.bytes, branch, seq)?;
                    if footer.epoch > epoch {
                        return Err(Error::Fenced);
                    }
                    footer
                }
            };
            state.apply(&key, &footer);
            seq += 1;
        }
        let mut floor = 0;
        if rules().epoch_sequences {
            // Still its lease, no later writer's segment, minted from a later range, is in what
            // it recovered.
            if branch::lease_epoch(&*bucket, &layout::lease(branch)).await? != epoch {
                return Err(Error::Fenced);
            }
            floor = epoch << EPOCH_SHIFT;
        }
        let chain = chain_after(&state, epoch, seq, floor);
        let writer = Self {
            bucket,
            branch,
            epoch,
            inner: Mutex::new(Inner {
                state,
                chain,
                pending: BTreeMap::new(),
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

    /// Where the writer's next segment goes: its chain, as of the last segment it took, which
    /// stamps the footer of the next, a store's or a batch's ([`Writer::add_segment`]). The
    /// branch's next sequence number it names is what the guest mints from.
    pub fn chain(&self) -> Chain {
        self.inner().chain
    }

    /// Why the writer stopped, if it did.
    pub fn stopped(&self) -> Option<Error> {
        self.inner().stopped.clone()
    }

    fn stop(&self, error: Error) -> Error {
        self.inner().stopped.get_or_insert(error).clone()
    }

    /// Takes a segment, `objects`, as the next to upload ([`Writer::upload`]): one a store closed,
    /// or a batch's ([`Batch`](crate::Batch)), closed on the writer's chain. Its footer must be the
    /// next the chain would have stamped ([`Chain::admit`]), and the objects of the extents it says
    /// are stored on their own must come with it, by their keys. Returns its number.
    pub fn add_segment(&self, objects: Objects) -> Result<u64, Error> {
        let Objects { segment, own } = objects;
        let footer: Footer = layout::footer(&segment)?;
        let mut inner = self.inner();
        if let Some(error) = &inner.stopped {
            return Err(error.clone());
        }
        let chain = inner.chain.admit(&footer)?;
        let keys: BTreeSet<&String> = own.iter().map(|(key, _)| key).collect();
        for (extent, location) in &footer.own {
            if !keys.contains(&location.key) {
                return Err(Error::Corrupt(format!("{extent:?} is stored at {}", location.key)));
            }
        }
        inner.chain = chain;
        let seq = footer.seq;
        inner.pending.insert(seq, Pending { bytes: segment, footer, own, uploaded: false });
        Ok(seq)
    }

    /// Uploads segment `seq`: its extents stored on their own, at once, then it. Another writer's
    /// segment found in its place stops the writer for good: a later writer's, which fenced it
    /// ([`Error::Fenced`]), or an earlier one's, landed late past its fence ([`Error::Displaced`]).
    pub async fn upload(&self, seq: u64) -> Result<(), Error> {
        let Some((key, bytes, own)) = self.inner().pending.get(&seq).map(|pending| {
            (layout::segment(self.branch, seq), pending.bytes.clone(), pending.own.clone())
        }) else {
            return Ok(());
        };
        // Its extents of their own at once, then it.
        let creates = own.into_iter().map(|(key, bytes)| async move {
            // One uploaded before, by this writer, whose key names its epoch, is created: its bytes.
            match bucket::create(&*self.bucket, &key, bytes.to_vec()).await {
                Created::Created => Ok(()),
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
                let footer = recovery::footer(&key, &object.bytes, self.branch, seq)?;
                Err(self.stop(if footer.epoch > self.epoch {
                    Error::Fenced
                } else {
                    Error::Displaced
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
    use crate::batch::{Batch, Commit};
    use crate::store::Store;
    use object_store::memory::InMemory;

    fn memory() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), ""))
    }

    /// Adds a segment of `commit` alone to `writer`, closed on its chain: returns its number.
    fn add(writer: &Writer<Store>, commit: Commit) -> Result<u64, Error> {
        writer.add_segment(Batch::from(commit).close(&mut writer.chain()).frame())
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
            let footer = Chain::new(1, 5, 1, None, 0).close(Contents::default());
            let later = Segment { data: Vec::new(), footer };
            bucket::create(&*bucket, &layout::segment(1, 1), later.encode()).await;
            let opened = Writer::open(bucket.clone(), 1, 10, 0).await;
            assert_eq!(opened.err(), Some(Error::Fenced));
        });
    }

    /// A writer another took the branch from, whose fence takes its segment's place, stops, and
    /// says why; it takes no segment more.
    #[test]
    fn writers_taken_over_stop() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let first = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let _second = Writer::open(bucket.clone(), 1, 11, 0).await.unwrap();
            let seq = add(&first, Commit { tag: 1, ..Commit::default() }).unwrap();
            assert_eq!(first.upload(seq).await, Err(Error::Fenced));
            assert_eq!(first.stopped(), Some(Error::Fenced));
            assert_eq!(first.acknowledge().await, Err(Error::Fenced));
            assert_eq!(add(&first, Commit { tag: 2, ..Commit::default() }), Err(Error::Fenced));
        });
    }

    /// A writer whose next segment's place an earlier writer's segment took, landing late past its
    /// fence, stops for good, and says why; it takes no segment more.
    #[test]
    fn writers_displaced_by_an_earlier_writers_segment_stop() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let first = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let second = Writer::open(bucket.clone(), 1, 11, 0).await.unwrap();
            // The first's second segment in flight as the second took the branch, landing after
            // the second's fence, where its next goes.
            let mut late = first.chain();
            late.close(Contents::default());
            let footer = late.close(Contents::default());
            let seq = second.chain().next_seq();
            assert_eq!((footer.seq, footer.epoch), (seq, first.epoch()));
            let landed = Segment { data: Vec::new(), footer }.encode();
            bucket::create(&*bucket, &layout::segment(1, seq), landed).await;
            assert_eq!(add(&second, Commit { tag: 1, ..Commit::default() }), Ok(seq));
            assert_eq!(second.upload(seq).await, Err(Error::Displaced));
            assert_eq!(second.stopped(), Some(Error::Displaced));
            assert_eq!(second.acknowledge().await, Err(Error::Displaced));
            let refused = add(&second, Commit { tag: 2, ..Commit::default() });
            assert_eq!(refused, Err(Error::Displaced));
        });
    }

    /// An extent of its own a writer uploads again is its own; one that finds another's object
    /// at its key stops the writer.
    #[test]
    fn extents_of_their_own_found_uploaded_are_the_writers() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (again, other) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let again_key = layout::extent(writer.epoch(), again);
            let again_bytes = layout::with_footer(b"again", &());
            bucket::create(&*bucket, &again_key, again_bytes).await;
            let own = vec![(again, b"again".to_vec())];
            let seq = add(&writer, Commit { tag: 1, own, ..Commit::default() }).unwrap();
            assert_eq!(writer.upload(seq).await, Ok(()));
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));

            let other_key = layout::extent(writer.epoch(), other);
            bucket::create(&*bucket, &other_key, layout::with_footer(b"another", &())).await;
            let own = vec![(other, b"other".to_vec())];
            let seq = add(&writer, Commit { tag: 2, own, ..Commit::default() }).unwrap();
            let corrupt = Error::Corrupt(format!("{other_key} holds another extent"));
            assert_eq!(writer.upload(seq).await, Err(corrupt.clone()));
            assert_eq!(writer.stopped(), Some(corrupt));
        });
    }

    /// A checkpoint as of the last one is not written again.
    #[test]
    fn checkpoints_are_written_once() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let seq = writer.checkpoint().await.unwrap();
            let requests = bucket.stats().requests;
            assert_eq!(writer.checkpoint().await, Ok(seq));
            assert_eq!(bucket.stats().requests, requests);
        });
    }

    /// A writer takes a segment a store closed for it, and uploads it as it is; one numbered
    /// otherwise, of another writer, naming its extents of their own elsewhere, or without their
    /// objects, it refuses, and goes on as before.
    #[test]
    fn writers_upload_segments_stores_closed() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let chain = writer.chain();
            let (seq, after) = (chain.next_seq(), chain.after());
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
            let key = layout::extent(writer.epoch() + 1, large);
            let elsewhere = Location { key, offset: 0, len: 5 };
            for wrong in [
                Footer { seq: seq + 1, ..footer.clone() },
                Footer { epoch: writer.epoch() + 1, ..footer.clone() },
                Footer { after: Some((seq, 99)), ..footer.clone() },
                Footer { own: vec![(large, elsewhere.clone())], ..footer.clone() },
            ] {
                let objects = Objects { segment: segment(&wrong), own: owns() };
                assert!(writer.add_segment(objects).is_err(), "{wrong:?}");
            }
            let unowned = Objects { segment: segment(&footer), own: vec![] };
            let missing = Error::Corrupt(format!("{large:?} is stored at {own_key}"));
            assert_eq!(writer.add_segment(unowned), Err(missing));
            assert_eq!(writer.chain(), chain);
            let bytes = segment(&footer);
            let objects = Objects { segment: bytes.clone(), own: owns() };
            assert_eq!(writer.add_segment(objects), Ok(seq));
            let chain = writer.chain();
            assert_eq!((chain.next_seq(), chain.after()), (seq + 1, Some((seq, writer.epoch()))));
            writer.upload(seq).await.unwrap();
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));
            let object = bucket::get(&*bucket, &layout::segment(1, seq)).await.unwrap();
            assert_eq!(*object.bytes, *bytes);
            let object = bucket::get(&*bucket, &own_key).await.unwrap();
            assert_eq!(*object.bytes, *own);
            let state = writer.state();
            assert_eq!((state.last_commit, state.root), (Some(7), Some(b"root".to_vec())));
            assert_eq!(crate::read(&bucket, small, &state.extents[&small]).await.unwrap(), b"abc");
            assert_eq!(
                crate::read(&bucket, large, &state.extents[&large]).await.unwrap(),
                b"large"
            );
        });
    }
}
