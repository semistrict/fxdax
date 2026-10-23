// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's writer: its commits, in segments, uploaded in parallel and acknowledged in order.
//!
//! A writer takes the branch's lease, recovers its state, anew or on from what it found before
//! ([`Writer::open_recovered`]), and fences out the writer before it with an empty segment of its
//! own at the branch's end. Its segments are created where none is, numbered on from there, each
//! naming the one it follows; it acknowledges one once it and every one before it are created and
//! a read of the lease, after them, shows the lease still its. So a segment it acknowledges was in
//! the branch before any later writer, which took the lease after, looked for the branch's end;
//! and a segment of its that lands after a later writer's fence follows none of the branch's, and
//! is void. A writer that finds another's segment where its own goes, or the lease another's,
//! stops for good ([`Writer::stopped`]): fenced, or displaced by an earlier writer's segment landed
//! late ([`Error::Displaced`]). Only a writer taking the branch anew goes on with it.
//!
//! Its segments' footers are stamped by its chain ([`Writer::chain`]): those a local store closed,
//! framed, which it uploads as they are (a store's closed segment is the object, doc-7, decision
//! 1), and those of batches of commits it is given ([`Batch`](crate::Batch)) alike. It takes each
//! one way ([`Writer::add_segment`]), admitting it only if its chain would have stamped it so.
//!
//! The bytes of the objects it uploads are read only as their uploads go ([`Body`]), within a
//! budget it may share with other writers, a host's ([`Writer::uploading_within`]), and held only
//! while they go: what a host's writers hold at once is bounded, however far behind they are.

use crate::bucket::{self, Bucket, Created};
use crate::fetch::{Budget, Limits, Priority};
use crate::layout::{self, Chain, Contents, Footer, Location, Origin, Record, Segment};
use crate::moves::MovesMaps;
use crate::recovery::{self, Recovered, State};
use crate::{EPOCH_SHIFT, Error, branch, rules};
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::ExtentId;

/// How many of a segment's extents of their own upload at once.
const OWN_AT_ONCE: usize = 4;

/// The bytes of an object a writer uploads, read only as its upload goes, within the writer's
/// budget, and held only while it does.
#[derive(Clone)]
pub struct Body {
    /// About how many bytes it holds once read: what the budget counts.
    len: u64,
    read: Arc<dyn Fn() -> std::io::Result<Vec<u8>> + Send + Sync>,
}

impl Body {
    /// The bytes `read` reads, about `len` of them.
    pub fn new(
        len: u64,
        read: impl Fn() -> std::io::Result<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        Self { len, read: Arc::new(read) }
    }

    /// Its bytes.
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        (self.read)()
    }
}

impl From<Vec<u8>> for Body {
    /// `bytes`, held: each read copies them.
    fn from(bytes: Vec<u8>) -> Self {
        let bytes: Arc<[u8]> = bytes.into();
        Self::new(bytes.len() as u64, move || Ok(bytes.to_vec()))
    }
}

/// A segment's objects, as a writer uploads them ([`Writer::add_segment`]): its footer, which its
/// bytes end with; the segment's; and those of the extents its footer says are stored on their
/// own, by their keys, which go first.
pub struct Objects {
    pub footer: Footer,
    pub segment: Body,
    pub own: Vec<(String, Body)>,
}

pub struct Writer<B: Bucket> {
    bucket: Arc<B>,
    branch: u64,
    epoch: u64,
    /// What its uploads may hold at once, with those of the writers it shares it with.
    budget: Arc<Budget>,
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
    footer: Footer,
    /// Its object, and those of its extents stored on their own, uploaded first, until they are
    /// uploaded.
    objects: Option<(Body, Vec<(String, Body)>)>,
}

/// The chain of the writer of lease epoch `epoch` whose next segment is `seq`, after `state`: its
/// next segment follows the state's last, and names the branch's next sequence number as the state
/// has it, or `floor`, at least.
fn chain_after(state: &State, epoch: u64, seq: u64, floor: u64) -> Chain {
    Chain::new(state.branch, epoch, seq, state.last_segment, state.next_sequence.max(floor))
}

