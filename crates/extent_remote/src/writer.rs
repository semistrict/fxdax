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
use crate::layout::{self, Branch, Chain, Child, Footer, Location, Origin};
use crate::moves::MovesMaps;
use crate::record::Record;
use crate::recovery::{self, Recovered, State};
use crate::wal::Fenced;
use crate::{EPOCH_SHIFT, Error, branch, rules, trace};
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::ExtentId;

/// How many of a segment's extents of their own upload at once, within the writer's budget, which
/// bounds what they hold: a guest's sparse extents make thousands of little objects a commit, and
/// four at once, 60 a second at 15 ms a PUT, held commits up for minutes as they reached the bound
/// on their lag (GCE r353, pgbench's checkpoint at shutdown: TASK-142).
const OWN_AT_ONCE: usize = 32;

/// How many forks' children's records are read, or made, at once: each a round trip to object
/// storage, which one at a time held a checkpoint, and the uploader with it, up for a round trip
/// a fork, 300 forks of a base 6-9 s at 25 ms of latency (TASK-92.4).
const RECORDS_AT_ONCE: usize = 32;

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

/// What a writer tells of the extents its checkpoints record as moved, from where to where, as it
/// names them where they went ([`Writer::following`]).
pub type Mover = Box<dyn Fn(&[(ExtentId, Location, Location)]) + Send + Sync>;

pub struct Writer<B: Bucket> {
    bucket: Arc<B>,
    branch: Branch,
    epoch: u64,
    /// Who took its lease: unique to its taking, which the lease must still be.
    holder: u64,
    /// What its uploads may hold at once, with those of the writers it shares it with.
    budget: Arc<Budget>,
    /// Where it found the branch's end, taking it over.
    fenced: Fenced,
    inner: Mutex<Inner>,
    /// What follows the collector's moves with it, if anything does ([`Writer::following`]).
    mover: std::sync::OnceLock<Mover>,
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
    recorded: BTreeSet<Branch>,
    /// The children of those forks whose IDs another branch has: their forks made no branch.
    refused: BTreeSet<Branch>,
    /// The child of a fork whose record a checkpoint read last, looking for those deleted, which
    /// the next goes on after (TASK-92.4).
    looked: Option<Branch>,
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
    pub async fn open(
        bucket: Arc<B>,
        branch: Branch,
        holder: u64,
        token: u64,
    ) -> Result<Self, Error> {
        let record = branch::record(&*bucket, branch).await?.ok_or(Error::NoBranch)?;
        Self::take(bucket, branch, record, (None, None), holder, token).await
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
        Self::take(bucket, branch, record, (Some(before), None), holder, token).await
    }

    /// Takes over the branch `before` recovered as [`Writer::open_recovered`] does, its lease taken
    /// from `lease`, what a read of it found as its record was read ([`branch::take_lease_from`]):
    /// a round trip less on a real bucket (TASK-165.11).
    pub async fn open_recovered_from(
        bucket: Arc<B>,
        record: Record,
        (before, lease): (Recovered, branch::LeaseRead),
        holder: u64,
        token: u64,
    ) -> Result<Self, Error> {
        let branch = before.state.branch;
        Self::take(bucket, branch, record, (Some(before), Some(lease)), holder, token).await
    }