impl<B: Bucket> Writer<B> {
    /// Takes `branch` over: its lease, as `holder`, which must differ from every other taking's, of
    /// the placement whose fencing token is `token` ([`branch::take_lease`]); then its state,
    /// recovered once it holds the lease.
    pub async fn open(bucket: Arc<B>, branch: u64, holder: u64, token: u64) -> Result<Self, Error> {
        let record = branch::record(&*bucket, branch).await?.ok_or(Error::NoBranch)?;
        Self::take(bucket, branch, record, None, holder, token).await
    }

    /// Takes over the branch `before` recovered, whose record is `record`, as [`Writer::open`]
    /// does, having found both before it takes the lease: its state, once it holds it, is
    /// `before`'s recovered on ([`recovery::recover_on`]), which reads only what came since.
    pub async fn open_recovered(
        bucket: Arc<B>,
        record: Record,
        before: Recovered,
        holder: u64,
        token: u64,
    ) -> Result<Self, Error> {
        let branch = before.state.branch;
        Self::take(bucket, branch, record, Some(before), holder, token).await
    }

    /// Takes `branch`, whose record is `record`, over, as [`Writer::open`] says: its state
    /// recovered once it holds the lease, anew, or on from `before`, a recovery of it made before.
    async fn take(
        bucket: Arc<B>,
        branch: u64,
        record: Record,
        before: Option<Recovered>,
        holder: u64,
        token: u64,
    ) -> Result<Self, Error> {
        if record.deleted {
            return Err(Error::Deleted);
        }
        let epoch = branch::take_lease(&*bucket, &layout::lease(branch), holder, token).await?;
        // Deleted meanwhile, and its lease removed: the collector removes the lease taken here.
        if branch::record(&*bucket, branch).await?.is_none_or(|record| record.deleted) {
            return Err(Error::Deleted);
        }
        let recovered = match before {
            None => recovery::recover(&*bucket, branch, record.origin).await?,
            Some(before) => recovery::recover_on(&*bucket, record.origin, before).await?,
        };
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
            budget: Budget::new(Limits::UNBOUNDED),
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

    /// Uploads within `budget`, which it shares with other writers, a host's: each object read only
    /// once the budget has room for it, and held only while it goes. Alone, a writer's uploads are
    /// bounded only by how many at once it is asked for.
    pub fn uploading_within(self, budget: Arc<Budget>) -> Self {
        Self { budget, ..self }
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

    /// The tag of the branch's last commit, as of the last segment acknowledged.
    pub fn last_commit(&self) -> Option<u64> {
        self.inner().state.last_commit
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
        let Objects { footer, segment, own } = objects;
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
        inner.pending.insert(seq, Pending { footer, objects: Some((segment, own)) });
        Ok(seq)
    }

    /// Uploads segment `seq`: its extents stored on their own, a few at once, then it, its bytes
    /// ending with the footer it was taken with. Another writer's segment found in its place stops
    /// the writer for good: a later writer's, which fenced it ([`Error::Fenced`]), or an earlier
    /// one's, landed late past its fence ([`Error::Displaced`]).
    pub async fn upload(&self, seq: u64) -> Result<(), Error> {
        let Some((footer, segment, own)) = self.inner().pending.get(&seq).and_then(|pending| {
            let (segment, own) = pending.objects.clone()?;
            Some((pending.footer.clone(), segment, own))
        }) else {
            return Ok(());
        };
        // Its extents of their own, a few at once, then it.
        let creates = futures::stream::iter(own).map(|(key, body)| async move {
            // One uploaded before, by this writer, whose key names its epoch, is created: its bytes.
            match self.create(&key, &body, |_| Ok(())).await? {
                Created::Created => Ok(()),
                Created::Exists(_) => Err(Error::Corrupt(format!("{key} holds another extent"))),
            }
        });
        if let Err(error) = creates.buffer_unordered(OWN_AT_ONCE).try_collect::<()>().await {
            return Err(self.stop(error));
        }
        let key = layout::segment(self.branch, seq);
        let taken = |bytes: &[u8]| match layout::footer::<Footer>(bytes)? == footer {
            true => Ok(()),
            false => Err(Error::Corrupt(format!("{key} ends with another footer than taken"))),
        };
        match self.create(&key, &segment, taken).await {
            Err(error) => Err(self.stop(error)),
            Ok(Created::Created) => {
                // Its objects, uploaded, go.
                if let Some(pending) = self.inner().pending.get_mut(&seq) {
                    pending.objects = None;
                }
                Ok(())
            }
            Ok(Created::Exists(object)) => {
                let footer = recovery::footer(&key, &object.bytes, self.branch, seq)?;
                Err(self.stop(if footer.epoch > self.epoch {
                    Error::Fenced
                } else {
                    Error::Displaced
                }))
            }
        }
    }

    /// Creates the object at `key` of `body`'s bytes, unless an object is there
    /// ([`bucket::create`]), once `check` passes them: read once the writer's budget has room for
    /// them, and held only until the create is done.
    async fn create(
        &self,
        key: &str,
        body: &Body,
        check: impl FnOnce(&[u8]) -> Result<(), Error>,
    ) -> Result<Created, Error> {
        let _grant = self.budget.enter(Priority::Demand, body.len).1.await;
        let bytes = body.read().map_err(|err| Error::Read(format!("{key}: {err}")))?;
        check(&bytes)?;
        Ok(bucket::create(&*self.bucket, key, bytes).await)
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
                .take_while(|(_, pending)| pending.objects.is_none())
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
        maps.read(&*self.bucket, state.extents.values().map(|location| location.key.clone()))
            .await?;
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
    use crate::store::{Request, Store};
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
            let again_key = layout::extent(writer.epoch(), writer.chain().next_seq(), again);
            let again_bytes = layout::with_footer(b"again", &());
            bucket::create(&*bucket, &again_key, again_bytes).await;
            let own = vec![(again, b"again".to_vec())];
            let seq = add(&writer, Commit { tag: 1, own, ..Commit::default() }).unwrap();
            assert_eq!(writer.upload(seq).await, Ok(()));
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));

            let other_key = layout::extent(writer.epoch(), writer.chain().next_seq(), other);
            bucket::create(&*bucket, &other_key, layout::with_footer(b"another", &())).await;
            let own = vec![(other, b"other".to_vec())];
            let seq = add(&writer, Commit { tag: 2, own, ..Commit::default() }).unwrap();
            let corrupt = Error::Corrupt(format!("{other_key} holds another extent"));
            assert_eq!(writer.upload(seq).await, Err(corrupt.clone()));
            assert_eq!(writer.stopped(), Some(corrupt));
        });
    }

    /// A checkpoint reads the moves of its segments that have them, which a listing finds: read
    /// one at a time, each segment's, at 25 ms of latency, a branch of a few minutes' pgbench held
    /// the uploader up for tens of seconds (TASK-91.2).
    #[test]
    fn checkpoints_read_only_the_moves_there_are() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            for n in 0..64 {
                let extents = vec![(ExtentId::new(1, first + n), b"segment".to_vec())];
                let seq =
                    add(&writer, Commit { tag: n + 1, extents, ..Commit::default() }).unwrap();
                writer.upload(seq).await.unwrap();
                writer.acknowledge().await.unwrap();
            }
            let before = bucket.stats();
            writer.checkpoint().await.unwrap();
            let after = bucket.stats();
            let made = |request| after.requests(request) - before.requests(request);
            assert_eq!(
                [Request::List, Request::Get, Request::GetRange].map(made),
                [1, 0, 0],
                "the checkpoint's listings, reads and ranged reads"
            );
        });
    }

    /// TASK-79.19: a segment's extents of their own upload a few at once, each holding its bytes
    /// while it goes, rather than all at once: the next goes as one is done.
    #[test]
    fn extents_of_their_own_upload_a_few_at_once() {
        use crate::sim::bucket::{Fault, SimBucket};
        use std::task::{Context, Poll};
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        let first = writer.chain().next_sequence();
        let own = (0..2 * OWN_AT_ONCE as u64 + 1)
            .map(|n| (ExtentId::new(1, first + n), vec![n as u8; 10]))
            .collect();
        let commit = Commit { tag: 1, own, ..Commit::default() };
        let seq = writer.add_segment(Batch::from(commit).close(&mut writer.chain()).frame());
        let mut upload = std::pin::pin!(writer.upload(seq.unwrap()));
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        // The most creates waiting at once.
        let mut most = 0;
        while let Poll::Pending = upload.as_mut().poll(&mut context) {
            most = most.max(bucket.waiting());
            bucket.complete(0, Fault::None);
        }
        assert_eq!(most, OWN_AT_ONCE);
    }

    /// TASK-79.19: a writer uploading within a budget reads each object only once the budget has
    /// room for it, and gives the room up once the object is created: with room for one, the next
    /// is read only as the one before is done.
    #[test]
    fn uploads_read_their_objects_within_their_budget() {
        use crate::sim::bucket::{Fault, SimBucket};
        use std::task::{Context, Poll};
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        let writer = writer.uploading_within(Budget::new(Limits { requests: 1, bytes: u64::MAX }));
        // Two segments, a commit's each, whose reads are noted.
        let reads = Arc::new(Mutex::new(Vec::new()));
        let mut seqs = Vec::new();
        for tag in 1..=2 {
            let commit = Commit { tag, ..Commit::default() };
            let Objects { footer, segment, own } =
                Batch::from(commit).close(&mut writer.chain()).frame();
            let reads = reads.clone();
            let segment = Body::new(10, move || {
                reads.lock().unwrap().push(tag);
                segment.read()
            });
            seqs.push(writer.add_segment(Objects { footer, segment, own }).unwrap());
        }
        let uploads = futures::future::join_all(seqs.iter().map(|&seq| writer.upload(seq)));
        let mut uploads = std::pin::pin!(uploads);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let mut uploading = || match uploads.as_mut().poll(&mut context) {
            Poll::Ready(uploaded) => Some(uploaded),
            Poll::Pending => None,
        };
        assert!(uploading().is_none());
        assert_eq!((reads.lock().unwrap().clone(), bucket.waiting()), (vec![1], 1));
        bucket.complete(0, Fault::None);
        assert!(uploading().is_none());
        assert_eq!((reads.lock().unwrap().clone(), bucket.waiting()), (vec![1, 2], 1));
        bucket.complete(0, Fault::None);
        assert_eq!(uploading(), Some(vec![Ok(()), Ok(())]));
    }

    /// A segment taken with a footer its bytes do not end with stops its writer, which uploads
    /// nothing of it.
    #[test]
    fn segments_ending_with_another_footer_than_taken_stop_their_writers() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let close = |tag| {
                let commit = Commit { tag, ..Commit::default() };
                Batch::from(commit).close(&mut writer.chain()).frame()
            };
            let Objects { footer, own, .. } = close(1);
            let objects = Objects { footer, segment: close(2).segment, own };
            let seq = writer.add_segment(objects).unwrap();
            let key = layout::segment(1, seq);
            let corrupt = Error::Corrupt(format!("{key} ends with another footer than taken"));
            assert_eq!(writer.upload(seq).await, Err(corrupt.clone()));
            assert_eq!(writer.stopped(), Some(corrupt));
            assert_eq!(bucket.head(&key).await.map(|_| ()), Err(bucket::Error::NotFound));
        });
    }

    /// TASK-92.6: a writer taking over a branch recovered before it took the lease recovers it on
    /// from there: it reads the lease, takes it, reads the record, then only the segments that came
    /// in between, as its state has them, and fences the branch after them.
    #[test]
    fn writers_open_on_what_was_recovered_before_their_lease() {
        use crate::sim::bucket::SimBucket;
        let bucket = SimBucket::new();
        let (first, _) = bucket.served(async {
            branch::create(&*bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        let commit = |tag| {
            let objects =
                Batch::from(Commit { tag, ..Commit::default() }).close(&mut first.chain()).frame();
            let seq = first.add_segment(objects).unwrap();
            bucket.served(first.upload(seq)).0.unwrap();
            bucket.served(first.acknowledge()).0.unwrap();
        };
        commit(1);
        let record = bucket.served(branch::record(&*bucket, 1)).0.unwrap().unwrap();
        let before = bucket.served(recovery::recover(&*bucket, 1, None)).0.unwrap();
        assert_eq!(before.end, 2);
        // Committed in between.
        commit(2);
        let opening = Writer::open_recovered(bucket.clone(), record, before, 11, 0);
        let (second, rounds) = bucket.served_in_rounds(opening);
        let second = second.unwrap();
        assert_eq!(second.last_commit(), Some(2));
        let tail = |seq| format!("get {} last {}", layout::segment(1, seq), crate::object::TAIL);
        let lease = layout::lease(1);
        assert_eq!(
            rounds,
            [
                vec![format!("get {lease}")],
                vec![format!("put {lease}")],
                vec![format!("get {}", layout::record(1))],
                vec![tail(2)],
                vec![tail(3), tail(4)],
                vec![format!("list {}", layout::checkpoints(1))],
                vec![format!("list {}", layout::segments(1))],
                vec![format!("put {}", layout::segment(1, 3))],
                vec![format!("get {lease}")],
            ]
        );
        let anew = bucket.served(recovery::recover(&*bucket, 1, None)).0.unwrap();
        assert_eq!(second.state(), anew.state);
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
            let own_key = layout::extent(writer.epoch(), seq, large);
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
            let segment = |footer: &Footer| -> Vec<u8> {
                Segment { data: b"abc".to_vec(), footer: footer.clone() }.encode()
            };
            let own = layout::with_footer(b"large", &());
            let owns = || vec![(own_key.clone(), own.clone().into())];
            let objects = |footer: &Footer, own| Objects {
                footer: footer.clone(),
                segment: segment(footer).into(),
                own,
            };
            let key = layout::extent(writer.epoch() + 1, seq, large);
            let elsewhere = Location { key, offset: 0, len: 5 };
            // Its chain's next place: segment 1 of epoch 1, after its fence, 0.
            assert_eq!((writer.epoch(), seq, after), (1, 1, Some((0, 1))));
            let wanted = "(1, 1, 1, Some((0, 1)))";
            let misplaced = |place: &str| {
                Error::Corrupt(format!("a segment of branch 1, {place}, not {wanted}"))
            };
            for (wrong, refused) in [
                (
                    Footer { seq: seq + 1, ..footer.clone() },
                    misplaced("number 2, epoch 1, after Some((0, 1))"),
                ),
                (
                    Footer { epoch: writer.epoch() + 1, ..footer.clone() },
                    misplaced("number 1, epoch 2, after Some((0, 1))"),
                ),
                (
                    Footer { after: Some((seq, 99)), ..footer.clone() },
                    misplaced("number 1, epoch 1, after Some((1, 99))"),
                ),
                (
                    Footer { own: vec![(large, elsewhere.clone())], ..footer.clone() },
                    Error::Corrupt(format!("{large:?} is stored at {}", elsewhere.key)),
                ),
            ] {
                assert_eq!(writer.add_segment(objects(&wrong, owns())), Err(refused), "{wrong:?}");
            }
            let missing = Error::Corrupt(format!("{large:?} is stored at {own_key}"));
            assert_eq!(writer.add_segment(objects(&footer, vec![])), Err(missing));
            assert_eq!(writer.chain(), chain);
            assert_eq!(writer.add_segment(objects(&footer, owns())), Ok(seq));
            let chain = writer.chain();
            assert_eq!((chain.next_seq(), chain.after()), (seq + 1, Some((seq, writer.epoch()))));
            writer.upload(seq).await.unwrap();
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));
            let object = bucket::get(&*bucket, &layout::segment(1, seq)).await.unwrap();
            assert_eq!(object.bytes, segment(&footer));
            let object = bucket::get(&*bucket, &own_key).await.unwrap();
            assert_eq!(object.bytes, own);
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