    /// Takes `branch`, whose record is `record`, over, as [`Writer::open`] says: its state
    /// recovered once it holds the lease, anew, or on from `before`, a recovery of it made before;
    /// the lease taken from `lease`, a read of it before, if given.
    async fn take(
        bucket: Arc<B>,
        branch: Branch,
        record: Record,
        (before, lease): (Option<Recovered>, Option<branch::LeaseRead>),
        holder: u64,
        token: u64,
    ) -> Result<Self, Error> {
        // Before the lease: a point has none, and nothing writes it, whoever asks.
        record.writable()?;
        let key = layout::lease(branch);
        let epoch = match lease {
            Some(read) => branch::take_lease_from(&*bucket, &key, read, holder, token).await?,
            None => branch::take_lease(&*bucket, &key, holder, token).await?,
        };
        // Once the lease is taken, three reads, none of which waits for another, at once: each a
        // round trip or more on a real bucket, which every opening of a store pays (TASK-165.11).
        // The record: deleted meanwhile, its lease removed, as the collector removes the lease
        // taken here, the branch is gone, whatever its recovery found. The branch, recovered. And
        // the segments there are, past its end too (below): read alongside the recovery, not
        // after it, they miss no more than a listing after it does, as a segment may land at any
        // time after either, which the fence and the links between segments answer for.
        let recovering = async {
            match before {
                None => recovery::recover(&*bucket, branch, record.origin()).await,
                Some(before) => recovery::recover_on(&*bucket, record.origin(), before).await,
            }
        };
        let segments = layout::segments(branch);
        let (found, recovered, listed) = futures::join!(
            branch::record(&*bucket, branch),
            recovering,
            bucket::list(&*bucket, &segments),
        );
        if found?.is_none_or(|record| record.deleted) {
            return Err(Error::Deleted);
        }
        let recovered = recovered?;
        let mut state = recovered.state;
        let recovered_commit = state.last_commit;
        let mut seq = recovered.end;
        // Its fence: an empty segment of its at the branch's end, so that no earlier writer's
        // segment, landing late, changes the state it starts from; one that landed there first is
        // the branch's, and the fence goes after it. Segments past the end, earlier writers' landed
        // past a gap, are void, and empty segments fill the gaps between them, so that this
        // writer's follow without one.
        let last = listed
            .iter()
            .filter_map(|meta| match layout::kind(&meta.key) {
                layout::Kind::Segment { seq, .. } => Some(seq),
                _ => None,
            })
            .max();
        let mut fenced = !rules().fence;
        while !fenced || last.is_some_and(|last| seq <= last) {
            match recovery::fence(&*bucket, &mut state, epoch, seq).await? {
                None => fenced = true,
                Some(footer) if footer.epoch > epoch => return Err(Error::Fenced),
                Some(_) => {}
            }
            seq += 1;
        }
        let mut floor = 0;
        if rules().epoch_sequences {
            // Still its lease, no later writer's segment, minted from a later range, is in what
            // it recovered.
            if !branch::holds_lease(&*bucket, &layout::lease(branch), epoch, holder).await? {
                return Err(Error::Fenced);
            }
            floor = epoch << EPOCH_SHIFT;
        }
        let chain = chain_after(&state, epoch, seq, floor);
        let fenced = Fenced { recovered: recovered_commit, last_commit: state.last_commit, chain };
        let writer = Self {
            bucket,
            branch,
            epoch,
            holder,
            budget: Budget::new(Limits::UNBOUNDED),
            fenced,
            inner: Mutex::new(Inner {
                state,
                chain,
                pending: BTreeMap::new(),
                checkpoint: recovered.checkpoint,
                recorded: BTreeSet::new(),
                refused: BTreeSet::new(),
                looked: None,
                stopped: None,
            }),
            mover: std::sync::OnceLock::new(),
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

    /// Has `mover` told of the extents the writer's checkpoints record as moved, from where to
    /// where, as the writer names them where they went, before each checkpoint lands: a host's
    /// store reading them, which so names none further back along its way than the writer's newest
    /// checkpoint, from which the collector keeps the moves (TASK-195). Once.
    pub fn following(
        &self,
        mover: impl Fn(&[(ExtentId, Location, Location)]) + Send + Sync + 'static,
    ) {
        let _ = self.mover.set(Box::new(mover));
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn branch(&self) -> Branch {
        self.branch
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Where it found the branch's end, taking it over: what a host's write-ahead log is put after
    /// ([`Wal::rebase`](crate::Wal::rebase)), before the writer takes a segment.
    pub fn fenced(&self) -> Fenced {
        self.fenced
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
        for (extent, location) in footer.own() {
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
    /// one's, landed late past its fence ([`Error::Displaced`]). fxdax: in a span of its own,
    /// `upload.segment`, its bucket's requests children of it (`crate::trace`, TASK-258).
    pub async fn upload(&self, seq: u64) -> Result<(), Error> {
        let span = trace::Span::start("upload.segment");
        span.set_branch(self.branch);
        span.set_count(trace::SEGMENT, seq);
        let uploaded = span.around(self.upload_segment(seq, &span)).await;
        span.result(&uploaded);
        uploaded
    }

    /// Uploads segment `seq`, as [`Writer::upload`] says, saying its extents and bytes on `span`.
    async fn upload_segment(&self, seq: u64, span: &trace::Span) -> Result<(), Error> {
        let Some((footer, segment, own)) = self.inner().pending.get(&seq).and_then(|pending| {
            let (segment, own) = pending.objects.clone()?;
            Some((pending.footer.clone(), segment, own))
        }) else {
            return Ok(());
        };
        span.set_count(trace::EXTENTS, own.len() as u64);
        let bytes = segment.len + own.iter().map(|(_, body)| body.len).sum::<u64>();
        span.set_count(trace::BYTES, bytes);
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
        failpoint!("writer::upload");
        match self.create(&key, &segment, taken).await {
            Err(error) => Err(self.stop(error)),
            Ok(Created::Created) => {
                failpoint!("writer::uploaded");
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

    /// Acknowledges the segments uploaded with every one before them: their creates are the
    /// fencing, as a writer taken over finds its create failing at its successor's fence, which the
    /// collector keeps (doc-7, Fencing; TASK-371). Those holding a fork, once the lease shows them
    /// the branch's too: a deleted branch's fences go. Returns the last segment acknowledged.
    /// fxdax: to the last that goes on with no commit, a commit spanning segments acknowledged with
    /// the segment it ends in, not part of it (`extent_frame::segment::Continued`, TASK-92.41):
    /// what a store lets go of, once acknowledged, the bucket has whole, and the state
    /// acknowledged, which checkpoints hold, holds no commit begun.
    pub async fn acknowledge(&self) -> Result<Option<u64>, Error> {
        let (through, forks) = {
            let inner = self.inner();
            if let Some(error) = &inner.stopped {
                return Err(error.clone());
            }
            let created: Vec<_> = (inner.pending.iter())
                .take_while(|(_, pending)| pending.objects.is_none())
                .collect();
            let Some(end) = created
                .iter()
                .rposition(|(_, pending)| pending.footer.contents.continued.is_none())
            else {
                return Ok(inner.state.through);
            };
            let acknowledged = &created[..=end];
            let forks =
                acknowledged.iter().any(|(_, pending)| !pending.footer.contents.forks.is_empty());
            (*acknowledged[end].0, forks)
        };
        let lease = layout::lease(self.branch);
        if forks
            && rules().forks_after_lease
            && !branch::holds_lease(&*self.bucket, &lease, self.epoch, self.holder).await?
        {
            return Err(self.stop(Error::Fenced));
        }
        failpoint!("writer::acknowledge");
        {
            let mut inner = self.inner();
            let inner = &mut *inner;
            while let Some(entry) = inner.pending.first_entry() {
                if *entry.key() > through {
                    break;
                }
                inner.state.apply(&entry.remove().footer);
            }
        }
        failpoint!("writer::acknowledged");
        self.record_forks().await?;
        Ok(self.inner().state.through)
    }

    /// Whether another branch has the ID of `child`, a fork's child, acknowledged: the fork made no
    /// branch.
    pub fn refused(&self, child: Branch) -> bool {
        self.inner().refused.contains(&child)
    }

    /// Gives the children of acknowledged forks their records: but one whose ID another branch took
    /// while its fork was made, which is that branch's, and this one's segments go on sound; its
    /// writers would fail on it for ever otherwise, taking the branch or not (TASK-79.82).
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
        futures::stream::iter(forks)
            .map(|fork| async move {
                let origin = Origin { parent: self.branch, at: fork.at };
                let child = Child { branch: fork.child, owner: fork.owner, point: fork.point };
                match branch::record_fork(&*self.bucket, child, origin).await {
                    Ok(()) => {}
                    Err(Error::IdTaken(child)) => drop(self.inner().refused.insert(child)),
                    Err(error) => return Err(error),
                }
                self.inner().recorded.insert(fork.child);
                Ok::<_, Error>(())
            })
            .buffer_unordered(RECORDS_AT_ONCE)
            .try_collect()
            .await
    }

    /// Writes a checkpoint as of the last segment acknowledged, with the extents the collector
    /// moved where they went: where the writer names them from then on, before the checkpoint
    /// lands, so that it never names one before where a checkpoint of its own does, which the
    /// collector, seeing the writer hold the lease, keeps the moves from, not from before (TASK-175).
    /// fxdax: in a span of its own, `upload.checkpoint`, its bucket's requests children of it
    /// (`crate::trace`, TASK-258).
    pub async fn checkpoint(&self) -> Result<Option<u64>, Error> {
        let span = trace::Span::start("upload.checkpoint");
        span.set_branch(self.branch);
        let checkpointed = span.around(self.write_checkpoint(&span)).await;
        span.result(&checkpointed);
        checkpointed
    }

    /// Writes a checkpoint, as [`Writer::checkpoint`] says, saying what it is of on `span`.
    async fn write_checkpoint(&self, span: &trace::Span) -> Result<Option<u64>, Error> {
        let mut state = {
            let inner = self.inner();
            if inner.state.through.is_none() || inner.state.through <= inner.checkpoint {
                return Ok(inner.checkpoint);
            }
            // fxdax: acknowledged, its last segment ends its commits (TASK-92.41).
            debug_assert!(inner.state.begun.is_none(), "a checkpoint of a commit begun");
            inner.state.clone()
        };
        let seq = state.through.unwrap();
        span.set_count(trace::SEGMENT, seq);
        span.set_count(trace::EXTENTS, state.extents.len() as u64);
        // Forks whose children were deleted. Their records stay; their forks need not. Of a few
        // forks' children each checkpoint, in turn, after those the last looked at: every child's,
        // each checkpoint, made a storm of forks, a checkpoint each, read their records N^2 / 2
        // times (TASK-92.4). A child deleted has its fork go within as many checkpoints as there
        // are forks to read RECORDS_AT_ONCE at a time.
        let mut children: Vec<Branch> = state.forks.iter().map(|fork| fork.child).collect();
        children.sort_unstable();
        children.dedup();
        let after = children.partition_point(|&child| Some(child) <= self.inner().looked);
        let children: Vec<Branch> = children[after..]
            .iter()
            .chain(&children[..after])
            .take(RECORDS_AT_ONCE)
            .copied()
            .collect();
        let looked = children.last().copied();
        let looking = futures::stream::iter(children)
            .map(|child| async move {
                let record = branch::record(&*self.bucket, child).await?;
                Ok::<_, Error>(record.is_some_and(|record| record.deleted).then_some(child))
            })
            .buffer_unordered(RECORDS_AT_ONCE)
            .try_filter_map(|gone| async move { Ok(gone) })
            .try_collect::<Vec<Branch>>();
        // The moves of the objects its extents are in, followed, alongside those records' reads:
        // neither waits for the other, a round trip less for each checkpoint, and so each fork,
        // on a real bucket (TASK-165.11).
        let following = async {
            let mut maps = MovesMaps::default();
            let keys = state.extents.values().map(|location| location.key.clone());
            maps.read(&*self.bucket, keys).await?;
            let mut moved = Vec::new();
            for (extent, old) in &state.extents {
                let location = maps.follow(&*self.bucket, *extent, old).await?;
                if location != *old {
                    moved.push((*extent, old.clone(), location));
                }
            }
            Ok::<_, Error>(moved)
        };
        let (moved, gone) = futures::join!(following, looking);
        let (moved, gone) = (moved?, gone?);
        for (extent, _, location) in &moved {
            state.extents.insert(*extent, location.clone());
        }
        if rules().moves_taken_first {
            self.take_moves(&moved);
        }
        state.forks.retain(|fork| !gone.contains(&fork.child));
        failpoint!("writer::checkpoint");
        let key = layout::checkpoint(self.branch, seq);
        // Another writer's of the same segment holds the same extents, perhaps elsewhere.
        bucket::create(&*self.bucket, &key, crate::checkpoint::encode(&state)).await;
        failpoint!("writer::checkpointed");
        if !rules().moves_taken_first {
            self.take_moves(&moved);
        }
        let mut inner = self.inner();
        inner.state.forks.retain(|fork| !gone.contains(&fork.child));
        inner.looked = looked.or(inner.looked);
        inner.checkpoint = inner.checkpoint.max(Some(seq));
        Ok(inner.checkpoint)
    }

    /// Names the extents `moved` says the collector moved, from where to where, where they went,
    /// if it names them on their way there, before: only forward along their ways, which another
    /// checkpoint's moves may have taken further, its own since.
    fn take_moves(&self, moved: &[(ExtentId, Location, Location)]) {
        let mut inner = self.inner();
        for (extent, _, to) in moved {
            if let Some(at) = inner.state.extents.get_mut(extent)
                && layout::further(&at.key, &to.key)
            {
                *at = to.clone();
            }
        }
        drop(inner);
        // And what follows them with it.
        if rules().stores_follow_moves
            && let Some(mover) = self.mover.get()
        {
            mover(moved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::layout::test_branch;
    use crate::layout::{Contents, Segment, child};
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            for _ in 0..2 {
                branch::take_lease(&*bucket, &layout::lease(test_branch(1)), 9, 0).await.unwrap();
            }
            let footer = Chain::new(test_branch(1), 5, 1, None, 0).close(Contents::default());
            let later = Segment { data: Vec::new(), footer };
            bucket::create(&*bucket, &layout::segment(test_branch(1), 1), later.encode()).await;
            let opened = Writer::open(bucket.clone(), test_branch(1), 10, 0).await;
            assert_eq!(opened.err(), Some(Error::Fenced));
        });
    }

    /// A writer another took the branch from, whose fence takes its segment's place, stops, and
    /// says why; it takes no segment more.
    #[test]
    fn writers_taken_over_stop() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let first = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let _second = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let first = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let second = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
            // The first's second segment in flight as the second took the branch, landing after
            // the second's fence, where its next goes.
            let mut late = first.chain();
            late.close(Contents::default());
            let footer = late.close(Contents::default());
            let seq = second.chain().next_seq();
            assert_eq!((footer.seq, footer.epoch), (seq, first.epoch()));
            let landed = Segment { data: Vec::new(), footer }.encode();
            bucket::create(&*bucket, &layout::segment(test_branch(1), seq), landed).await;
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (again, other) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let again_key =
                layout::extent(layout::TEST_CELL, writer.epoch(), writer.chain().next_seq(), again);
            let again_bytes = layout::with_footer(b"again", &());
            bucket::create(&*bucket, &again_key, again_bytes).await;
            let own = vec![(again.sequence, b"again".to_vec())];
            let seq = add(&writer, Commit { tag: 1, own, ..Commit::default() }).unwrap();
            assert_eq!(writer.upload(seq).await, Ok(()));
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));

            let other_key =
                layout::extent(layout::TEST_CELL, writer.epoch(), writer.chain().next_seq(), other);
            bucket::create(&*bucket, &other_key, layout::with_footer(b"another", &())).await;
            let own = vec![(other.sequence, b"other".to_vec())];
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
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
        use crate::testing::{Fault, SimBucket};
        use std::task::{Context, Poll};
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        let first = writer.chain().next_sequence();
        let own = (0..2 * OWN_AT_ONCE as u64 + 1).map(|n| (first + n, vec![n as u8; 10])).collect();
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

    /// TASK-92.4: a fork's children's records are made, as their forks are acknowledged, and read,
    /// as a checkpoint looks for those deleted, a few at once: one at a time, 300 forks of a base
    /// held the uploader up for a round trip each.
    #[test]
    fn forks_records_are_made_and_read_a_few_at_once() {
        use crate::layout::Segment;
        use crate::testing::{Fault, SimBucket};
        use std::task::{Context, Poll};
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        let children =
            (2..2 + 2 * RECORDS_AT_ONCE as u64 + 1).map(|id| child(test_branch(id))).collect();
        let footer = writer.chain().close(Contents { forks: children, ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() };
        let seq = writer.add_segment(Objects {
            footer,
            segment: segment.encode().into(),
            own: Vec::new(),
        });
        bucket.served(writer.upload(seq.unwrap())).0.unwrap();
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        // The most requests waiting at once, until `future` is done.
        let mut most_waiting = |future: std::pin::Pin<&mut dyn Future<Output = _>>| {
            let mut future = future;
            let mut most = 0;
            while let Poll::Pending = future.as_mut().poll(&mut context) {
                most = most.max(bucket.waiting());
                bucket.complete(0, Fault::None);
            }
            most
        };
        let acknowledged = most_waiting(std::pin::pin!(async {
            writer.acknowledge().await.unwrap();
        }));
        assert_eq!(acknowledged, RECORDS_AT_ONCE, "the records made at once");
        let checkpointed = most_waiting(std::pin::pin!(async {
            writer.checkpoint().await.unwrap();
        }));
        assert_eq!(checkpointed, RECORDS_AT_ONCE, "the records read at once");
    }

    /// TASK-92.4: a storm of forks, each acknowledged and checkpointed as the uploader does, its
    /// children's records made as each is: a checkpoint reads the records of a few of the forks'
    /// children at most, looking for those deleted, taking them in turn, not every child's each
    /// time, which made a storm of N forks read N^2/2 records; and the forks of a child deleted go
    /// within as many checkpoints as there are forks to read a few at a time.
    #[test]
    fn checkpoints_read_a_few_forks_records_at_most() {
        const FORKS: u64 = 4 * RECORDS_AT_ONCE as u64;
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let mut most = 0;
            let mut first = None;
            for n in 0..FORKS {
                let batch = Batch::from(Commit { tag: n + 1, ..Commit::default() });
                let forked = batch.fork(child(test_branch(2 + n)), &mut writer.chain()).frame();
                let seq = writer.add_segment(forked).unwrap();
                first.get_or_insert(seq);
                writer.upload(seq).await.unwrap();
                writer.acknowledge().await.unwrap();
                let before = bucket.stats().requests(Request::Get);
                writer.checkpoint().await.unwrap();
                most = most.max(bucket.stats().requests(Request::Get) - before);
            }
            assert_eq!(most, RECORDS_AT_ONCE as u64, "the most records a checkpoint read");
            // The first child deleted: its fork goes within FORKS / RECORDS_AT_ONCE checkpoints.
            let origin = Origin { parent: test_branch(1), at: first.unwrap() };
            let forked = crate::record::Kind::Fork(origin);
            branch::delete_fork(
                &*bucket,
                test_branch(2),
                forked,
                child(test_branch(2)).owner,
                9,
                0,
            )
            .await
            .unwrap();
            let forks = |writer: &Writer<Store>| writer.inner().state.forks.len() as u64;
            for n in 0..FORKS / RECORDS_AT_ONCE as u64 {
                let seq = add(&writer, Commit { tag: FORKS + n + 1, ..Commit::default() }).unwrap();
                writer.upload(seq).await.unwrap();
                writer.acknowledge().await.unwrap();
                writer.checkpoint().await.unwrap();
            }
            assert_eq!(forks(&writer), FORKS - 1);
        });
    }

    /// TASK-79.19: a writer uploading within a budget reads each object only once the budget has
    /// room for it, and gives the room up once the object is created: with room for one, the next
    /// is read only as the one before is done.
    #[test]
    fn uploads_read_their_objects_within_their_budget() {
        use crate::testing::{Fault, SimBucket};
        use std::task::{Context, Poll};
        let bucket = SimBucket::new();
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let close = |tag| {
                let commit = Commit { tag, ..Commit::default() };
                Batch::from(commit).close(&mut writer.chain()).frame()
            };
            let Objects { footer, own, .. } = close(1);
            let objects = Objects { footer, segment: close(2).segment, own };
            let seq = writer.add_segment(objects).unwrap();
            let key = layout::segment(test_branch(1), seq);
            let corrupt = Error::Corrupt(format!("{key} ends with another footer than taken"));
            assert_eq!(writer.upload(seq).await, Err(corrupt.clone()));
            assert_eq!(writer.stopped(), Some(corrupt));
            assert_eq!(bucket.head(&key).await.map(|_| ()), Err(bucket::Error::NotFound));
        });
    }

    /// TASK-92.6: a writer taking over a branch recovered before it took the lease recovers it on
    /// from there: it reads the lease, takes it, reads the record, then only the segments that came
    /// in between, as its state has them, and fences the branch after them. TASK-165.11: once the
    /// lease is taken, the record, the recovery and the listing of the segments go at once, each
    /// round a round trip on a real bucket.
    #[test]
    fn writers_open_on_what_was_recovered_before_their_lease() {
        use crate::testing::SimBucket;
        let bucket = SimBucket::new();
        let (first, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        let commit = |tag| {
            let objects =
                Batch::from(Commit { tag, ..Commit::default() }).close(&mut first.chain()).frame();
            let seq = first.add_segment(objects).unwrap();
            bucket.served(first.upload(seq)).0.unwrap();
            bucket.served(first.acknowledge()).0.unwrap();
        };
        commit(1);
        let record = bucket.served(branch::record(&*bucket, test_branch(1))).0.unwrap().unwrap();
        let before = bucket.served(recovery::recover(&*bucket, test_branch(1), None)).0.unwrap();
        assert_eq!(before.end, 2);
        // Committed in between.
        commit(2);
        let opening = Writer::open_recovered(bucket.clone(), record, before, 11, 0);
        let (second, rounds) = bucket.served_in_rounds(opening);
        let second = second.unwrap();
        assert_eq!(second.last_commit(), Some(2));
        let tail = |seq| {
            format!("get {} last {}", layout::segment(test_branch(1), seq), crate::object::TAIL)
        };
        let lease = layout::lease(test_branch(1));
        let record_key = layout::record(test_branch(1));
        assert_eq!(
            rounds,
            [
                vec![format!("get {lease}")],
                vec![format!("put {lease}")],
                vec![
                    format!("get {record_key}"),
                    tail(2),
                    format!("list {}", layout::segments(test_branch(1)))
                ],
                vec![tail(3), tail(4)],
                vec![format!("list {}", layout::checkpoints(test_branch(1)))],
                vec![format!("put {}", layout::segment(test_branch(1), 3))],
                vec![format!("get {lease}")],
            ]
        );
        let anew = bucket.served(recovery::recover(&*bucket, test_branch(1), None)).0.unwrap();
        assert_eq!(second.state(), anew.state);
    }

    /// TASK-165.11: a writer taking over a branch from a read of its lease made before, as its
    /// record was read, takes the lease from that read: its first round the lease's update, a round
    /// trip less; and from a read gone stale, another writer having taken the lease since, by
    /// reading it again once its update finds it changed, at the epoch after that writer's.
    #[test]
    fn writers_open_on_a_read_of_their_lease_made_before() {
        use crate::testing::SimBucket;
        let bucket = SimBucket::new();
        let (first, _) = bucket.served(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        assert_eq!(first.epoch(), 1);
        let lease = layout::lease(test_branch(1));
        let found = || {
            let record =
                bucket.served(branch::record(&*bucket, test_branch(1))).0.unwrap().unwrap();
            let before =
                bucket.served(recovery::recover(&*bucket, test_branch(1), None)).0.unwrap();
            let read = bucket.served(branch::read_lease(&*bucket, &lease)).0;
            (record, before, read)
        };
        let (record, before, read) = found();
        let opening = Writer::open_recovered_from(bucket.clone(), record, (before, read), 11, 0);
        let (second, rounds) = bucket.served_in_rounds(opening);
        let second = second.unwrap();
        assert_eq!(second.epoch(), 2);
        assert_eq!(rounds[0], [format!("put {lease}")]);
        // A read of the lease, then another writer's taking of it, then the opening.
        let (record, before, read) = found();
        let (third, _) = bucket.served(Writer::open(bucket.clone(), test_branch(1), 12, 0));
        assert_eq!(third.unwrap().epoch(), 3);
        let opening = Writer::open_recovered_from(bucket.clone(), record, (before, read), 13, 0);
        let (fourth, rounds) = bucket.served_in_rounds(opening);
        assert_eq!(fourth.unwrap().epoch(), 4);
        // Its update refused, its answer read back, as an update whose answer was lost is told
        // apart (`bucket::update`); then the lease, read anew, taken.
        let (put, get) = (vec![format!("put {lease}")], vec![format!("get {lease}")]);
        assert_eq!(rounds[..4], [put.clone(), get.clone(), get, put]);
    }

    /// A checkpoint as of the last one is not written again.
    #[test]
    fn checkpoints_are_written_once() {
        futures::executor::block_on(async {
            let bucket = memory();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let seq = writer.checkpoint().await.unwrap();
            let requests = bucket.stats().requests;
            assert_eq!(writer.checkpoint().await, Ok(seq));
            assert_eq!(bucket.stats().requests, requests);
        });
    }

    /// A writer takes a segment a store closed for it, and uploads it as it is; one numbered
    /// otherwise, of another writer, or without the objects of its extents of their own at the
    /// keys it names them at, it refuses, and goes on as before.
    #[test]
    fn writers_upload_segments_stores_closed() {
        futures::executor::block_on(async {
            let bucket = Arc::new(Store::new(Arc::new(InMemory::new()), ""));
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let chain = writer.chain();
            let (seq, after) = (chain.next_seq(), chain.after());
            let (small, large) = (ExtentId::new(1, 2), ExtentId::new(1, 3));
            let own_key = layout::extent(layout::TEST_CELL, writer.epoch(), seq, large);
            let footer = Footer {
                branch: test_branch(1),
                seq,
                epoch: writer.epoch(),
                after,
                contents: Contents {
                    commits: vec![7],
                    extents: vec![(small, 0, 3)],
                    own: vec![(large.sequence, 5)],
                    frees: vec![],
                    root: Some(b"root".to_vec()),
                    next_sequence: 4,
                    forks: vec![],
                    capacity: None,
                    continued: None,
                },
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
            let elsewhere = layout::extent(layout::TEST_CELL, writer.epoch() + 1, seq, large);
            // Its chain's next place: segment 1 of epoch 1, after its fence, 0.
            assert_eq!((writer.epoch(), seq, after), (1, 1, Some((0, 1))));
            let wanted = format!("{:?}", (test_branch(1), 1, 1, Some((0, 1))));
            let misplaced = |place: &str| {
                Error::Corrupt(format!("a segment of branch 1/1/1, {place}, not {wanted}"))
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
            ] {
                assert_eq!(writer.add_segment(objects(&wrong, owns())), Err(refused), "{wrong:?}");
            }
            let elsewhere = vec![(elsewhere, own.clone().into())];
            let missing = Error::Corrupt(format!("{large:?} is stored at {own_key}"));
            assert_eq!(writer.add_segment(objects(&footer, elsewhere)), Err(missing));
            let missing = Error::Corrupt(format!("{large:?} is stored at {own_key}"));
            assert_eq!(writer.add_segment(objects(&footer, vec![])), Err(missing));
            assert_eq!(writer.chain(), chain);
            assert_eq!(writer.add_segment(objects(&footer, owns())), Ok(seq));
            let chain = writer.chain();
            assert_eq!((chain.next_seq(), chain.after()), (seq + 1, Some((seq, writer.epoch()))));
            writer.upload(seq).await.unwrap();
            assert_eq!(writer.acknowledge().await, Ok(Some(seq)));
            let object =
                bucket::get(&*bucket, &layout::segment(test_branch(1), seq)).await.unwrap();
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
