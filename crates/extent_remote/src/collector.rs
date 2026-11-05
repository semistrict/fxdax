// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The collector (`fxdax-gc`): the only thing that deletes.
//!
//! A pass takes the collector's lease, lists every object, and only then reads what branches
//! need. An object written after the listing is no candidate, which is what makes reading the
//! branches' states after it safe: what a state names was either listed, and is seen as needed, or
//! not, and is left.
//!
//! The live branches are those whose records are not deleted, and the children of forks in live
//! branches' states, which get their records only once their forks are acknowledged. What they
//! need is what recovering them reads, the objects holding the extents their states name, the moves
//! the way to those goes through, and each one's segments past its end, which may be uploaded and
//! not yet in it. Segments go only once a checkpoint covers them, so that a reader that finds one
//! missing knows, from the checkpoints, whether it is the branch's end.
//!
//! A fork of a point is its child's record alone (doc-7, Points): its child names what the point
//! names, and a pass that read the point deleted lists the records after that, and so finds every
//! child whose fork can succeed ([`branch::fork_point`]). A child still being made of a point
//! deleted the pass gives up on, first ([`abandon`]): nothing relies on it, and it may name what is
//! gone, which would stop this pass and every other.
//!
//! A state read names no fork deleted before it, a writer's checkpoint dropping those, and the
//! fork's record and its children's may have come after the pass listed the records. So it lists
//! them again after it read the states, and recovers what it finds, again while a state it read
//! may have dropped a fork: while it recovers a branch with a checkpoint of its own. A record made
//! after that names what the pass kept, or what it never listed: what a fork after a state read
//! inherited, that state holds, or a later object; and a fork before it that state names.
//!
//! Objects mostly freed are compacted: their live extents copied to their moves, one object,
//! written whole, before the object goes, a grace period later. Writers record moves in their next
//! checkpoints, after which the moves go too, unless a state's way to its extents still goes through
//! them.
//!
//! A branch's writer names its extents where the checkpoint it recovered from, or its own since,
//! said, which the state a pass reads may name further along their moves: a checkpoint another
//! writer made, landing after the writer recovered the branch (one it took the branch from, which
//! knew nothing of it), or the state of the parent a branch inherited them from, which its writer
//! read as it was then. So a state's way to an extent starts where the extent was sealed, its
//! moves all kept, unless the state's checkpoint is that of the writer holding the branch's lease,
//! read after the checkpoint was listed: a writer taking the branch after that recovers from that
//! checkpoint or a later one, and the writer that made it took its moves first (TASK-175).
//!
//! A pass reads records, recovers branches and deletes objects many at once; what forks inherited
//! together, their parent's state where they were forked, it reads once and takes once for them
//! all; and a collector reads a record again only once it changed ([`Collector`], TASK-92.7).

use crate::bucket::{self, Bucket, Meta};
use crate::layout::{self, Kind, Location, Moves, Origin};
use crate::moves::{Moved, MovesMaps};
use crate::record::Record;
use crate::recovery::{self, Checkpoints, Layered, Layers, Recovered, State};
use crate::{Error, branch, rules};
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_device::ExtentId;

/// How many branches' records a pass reads at once.
const RECORDS_AT_ONCE: usize = 32;

/// How many branches a pass recovers at once.
const RECOVERIES_AT_ONCE: usize = 16;

/// How many of the checkpoints a pass read whole it keeps for the recoveries after: those of forks
/// of a base, recovered one after another, which start from the base's.
const CHECKPOINTS_KEPT: usize = 4;

/// How many objects a pass deletes at once.
const DELETES_AT_ONCE: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Objects whose live extents take less than this fraction of their data are compacted.
    pub compact_below: f64,
    /// How long, by the store's clock, a compacted object stays after its moves are written, and
    /// moves after nothing names the object they are of: what spares a reader that looked an
    /// extent up just before a round trip.
    pub grace: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub deleted: usize,
    pub compacted: usize,
}

/// One pass of the collector, as `holder`, which must differ from every other pass's: of a
/// collector of its own, which remembers nothing of it ([`Collector`]).
pub async fn collect(
    bucket: &impl Bucket,
    holder: u64,
    options: &Options,
) -> Result<Report, Error> {
    Collector::new(*options).collect(bucket, holder).await
}

/// The collector, from one pass to the next: it remembers the branches' records it read, with the
/// versions it read them at, and reads one again only once it changed: few do, and those of
/// deleted forks, which stay, never do (TASK-92.7).
pub struct Collector {
    options: Options,
    /// The records read, by their keys, with the versions they were read at.
    records: BTreeMap<String, (String, Record)>,
}

impl Collector {
    pub fn new(options: Options) -> Self {
        Self { options, records: BTreeMap::new() }
    }

    /// One pass, as `holder`, which must differ from every other pass's.
    pub async fn collect(&mut self, bucket: &impl Bucket, holder: u64) -> Result<Report, Error> {
        let epoch = branch::take_lease(bucket, layout::GC_LEASE, holder, 0).await?;
        let early = rules().candidates_first;
        let mut objects = if early { listing(bucket).await } else { BTreeMap::new() };
        let mut records = self.records(bucket).await?;
        let mut live = Live::default();
        loop {
            if rules().collector_abandons {
                abandon(bucket, &mut records).await?;
            }
            let checkpointed = recover_live(bucket, &records, &mut live).await?;
            if !checkpointed || !rules().records_relisted {
                break;
            }
            let again = self.records(bucket).await?;
            let found = again.keys().any(|branch| !records.contains_key(branch));
            records.extend(again);
            if !found {
                break;
            }
        }
        if !early {
            objects = listing(bucket).await;
        }
        let pass = Pass::new(bucket, objects, records, live).await?;
        let mut report = Report::default();
        pass.compact(bucket, &self.options, &mut report).await?;
        let doomed = pass.doomed(bucket, &self.options).await?;
        // A pass a later one took the lease from stops.
        if branch::lease_epoch(bucket, layout::GC_LEASE).await? != epoch {
            return Ok(report);
        }
        // Many at once, in any order: none of it is what a live branch needs, whatever else goes.
        let deletes = futures::stream::iter(&doomed).map(|key| bucket::delete(bucket, key));
        report.deleted += deletes.buffer_unordered(DELETES_AT_ONCE).count().await;
        Ok(report)
    }

    /// The branches' records, read many at once, but those read before at the versions listed. A
    /// deleted branch's children got theirs before it was marked deleted, so listing them again
    /// after reading them finds those of any it read deleted.
    async fn records(&mut self, bucket: &impl Bucket) -> Result<BTreeMap<u64, Record>, Error> {
        let mut records = BTreeMap::new();
        loop {
            let listed =
                bucket::list(bucket, layout::BRANCHES).await.into_iter().filter_map(|meta| {
                    match layout::kind(&meta.key) {
                        Kind::Record { branch } if !records.contains_key(&branch) => {
                            Some((branch, meta))
                        }
                        _ => None,
                    }
                });
            let known = &self.records;
            let reads = futures::stream::iter(listed)
                .map(|(branch, meta)| async move {
                    (branch, read_record(bucket, known, branch, &meta).await, meta.key)
                })
                .buffered(RECORDS_AT_ONCE)
                .collect::<Vec<_>>()
                .await;
            let mut new = false;
            for (branch, read, key) in reads {
                let Some((version, record)) = read? else { continue };
                match version {
                    Some(version) => self.records.insert(key, (version, record)),
                    None => self.records.remove(&key),
                };
                records.insert(branch, record);
                new = true;
            }
            if !new {
                // Those gone, which the collector deleted, it forgets.
                self.records.retain(|key, _| match layout::kind(key) {
                    Kind::Record { branch } => records.contains_key(&branch),
                    _ => false,
                });
                return Ok(records);
            }
        }
    }
}

/// Gives up on the forks being made of points that were deleted, as `records` say, which it says
/// are deleted then, or made, if that came first ([`branch::abandon`]).
async fn abandon(bucket: &impl Bucket, records: &mut BTreeMap<u64, Record>) -> Result<(), Error> {
    let gone = |point: u64| records.get(&point).is_none_or(|record| record.deleted);
    let abandoned: Vec<u64> = (records.iter())
        .filter(|(_, record)| !record.deleted && record.forking().is_some_and(gone))
        .map(|(&child, _)| child)
        .collect();
    for child in abandoned {
        records.insert(child, branch::abandon(bucket, child).await?);
    }
    Ok(())
}

/// The record of `branch` at the key of `meta`, which a listing gave, if there is one, and the
/// version read: as `known` has it, if it read it at the version listed; or read now, unless it is
/// of another format ([`Error::Format`]), which stops the pass: what that branch needs, it can't
/// read.
async fn read_record(
    bucket: &impl Bucket,
    known: &BTreeMap<String, (String, Record)>,
    branch: u64,
    meta: &Meta,
) -> Result<Option<(Option<String>, Record)>, Error> {
    if let Some(listed) = &meta.version.e_tag
        && let Some((version, record)) = known.get(&meta.key)
        && version == listed
    {
        return Ok(Some((Some(version.clone()), *record)));
    }
    let Some(object) = bucket::get(bucket, &meta.key).await else { return Ok(None) };
    Ok(Some((object.meta.version.e_tag, Record::decode(branch, &object.bytes)?)))
}

async fn listing(bucket: &impl Bucket) -> BTreeMap<String, Meta> {
    bucket::list(bucket, "").await.into_iter().map(|meta| (meta.key.clone(), meta)).collect()
}

/// Recovers the live branches `live` has not yet, several at once, folding them into it: those
/// whose records are not deleted, and the children of forks in their states that have no records
/// yet. Returns whether one has a checkpoint of its own, which a fork deleted may have gone from.
/// Forks of a branch at one segment are recovered together, those without checkpoints of their own
/// starting from the branch's state there, read once for them ([`Checkpoints`]); and the states are
/// folded into what the pass needs of them as they are recovered, what forks inherited together
/// once for them all, rather than each fork's copy of it ([`Layers`]).
async fn recover_live(
    bucket: &impl Bucket,
    records: &BTreeMap<u64, Record>,
    live: &mut Live,
) -> Result<bool, Error> {
    let checkpoints = Checkpoints::new(bucket, CHECKPOINTS_KEPT);
    let mut checkpointed = false;
    let origin = |origin: Option<Origin>| origin.map(|origin| (origin.parent, origin.at));
    // Forks made at the same segment next to each other, in the same rounds.
    let mut queue: BTreeSet<(Option<(u64, u64)>, u64)> = records
        .iter()
        .filter(|(branch, record)| !record.deleted && !live.taken.contains(branch))
        .map(|(&branch, record)| (origin(record.origin()), branch))
        .collect();
    while !queue.is_empty() {
        // Each once, queued by its record, or by its parent's state, or both.
        let round: Vec<_> = std::iter::from_fn(|| queue.pop_first())
            .filter(|&(_, branch)| live.taken.insert(branch))
            .take(RECOVERIES_AT_ONCE)
            .collect();
        let recoveries = round.into_iter().map(|(origin, branch)| {
            let origin = origin.map(|(parent, at)| Origin { parent, at });
            let checkpoints = &checkpoints;
            async move {
                let recovered = match recovery::recover_layers(checkpoints, branch, origin).await {
                    Ok(recovered) => match behind(bucket, branch, &recovered).await {
                        Ok(behind) => Ok((recovered, behind)),
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                };
                (branch, recovered)
            }
        });
        let mut inherited = Inherited::default();
        for (branch, recovered) in futures::future::join_all(recoveries).await {
            let (recovered, behind) = match recovered {
                Ok(recovered) => recovered,
                // Deleted since its record was read, what it needed may be gone: it is not live.
                // Nor is a failed fork's claim no writer has: nothing ever read what it names.
                Err(Error::Lost(lost)) => match branch::record(bucket, branch).await? {
                    Some(record) if record.deleted => continue,
                    Some(record) if branch::lost_is_ok(bucket, branch, &record).await? => {
                        branch::abandon_claim(bucket, branch).await?;
                        continue;
                    }
                    Some(_) => return Err(Error::Lost(lost)),
                    None => continue,
                },
                Err(error) => return Err(error),
            };
            // A child with a record is queued by it, as it says: one whose ID another branch took
            // as the fork was made has another origin than its parent's state says (TASK-79.82).
            for fork in &recovered.state.forks {
                let recorded = records.contains_key(&fork.child);
                if recorded || !rules().forks_from_states || live.taken.contains(&fork.child) {
                    continue;
                }
                queue.insert((Some((branch, fork.at)), fork.child));
            }
            checkpointed |= recovered.checkpoint.is_some();
            live.add(recovered, behind, &mut inherited);
        }
        live.inherit(inherited);
    }
    Ok(checkpointed)
}

/// Whether the writer of `branch` may name the extents `recovered` holds anywhere on their ways
/// from where they were sealed, before the moves its state followed: unless its state is from a
/// checkpoint of the writer holding its lease, read now, after the checkpoint was listed. The
/// extents it inherited, its writer may name so whatever this says ([`Live::inherit`]).
async fn behind(
    bucket: &impl Bucket,
    branch: u64,
    recovered: &Recovered<Layers>,
) -> Result<bool, Error> {
    if !rules().ways_from_seals {
        return Ok(false);
    }
    let Some(checkpointer) = recovered.checkpointer else { return Ok(true) };
    Ok(branch::lease_epoch(bucket, &layout::lease(branch)).await? != checkpointer)
}

/// What a pass needs of the live branches' states, folded in as they are recovered.
#[derive(Default)]
struct Live {
    /// Those recovered, or being: each once.
    taken: BTreeSet<u64>,
    branches: BTreeMap<u64, Ended>,
    /// What recovering them read.
    touched: BTreeSet<String>,
    /// Where the extents they name are: each place once, however many of them name it there.
    located: BTreeMap<ExtentId, BTreeSet<Location>>,
    /// The moves they name extents in whose writers may name them anywhere on their ways from
    /// where they were sealed ([`behind`]).
    behind: BTreeSet<String>,
}

/// What deciding what goes needs of a live branch's state, its extents but: where its segments end,
/// the highest epoch of those applied, and the branch's next sequence number.
struct Ended {
    end: u64,
    epoch: u64,
    next_sequence: u64,
}

/// The states forks inherited, as their recoveries read them, each with the extents of it every
/// fork inheriting it holds no longer where it inherited it: what no fork of a round recovered
/// needs of it.
#[derive(Default)]
struct Inherited {
    states: Vec<(Arc<State>, BTreeSet<ExtentId>)>,
}

impl Live {
    /// Folds in `recovered`, a live branch's, whose writer may be `behind` it: its own layer, and
    /// what it inherited into `inherited`, its round's, which [`Live::inherit`] folds in once for
    /// all its forks.
    fn add(&mut self, recovered: Recovered<Layers>, behind: bool, inherited: &mut Inherited) {
        let Recovered { state, end, touched, .. } = recovered;
        let State { branch, extents, epoch, next_sequence, .. } = state;
        let Layered { inherited: from, own, shadowed } = extents;
        self.touched.extend(touched);
        for (extent, location) in own {
            if behind {
                self.behind_at(&location);
            }
            self.located.entry(extent).or_default().insert(location);
        }
        if let Some(from) = from {
            match inherited.states.iter_mut().find(|(state, _)| Arc::ptr_eq(state, &from)) {
                Some((_, unheld)) => unheld.retain(|extent| shadowed.contains(extent)),
                None => inherited.states.push((from, shadowed)),
            }
        }
        self.branches.insert(branch, Ended { end, epoch, next_sequence });
    }

    /// Folds in what forks inherited, `inherited`: each state once, but the extents of it none of
    /// them holds where they inherited them. Their writers may name those anywhere on their ways
    /// from where they were sealed: each recovered the parent's state as it was then.
    fn inherit(&mut self, inherited: Inherited) {
        let behind = rules().ways_from_seals;
        for (state, unheld) in inherited.states {
            for (extent, location) in &state.extents {
                if unheld.contains(extent) {
                    continue;
                }
                if behind {
                    self.behind_at(location);
                }
                let located = self.located.entry(*extent).or_default();
                if !located.contains(location) {
                    located.insert(location.clone());
                }
            }
        }
    }

    /// Notes that a writer may name the extent at `location` anywhere on its way there from where
    /// it was sealed: in moves, if it is in some.
    fn behind_at(&mut self, location: &Location) {
        if matches!(layout::kind(&location.key), Kind::Moves { .. })
            && !self.behind.contains(&location.key)
        {
            self.behind.insert(location.key.clone());
        }
    }
}

/// What a pass found: every object listed, and what the live branches need of them.
struct Pass {
    objects: BTreeMap<String, Meta>,
    records: BTreeMap<u64, Record>,
    branches: BTreeMap<u64, Ended>,
    /// What recovering the live branches read.
    touched: BTreeSet<String>,
    /// The objects holding the extents the live branches name, and those extents in them.
    named: BTreeMap<String, BTreeMap<ExtentId, Location>>,
    /// The moves the way to those extents goes through, whose maps are needed even once their
    /// data moved on: from where the states name them, or, where a writer may name them before
    /// that, from where they were sealed.
    hops: BTreeSet<String>,
}

impl Pass {
    async fn new(
        bucket: &impl Bucket,
        objects: BTreeMap<String, Meta>,
        records: BTreeMap<u64, Record>,
        live: Live,
    ) -> Result<Self, Error> {
        let Live { branches, touched, located, behind, .. } = live;
        let mut named: BTreeMap<String, BTreeMap<ExtentId, Location>> = BTreeMap::new();
        // The moves on the ways there from where the extents were sealed, those named among them:
        // each moves is at the key of the moves of what it is of.
        let mut hops = BTreeSet::new();
        for key in &behind {
            let mut key = key.as_str();
            while let Kind::Moves { from } = layout::kind(key) {
                if objects.contains_key(key) {
                    hops.insert(key.to_owned());
                }
                key = from;
            }
        }
        let mut maps = MovesMaps::default();
        // Each place once, however many branches name it.
        for (&extent, locations) in &located {
            for location in locations {
                let mut location = location.clone();
                loop {
                    if objects.contains_key(&location.key) {
                        named
                            .entry(location.key.clone())
                            .or_default()
                            .insert(extent, location.clone());
                    }
                    let moves = layout::moves(&location.key);
                    if !objects.contains_key(&moves) {
                        break;
                    }
                    hops.insert(moves);
                    match maps.moved(bucket, extent, &location.key).await? {
                        Moved::To(to) => location = to,
                        Moved::No | Moved::Dropped => break,
                    }
                }
            }
        }
        Ok(Self { objects, records, branches, touched, named, hops })
    }

    /// fxdax: whether the object of `kind`, which no live state names, may yet be named, by what
    /// its key declares (TASK-153.7): a segment, at or past the end of its branch, live, which may
    /// land in it; an extent stored on its own, its key naming the writer that uploaded it before
    /// the segment that names it, if that segment may land in the branch, live, as no later writer's
    /// is in it yet, and none is at its place. Nothing else may.
    async fn may_land(&self, bucket: &impl Bucket, kind: Kind<'_>) -> Result<bool, Error> {
        let (branch, seq, writer) = match kind {
            Kind::Segment { branch, seq } => (branch, seq, None),
            Kind::Extent { branch, epoch, seq, sequence } => (branch, seq, Some((epoch, sequence))),
            _ => return Ok(false),
        };
        let Some(ended) = self.branches.get(&branch) else { return Ok(false) };
        let Some((epoch, sequence)) = writer else { return Ok(seq >= ended.end) };
        // The epoch of the writer whose segments land in the branch: of the last one in it.
        let current = if rules().orphans_after_successor {
            ended.epoch
        } else {
            branch::lease_epoch(bucket, &layout::lease(branch)).await?
        };
        let ahead = match rules().extent_keys_name_segments {
            true => seq >= ended.end,
            false => sequence >= ended.next_sequence,
        };
        Ok(epoch >= current && (epoch != ended.epoch || ahead))
    }

    /// Compacts the objects mostly freed: copies the live extents of each to its moves.
    async fn compact(
        &self,
        bucket: &impl Bucket,
        options: &Options,
        report: &mut Report,
    ) -> Result<(), Error> {
        for (key, extents) in &self.named {
            let compactable = match layout::kind(key) {
                kind @ Kind::Segment { .. } => {
                    !self.touched.contains(key) && !self.may_land(bucket, kind).await?
                }
                Kind::Moves { .. } => true,
                _ => false,
            };
            let moves = layout::moves(key);
            let used: u64 = extents.values().map(|location| location.len).sum();
            let below = |size: usize| (used as f64) < options.compact_below * size as f64;
            // Its size, which counts its footer, first, then its data's.
            if !compactable
                || self.objects.contains_key(&moves)
                || !below(self.objects[key].size as usize)
            {
                continue;
            }
            let Some(object) = bucket::get(bucket, key).await else { continue };
            // Its data, checked: compacting copies none that is corrupt under new CRCs.
            let object_data = layout::data(&object.bytes)?;
            if !below(object_data.len()) {
                continue;
            }
            let mut data = Vec::new();
            let mut entries = Vec::new();
            for (&extent, location) in extents {
                let range = location.offset as usize..(location.offset + location.len) as usize;
                let bytes = object_data
                    .get(range)
                    .ok_or_else(|| Error::Corrupt(format!("{extent:?} past the end of {key}")))?;
                let to =
                    Location { key: moves.clone(), offset: data.len() as u64, len: location.len };
                entries.push((extent, to));
                data.extend_from_slice(bytes);
            }
            // Another pass's moves of it, if there are some, stand.
            let bytes = layout::with_footer(&data, &Moves { extents: entries });
            bucket::create(bucket, &moves, bytes).await;
            report.compacted += 1;
        }
        Ok(())
    }

    /// What goes: everything listed that the live branches do not need.
    async fn doomed(&self, bucket: &impl Bucket, options: &Options) -> Result<Vec<String>, Error> {
        let now = bucket.now();
        let aged = |meta: &Meta| meta.modified.saturating_add(options.grace) <= now;
        let moved_long_ago = |key: &str| self.objects.get(&layout::moves(key)).is_some_and(aged);
        let mut doomed = Vec::new();
        for (key, meta) in &self.objects {
            let named = self.named.contains_key(key);
            let kind = layout::kind(key);
            let goes = match kind {
                Kind::Checkpoint { .. } => !self.touched.contains(key),
                // Once a checkpoint covers it, if its extents are dead or moved.
                Kind::Segment { .. } => {
                    !self.may_land(bucket, kind).await?
                        && !self.touched.contains(key)
                        && (!named || moved_long_ago(key))
                }
                // Once no segment will name it.
                Kind::Extent { .. } => !named && !self.may_land(bucket, kind).await?,
                // Moves go once no state's way goes through them, and they are compacted in turn,
                // or what they are of is gone and they hold no extent a state names.
                Kind::Moves { from } => {
                    !self.hops.contains(key)
                        && if named {
                            moved_long_ago(key)
                        } else {
                            !self.objects.contains_key(from) && aged(meta)
                        }
                }
                // A deleted branch's record goes once no branch forked from it needs it, unless it
                // is a fork's. A lease goes with its branch: writers do not take a deleted one.
                Kind::Record { branch } => {
                    self.records.get(&branch).is_some_and(Record::may_drop)
                        && !self.touched.contains(key)
                }
                Kind::Lease { branch } => {
                    !self.records.get(&branch).is_some_and(|record| !record.deleted)
                }
                Kind::GcLease | Kind::Unknown => false,
            };
            if goes {
                doomed.push(key.clone());
            }
        }
        Ok(doomed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::bucket::{ByteRange, Created, Error as BucketError, Object, PutMode, Version};
    use crate::layout::{Child, child};
    use crate::sim::bucket::{Direct, SimBucket};
    use crate::store::Store;
    use crate::writer::Writer;
    use futures::FutureExt as _;
    use futures::executor::block_on;
    use object_store::memory::InMemory;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// No compaction, and compacted objects kept long.
    const KEEP: Options = Options { compact_below: 0.0, grace: 3_600_000 };

    fn store() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), ""))
    }

    /// `commit` in a segment of its own, uploaded and acknowledged.
    async fn commit<B: Bucket>(writer: &Writer<B>, commit: Commit) {
        let objects = Batch::from(commit).close(&mut writer.chain()).frame();
        let seq = writer.add_segment(objects).unwrap();
        writer.upload(seq).await.unwrap();
        writer.acknowledge().await.unwrap();
    }

    /// The keys of every object but the collector's lease.
    async fn keys(bucket: &impl Bucket) -> Vec<String> {
        listing(bucket).await.into_keys().filter(|key| key != layout::GC_LEASE).collect()
    }

    /// The extents a writer stored on their own and freed go; one uploaded ahead of the segment
    /// that will name it stays, as do those its state names, and its record.
    #[test]
    fn extents_minted_and_freed_go_and_those_ahead_of_their_segments_stay() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (freed, kept) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let own = vec![(freed.sequence, b"freed".to_vec()), (kept.sequence, b"kept".to_vec())];
            commit(&writer, Commit { tag: 1, own, ..Commit::default() }).await;
            let gone = writer.location(freed).unwrap().key;
            commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
            let (seq, next) = (writer.chain().next_seq(), writer.chain().next_sequence());
            let ahead = layout::extent(writer.epoch(), seq, ExtentId::new(1, next));
            let bytes = layout::with_footer(b"ahead", &());
            assert!(matches!(bucket::create(&*bucket, &ahead, bytes).await, Created::Created));

            let before = keys(&*bucket).await;
            let report = collect(&*bucket, 1, &KEEP).await.unwrap();
            assert_eq!(report, Report { deleted: 1, compacted: 0 });
            let left: Vec<_> = before.into_iter().filter(|key| *key != gone).collect();
            assert_eq!(keys(&*bucket).await, left);
            assert!(left.contains(&ahead) && left.contains(&layout::record(1)));
        });
    }

    /// TASK-141.2: a pass finding a branch of a newer format stops, deleting nothing, though what
    /// the other branches freed would go: what that branch needs, it can't read.
    #[test]
    fn passes_stop_at_branches_of_newer_formats() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let freed = ExtentId::new(1, writer.chain().next_sequence());
            let own = vec![(freed.sequence, b"freed".to_vec())];
            commit(&writer, Commit { tag: 1, own, ..Commit::default() }).await;
            commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
            let mut newer = (layout::FORMAT + 1).to_le_bytes().to_vec();
            newer.extend_from_slice(b"what a later format holds");
            let created = bucket::create(&*bucket, &layout::record(2), newer).await;
            assert!(matches!(created, Created::Created));

            let before = keys(&*bucket).await;
            let refused = collect(&*bucket, 1, &KEEP).await;
            assert_eq!(refused, Err(Error::Format { branch: 2, format: layout::FORMAT + 1 }));
            assert_eq!(keys(&*bucket).await, before);
        });
    }

    /// An extent stored on its own, uploaded ahead of the segment that names it, stays, though
    /// the branch's segments landed say its next sequence number is past it: as when the guest
    /// commits extents out of the order it minted them, or a store closes a segment between
    /// commits while extents are open (TASK-79.3). Its segment landing, the branch reads it.
    #[test]
    fn extents_ahead_of_their_segments_stay_past_the_next_sequence_number() {
        assert_eq!(block_on(extent_ahead_of_its_segment_read()), Ok(b"late".to_vec()));
    }

    /// The collector going by the branch's next sequence number, not by the segment the key of an
    /// extent stored on its own names, as before TASK-79.3: the extent goes ahead of its segment,
    /// which then names what is lost (TASK-153.7).
    #[test]
    fn extents_ahead_of_their_segments_need_keys_naming_them() {
        crate::set_rules(crate::Rules { extent_keys_name_segments: false, ..crate::Rules::ALL });
        let read = block_on(extent_ahead_of_its_segment_read());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What reading an extent stored on its own gives, uploaded ahead of the segment naming it,
    /// committed after one past it in sequence, once a pass ran and then the segment landed.
    async fn extent_ahead_of_its_segment_read() -> Result<Vec<u8>, Error> {
        let bucket = store();
        branch::create(&*bucket, 1, 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
        let first = writer.chain().next_sequence();
        let (late, early) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
        let extents = vec![(early, b"early".to_vec())];
        commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
        assert_eq!(writer.chain().next_sequence(), first + 2);
        // The writer's next segment, which names `late`, its object uploaded first.
        let own = vec![(late.sequence, b"late".to_vec())];
        let objects = Batch::from(Commit { tag: 2, own, ..Commit::default() })
            .close(&mut writer.chain())
            .frame();
        let [(key, bytes)] = &objects.own[..] else { panic!("{} objects", objects.own.len()) };
        let created = bucket::create(&*bucket, key, bytes.read().unwrap()).await;
        assert!(matches!(created, Created::Created));

        collect(&*bucket, 1, &KEEP).await.unwrap();
        let segment = layout::segment(1, writer.chain().next_seq());
        let created = bucket::create(&*bucket, &segment, objects.segment.read().unwrap()).await;
        assert!(matches!(created, Created::Created));
        let recovered = recovery::recover(&*bucket, 1, None).await.unwrap();
        let location = &recovered.state.extents[&late];
        crate::read(&bucket, late, location).await
    }

    /// An object compacted is compacted once: its moves stand while it waits out the grace
    /// period. A live branch's record stays.
    #[test]
    fn objects_are_compacted_once() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (small, large) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let large_bytes: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
            let extents = vec![(small, b"small".to_vec()), (large, large_bytes)];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            let segment = writer.location(small).unwrap().key;
            commit(&writer, Commit { tag: 2, frees: vec![large], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();

            let compacting = Options { compact_below: 0.5, ..KEEP };
            let report = collect(&*bucket, 1, &compacting).await.unwrap();
            assert_eq!(report.compacted, 1);
            let after = keys(&*bucket).await;
            assert!(after.contains(&segment) && after.contains(&layout::moves(&segment)));
            assert!(after.contains(&layout::record(1)));
            let report = collect(&*bucket, 2, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 0, compacted: 0 });
            assert_eq!(keys(&*bucket).await, after);
            let location = writer.location(small).unwrap();
            assert_eq!(crate::read(&bucket, small, &location).await.unwrap(), b"small");
        });
    }

    /// fxdax: an object a byte was flipped in, in storage, is compacted to no moves, where its
    /// extents would be under CRCs of their own: the pass fails, and the object stays, as its
    /// extents' reads fail (TASK-153.11). In a part stored compressed, the kept extent's bytes
    /// compressing, and in one stored raw.
    #[test]
    fn objects_with_flipped_bytes_are_not_compacted() {
        for compressible in [true, false] {
            block_on(async {
                let bucket = store();
                branch::create(&*bucket, 1, 0).await.unwrap();
                let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
                let first = writer.chain().next_sequence();
                let (kept, freed) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
                // The kept extent's bytes compress, or not; the freed one's don't, from a xorshift
                // generator, so that the object is large enough to compact either way.
                let mut state = 0x9e37_79b9_7f4a_7c15_u64;
                let mut bytes = |len: u32, compressible: bool| -> Vec<u8> {
                    (0..len)
                        .map(|i| match compressible {
                            true => (i * 7 % 251) as u8,
                            false => {
                                state ^= state << 13;
                                state ^= state >> 7;
                                state ^= state << 17;
                                state as u8
                            }
                        })
                        .collect()
                };
                let extents =
                    vec![(kept, bytes(4096, compressible)), (freed, bytes(20_000, false))];
                commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
                let segment = writer.location(kept).unwrap().key;
                commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
                writer.checkpoint().await.unwrap();
                let mut bytes = bucket::get(&*bucket, &segment).await.unwrap().bytes;
                let stored = extent_frame::Parts::of(&bytes).unwrap().stored[0];
                let encoding = match compressible {
                    true => extent_frame::Encoding::Zstd,
                    false => extent_frame::Encoding::Raw,
                };
                assert_eq!(stored.encoding, encoding);
                bytes[stored.offset as usize + 9] ^= 0x10;
                bucket.put(&segment, bytes, PutMode::Overwrite).await.unwrap();

                let compacting = Options { compact_below: 0.5, ..KEEP };
                let corrupt = Error::Corrupt("an object's part, by its CRC".into());
                assert_eq!(collect(&*bucket, 1, &compacting).await, Err(corrupt));
                let after = keys(&*bucket).await;
                assert!(after.contains(&segment) && !after.contains(&layout::moves(&segment)));
                let location = writer.location(kept).unwrap();
                let read = crate::read(&bucket, kept, &location).await;
                let twice = format!("part 0 of {segment}, by its CRC, twice");
                assert_eq!(read, Err(Error::Corrupt(twice)));
            });
        }
    }

    /// TASK-175: a checkpoint landing after the branch's writer recovered it, by the writer it took
    /// the branch from, names an extent further along its moves than the writer does; the collector
    /// compacts the moves in turn and deletes what it can, and the writer still reads the extent,
    /// the moves on its way from where it was sealed kept. Found by the simulation, seed 677 of its
    /// `recreated` on task-165-3; red on main at 13db5c67c too.
    #[test]
    fn writers_read_past_checkpoints_landing_after_they_took_over() {
        assert_eq!(block_on(read_past_a_late_checkpoint()), Ok(b"kept".to_vec()));
    }

    /// The collector keeping the moves only from where the state it reads names the extent: the
    /// writer finds neither the object it names it in nor its moves.
    #[test]
    fn writers_reading_past_checkpoints_landing_after_they_took_over_need_ways_from_seals() {
        crate::set_rules(crate::Rules { ways_from_seals: false, ..crate::Rules::ALL });
        let read = block_on(read_past_a_late_checkpoint());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What the writer reads of the extent [`writers_read_past_checkpoints_landing_after_they_took_over`]
    /// keeps: the writer it took the branch from checkpointing it, the collector having moved it,
    /// after the writer recovered the branch from the checkpoint before; the moves then compacted
    /// in turn, and passes run until they delete nothing more.
    async fn read_past_a_late_checkpoint() -> Result<Vec<u8>, Error> {
        let bucket = store();
        branch::create(&*bucket, 1, 0).await.unwrap();
        let before = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
        let Compacted { kept, moved, sealed } = compacted(&*bucket, &before).await;
        let later = ExtentId::new(1, before.chain().next_sequence());
        let extents = vec![(later, b"later".to_vec())];
        commit(&before, Commit { tag: 3, extents, ..Commit::default() }).await;

        // The next writer recovers the branch from the checkpoint before the compaction.
        let writer = Writer::open(bucket.clone(), 1, 11, 0).await.unwrap();
        assert_eq!(writer.location(kept).unwrap().key, sealed);
        // The writer taken over, unaware, checkpoints its last segment, past that checkpoint,
        // following the moves.
        before.checkpoint().await.unwrap();
        let tag = writer.chain().tag(1);
        commit(&writer, Commit { tag, frees: vec![moved], ..Commit::default() }).await;
        collect_all(&*bucket).await;
        let left = keys(&*bucket).await;
        assert!(!left.contains(&sealed) && left.contains(&layout::moves(&layout::moves(&sealed))));
        crate::read(&bucket, kept, &writer.location(kept).unwrap()).await
    }

    /// TASK-175: a writer names the extents its checkpoint moves where they went before the
    /// checkpoint lands: passes of the collector running as it lands, before its create is
    /// answered, trust the checkpoint of the writer holding the lease, compact the moves in turn
    /// and delete the moves before, and the writer still reads the extent.
    #[test]
    fn writers_name_what_their_checkpoints_move_before_they_land() {
        assert_eq!(block_on(read_as_a_checkpoint_lands()), Ok(b"kept".to_vec()));
    }

    /// A writer naming them so once its checkpoint's create is answered: as it lands, it names the
    /// extent where neither the object nor its moves are.
    #[test]
    fn writers_naming_what_their_checkpoints_move_need_moves_taken_first() {
        crate::set_rules(crate::Rules { moves_taken_first: false, ..crate::Rules::ALL });
        let read = block_on(read_as_a_checkpoint_lands());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What the writer reads of the extent [`writers_name_what_their_checkpoints_move_before_they_land`]
    /// keeps, where it names it as its checkpoint recording the extent's moves lands, passes of
    /// the collector having run until they delete nothing more.
    async fn read_as_a_checkpoint_lands() -> Result<Vec<u8>, Error> {
        let direct = Arc::new(Direct(SimBucket::new()));
        let bucket = Arc::new(Hooked::new(direct.clone(), Hook::Checkpointed));
        branch::create(&*bucket, 1, 0).await.unwrap();
        let writer = Arc::new(Writer::open(bucket.clone(), 1, 10, 0).await.unwrap());
        let Compacted { kept, moved, .. } = compacted(&*bucket, &writer).await;
        commit(&writer, Commit { tag: 3, frees: vec![moved], ..Commit::default() }).await;
        let named = Arc::new(Mutex::new(None));
        let (passing, naming, watched) = (direct.clone(), named.clone(), Arc::downgrade(&writer));
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            collect_all(&*passing).now_or_never().unwrap();
            *naming.lock().unwrap() = watched.upgrade().unwrap().location(kept);
        }));
        writer.checkpoint().await.unwrap();
        let location = named.lock().unwrap().clone().unwrap();
        crate::read(&direct, kept, &location).await
    }

    /// What [`compacted`] made.
    struct Compacted {
        /// An extent the collector moved, which stays.
        kept: ExtentId,
        /// One it moved with it, freed next.
        moved: ExtentId,
        /// Where they were sealed.
        sealed: String,
    }

    /// Has `writer` seal three extents in a segment, then free one and checkpoint, and the
    /// collector compact the segment, moving the two others.
    async fn compacted<B: Bucket>(bucket: &B, writer: &Writer<B>) -> Compacted {
        let first = writer.chain().next_sequence();
        let [kept, moved, freed] = [0, 1, 2].map(|i| ExtentId::new(1, first + i));
        // Bytes that don't compress, so that the objects' sizes are their extents'.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut noise = || -> Vec<u8> {
            (0..20_000)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect()
        };
        let extents = vec![(kept, b"kept".to_vec()), (moved, noise()), (freed, noise())];
        commit(writer, Commit { tag: 1, extents, ..Commit::default() }).await;
        let sealed = writer.location(kept).unwrap().key;
        commit(writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
        let compacting = Options { compact_below: 0.6, ..KEEP };
        assert_eq!(collect(bucket, 1, &compacting).await.unwrap().compacted, 1);
        Compacted { kept, moved, sealed }
    }

    /// Runs passes of the collector, compacting objects less than 60% live and deleting what they
    /// compacted at once, until one does nothing.
    async fn collect_all(bucket: &impl Bucket) {
        let now = Options { compact_below: 0.6, grace: 0 };
        let mut holder = 2;
        while collect(bucket, holder, &now).await.unwrap() != Report::default() {
            holder += 1;
        }
    }

    /// TASK-177: a fork's recovery that listed no checkpoint of the fork's own, going to its
    /// parent's state where it was forked, finds the parent's record gone, the parent deleted and
    /// the collector having dropped its record once the fork's own checkpoint came: it starts again
    /// from that checkpoint. Found by the simulation, seed 210 of its `forks` at SIM_SOAK=100.
    #[test]
    fn forks_recover_past_parents_records_their_own_checkpoints_let_go() {
        assert_eq!(block_on(recover_as_a_fork_checkpoints()), Ok(vec![ExtentId::new(1, 1 << 40)]));
    }

    /// A recovery that finds what it inherited lost not looking for a checkpoint that came since:
    /// it fails on the record.
    #[test]
    fn forks_recovering_past_parents_records_their_own_checkpoints_let_go_need_newest_checkpoints()
    {
        crate::set_rules(crate::Rules { newest_checkpoint: false, ..crate::Rules::ALL });
        let recovered = block_on(recover_as_a_fork_checkpoints());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(recovered, Err(Error::Lost(layout::record(1))));
    }

    /// The extents a fork's recovery finds, its parent deleted, the fork's writer checkpointing and
    /// a pass of the collector running as the recovery reads the parent's record.
    async fn recover_as_a_fork_checkpoints() -> Result<Vec<ExtentId>, Error> {
        use crate::layout::{Contents, Origin, Segment};
        use crate::writer::Objects;
        let direct = Arc::new(Direct(SimBucket::new()));
        let bucket = Arc::new(Hooked::new(direct.clone(), Hook::Getting(layout::record(1))));
        branch::create(&*bucket, 1, 0).await.unwrap();
        let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
        let extent = ExtentId::new(1, parent.chain().next_sequence());
        let extents = vec![(extent, b"held".to_vec())];
        commit(&parent, Commit { tag: 1, extents, ..Commit::default() }).await;
        let footer =
            parent.chain().close(Contents { forks: vec![child(2)], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = parent.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        parent.upload(at).await.unwrap();
        parent.acknowledge().await.unwrap();
        branch::delete(&*bucket, 1, 11, u64::MAX).await.unwrap();
        let forked = Arc::new(Writer::open(bucket.clone(), 2, 12, 0).await.unwrap());
        let passing = direct.clone();
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            forked.checkpoint().now_or_never().unwrap().unwrap();
            collect(&*passing, 1, &KEEP).now_or_never().unwrap().unwrap();
        }));
        let recovered = recovery::recover(&*bucket, 2, Some(Origin { parent: 1, at })).await?;
        Ok(recovered.state.extents.into_keys().collect())
    }

    /// A bucket that runs `then` once, at the first request its hook names.
    struct Hooked {
        bucket: Arc<Direct>,
        hook: Hook,
        then: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    /// Where [`Hooked`] runs what it runs.
    enum Hook {
        /// As the create of a checkpoint takes effect, before it is answered.
        Checkpointed,
        /// Before a get of the object at this key.
        Getting(String),
    }

    impl Hooked {
        fn new(bucket: Arc<Direct>, hook: Hook) -> Self {
            Self { bucket, hook, then: Mutex::default() }
        }

        /// What it runs, if `now` and it has yet to.
        fn then(&self, now: bool) -> Option<Box<dyn FnOnce() + Send>> {
            now.then(|| self.then.lock().unwrap().take()).flatten()
        }
    }

    impl Bucket for Hooked {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            if let Some(then) = self.then(matches!(&self.hook, Hook::Getting(at) if at == key)) {
                then();
            }
            self.bucket.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
            self.bucket.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
            self.bucket.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            let checkpoint = matches!(layout::kind(key), Kind::Checkpoint { .. });
            let then = self.then(checkpoint && matches!(self.hook, Hook::Checkpointed));
            let put = self.bucket.put(key, bytes, mode);
            async move {
                let put = put.await;
                if let Some(then) = then {
                    then();
                }
                put
            }
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// Moves whose extents are all freed stay as long as the object they are of: a reader that
    /// finds it gone looks its extents up in them. They go after it.
    #[test]
    fn moves_outlive_what_they_are_of() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (small, large) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let extents = vec![(small, b"small".to_vec()), (large, vec![7; 20_000])];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            let segment = writer.location(small).unwrap().key;
            commit(&writer, Commit { tag: 2, frees: vec![large], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
            let now = Options { compact_below: 0.5, grace: 0 };
            assert_eq!(collect(&*bucket, 1, &now).await.unwrap().compacted, 1);

            commit(&writer, Commit { tag: 3, frees: vec![small], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
            collect(&*bucket, 2, &now).await.unwrap();
            let left = keys(&*bucket).await;
            assert!(!left.contains(&segment) && left.contains(&layout::moves(&segment)));
            collect(&*bucket, 3, &now).await.unwrap();
            assert!(!keys(&*bucket).await.contains(&layout::moves(&segment)));
        });
    }

    /// What a pass asked, round by round, as [`SimBucket::served_in_rounds`] serves it: the reads
    /// of branches' records, the reads of checkpoints whole, and the deletes, how many in each
    /// round that asked for any.
    fn asked(rounds: &[Vec<String>]) -> [Vec<usize>; 3] {
        let counted = |asking: &dyn Fn(&str) -> bool| -> Vec<usize> {
            let counts = rounds.iter().map(|round| round.iter().filter(|r| asking(r)).count());
            counts.filter(|&count| count > 0).collect()
        };
        let whole = |asked: &str| asked.split(' ').count() == 2;
        [
            counted(&|asked| asked.starts_with("get branches/")),
            counted(&|asked| asked.starts_with("get checkpoints/") && whole(asked)),
            counted(&|asked| asked.starts_with("delete ")),
        ]
    }

    /// A base, branch 1: a segment of extents, some stored on their own, which the next frees,
    /// then a segment forking it to `forks` children, and a checkpoint at it, the children's
    /// recoveries' start; the last `deleted` children deleted. Its requests served by `bucket`.
    fn forked_base(bucket: &Arc<SimBucket>, forks: u64, deleted: u64) {
        use crate::layout::{Chain, Contents, Segment};
        use crate::writer::Objects;
        let (writer, _) = bucket.served(async {
            branch::create(&**bucket, 1, 0).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        let first = writer.chain().next_sequence();
        let extents = (0..20).map(|n| (ExtentId::new(1, first + n), vec![n as u8; 8])).collect();
        let own: Vec<_> = (20..23).map(|n| (first + n, vec![n as u8; 16])).collect();
        let frees = own.iter().map(|(sequence, _)| ExtentId::new(1, *sequence)).collect();
        let children = (2..2 + forks + deleted).map(child).collect();
        let segments: [Box<dyn FnOnce(&mut Chain) -> Objects>; 3] = [
            Box::new(|chain| {
                let commit = Commit { tag: 1, extents, own, ..Commit::default() };
                Batch::from(commit).close(chain).frame()
            }),
            Box::new(|chain| {
                Batch::from(Commit { tag: 2, frees, ..Commit::default() }).close(chain).frame()
            }),
            Box::new(|chain| {
                let footer = chain.close(Contents { forks: children, ..Contents::default() });
                let segment = Segment { data: Vec::new(), footer: footer.clone() };
                Objects { footer, segment: segment.encode().into(), own: Vec::new() }
            }),
        ];
        for segment in segments {
            let seq = writer.add_segment(segment(&mut writer.chain())).unwrap();
            bucket.served(writer.upload(seq)).0.unwrap();
            bucket.served(writer.acknowledge()).0.unwrap();
        }
        bucket.served(writer.checkpoint()).0.unwrap();
        for child in 2 + forks..2 + forks + deleted {
            bucket.served(branch::delete(&**bucket, child, 20, 0)).0.unwrap();
        }
    }

    /// TASK-92.7: a pass of the collector over a base and its forks reads the branches' records
    /// many at once, the checkpoint the base's recovery and its forks' start from once, and
    /// deletes what goes many at once; a second pass of the same collector reads no record it
    /// read unchanged, and one that changed again.
    #[test]
    fn passes_read_records_and_checkpoints_once_and_delete_at_once() {
        let bucket = SimBucket::new();
        forked_base(&bucket, 6, 2);
        let mut collector = Collector::new(KEEP);
        let mut passes = Vec::new();
        for holder in 1..=2 {
            let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, holder));
            report.unwrap();
            passes.push(asked(&rounds));
        }
        // Its record, its children's, the deleted ones' too; its checkpoint; the segments the
        // checkpoint covers that hold no extent its branches hold, its extents freed, and the
        // leases of its children deleted.
        assert_eq!(passes, [[vec![9], vec![1], vec![8]], [vec![], vec![1], vec![]]]);

        bucket.served(branch::delete(&*bucket, 2, 20, 0)).0.unwrap();
        let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, 3));
        report.unwrap();
        assert_eq!(asked(&rounds), [vec![1], vec![1], vec![1]]);
    }

    /// TASK-92.7: what forks inherited together, taken once for them all, goes only once none of
    /// them holds it, whichever round of the pass recovered them. A base's three extents stored
    /// on their own, which the base frees after forking more children than a pass recovers at
    /// once: all of them free the first; the first child, which the first round recovers, keeps
    /// the second; and the last, which the last round recovers, the third. The first goes; the
    /// second and the third stay.
    #[test]
    fn what_forks_inherited_goes_once_none_holds_it() {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        let bucket = store();
        let children = 2..2 + RECOVERIES_AT_ONCE as u64 + 1;
        let (extents, held_in) = block_on(async {
            branch::create(&*bucket, 1, 0).await.unwrap();
            let base = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let sequence = base.chain().next_sequence();
            let extents: Vec<_> = (0..3).map(|n| ExtentId::new(1, sequence + n)).collect();
            let own = extents.iter().map(|extent| (extent.sequence, b"held".to_vec())).collect();
            let held_in = base.chain().next_seq();
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks =
                Contents { forks: children.clone().map(child).collect(), ..Contents::default() };
            let footer = base.chain().close(forks);
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = base.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            base.upload(seq).await.unwrap();
            base.acknowledge().await.unwrap();
            assert_eq!(base.checkpoint().await, Ok(Some(seq)));
            commit(&base, Commit { tag: 2, frees: extents.clone(), ..Commit::default() }).await;
            base.checkpoint().await.unwrap();
            for child in children.clone() {
                let writer = Writer::open(bucket.clone(), child, 10 + child, 0).await.unwrap();
                let held = match child {
                    _ if child == children.start => Some(extents[1]),
                    _ if child == children.end - 1 => Some(extents[2]),
                    _ => None,
                };
                let frees = extents.iter().copied().filter(|&extent| Some(extent) != held);
                let commit_frees = Commit { tag: 1, frees: frees.collect(), ..Commit::default() };
                commit(&writer, commit_frees).await;
            }
            (extents, held_in)
        });
        let kept = |keys: &[String]| -> Vec<bool> {
            extents
                .iter()
                .map(|&extent| keys.contains(&layout::extent(1, held_in, extent)))
                .collect()
        };
        assert_eq!(kept(&block_on(keys(&*bucket))), [true, true, true]);
        block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        assert_eq!(kept(&block_on(keys(&*bucket))), [false, true, true]);
    }

    /// TASK-79.85: a fork no host holds any more, as one whose host left neither its handle nor
    /// its deletion, is a live branch to the collector, which keeps what it inherited for it. An
    /// operator finds it among the live branches, and deletes it, whatever placement last took
    /// it: the next pass frees what only it held.
    #[test]
    fn branches_no_host_holds_keep_what_they_inherited_until_deleted() {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        let bucket = store();
        let (extent, held_in) = block_on(async {
            branch::create(&*bucket, 1, 0).await.unwrap();
            let base = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, base.chain().next_sequence());
            let held_in = base.chain().next_seq();
            let own = vec![(extent.sequence, b"held".to_vec())];
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks = Contents { forks: vec![child(2)], ..Contents::default() };
            let footer = base.chain().close(forks);
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = base.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            base.upload(seq).await.unwrap();
            base.acknowledge().await.unwrap();
            assert_eq!(base.checkpoint().await, Ok(Some(seq)));
            commit(&base, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
            base.checkpoint().await.unwrap();
            // Its host took it, at a placement later than any since, and went.
            let lease = layout::lease(2);
            branch::take_lease(&*bucket, &lease, 11, 7).await.unwrap();
            (extent, held_in)
        });
        let kept = || block_on(keys(&*bucket)).contains(&layout::extent(1, held_in, extent));
        block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        assert!(kept(), "what the fork inherited went while it is live");

        let live = block_on(branch::live(&*bucket)).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), [1, 2]);
        block_on(branch::delete(&*bucket, 2, 12, u64::MAX)).unwrap();
        block_on(collect(&*bucket, 2, &KEEP)).unwrap();
        assert!(!kept(), "what the fork inherited stayed once it was deleted");
        let live = block_on(branch::live(&*bucket)).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), [1]);
    }

    /// TASK-79.82: a fork's child recovers from the origin its record names, not from a parent
    /// whose state names it too, a fork to its ID refused: what it inherited is kept, though the
    /// parent it was forked from frees it. A round of recoveries past the first, with more branches
    /// than one takes, takes the lowest origin first, here the refused fork's.
    #[test]
    fn forks_keep_what_their_records_origin_holds_whoever_else_forked_to_their_id() {
        use crate::layout::{Contents, Origin, Segment};
        use crate::writer::Objects;
        const REFUSED: u64 = 1;
        const PARENT: u64 = 2;
        const CHILD: u64 = 3;
        /// Forks `writer`'s branch to `CHILD`, of `owner`: at the segment it returns.
        async fn fork(writer: &Writer<Store>, owner: u64) -> u64 {
            let forked = Child { branch: CHILD, owner, point: false };
            let forks = Contents { forks: vec![forked], ..Contents::default() };
            let footer = writer.chain().close(forks);
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            seq
        }
        let bucket = store();
        let (extent, held_in) = block_on(async {
            for branch in [REFUSED, PARENT].into_iter().chain(10..30) {
                branch::create(&*bucket, branch, 0).await.unwrap();
            }
            let parent = Writer::open(bucket.clone(), PARENT, 10, 0).await.unwrap();
            let extent = ExtentId::new(PARENT, parent.chain().next_sequence());
            let held_in = parent.chain().next_seq();
            let own = vec![(extent.sequence, b"held".to_vec())];
            commit(&parent, Commit { tag: 1, own, ..Commit::default() }).await;
            let at = fork(&parent, 0x32).await;
            parent.checkpoint().await.unwrap();
            commit(&parent, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
            parent.checkpoint().await.unwrap();
            let refused = Writer::open(bucket.clone(), REFUSED, 11, 0).await.unwrap();
            fork(&refused, 0x31).await;
            assert!(refused.refused(CHILD));
            refused.checkpoint().await.unwrap();
            let record = branch::record(&*bucket, CHILD).await.unwrap().unwrap();
            assert_eq!(record.origin(), Some(Origin { parent: PARENT, at }));
            (extent, held_in)
        });
        block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        let key = layout::extent(1, held_in, extent);
        assert!(block_on(keys(&*bucket)).contains(&key), "what the child inherited went");
    }

    /// TASK-92.7: a benchmark of a pass over a base of 100,000 extents and 100 forks of it, none
    /// with a checkpoint of its own: how long the pass takes, and the most memory the process
    /// held. `cargo nextest run -p extent_remote --run-ignored only bench_passes_over_many_forks
    /// --no-capture`, in a process of its own, which nextest gives each test.
    #[test]
    #[ignore]
    fn bench_passes_over_many_forks() {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const EXTENTS: u64 = 100_000;
        const FORKS: u64 = 100;
        let bucket = Arc::new(Direct(SimBucket::new()));
        block_on(async {
            branch::create(&*bucket, 1, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let extents =
                (0..EXTENTS).map(|n| (ExtentId::new(1, first + n), n.to_le_bytes().to_vec()));
            let commit = Commit { tag: 1, extents: extents.collect(), ..Commit::default() };
            let objects = Batch::from(commit).close(&mut writer.chain()).frame();
            let seq = writer.add_segment(objects).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            let footer = writer.chain().close(Contents {
                forks: (2..2 + FORKS).map(child).collect(),
                ..Contents::default()
            });
            let segment = Segment { data: Vec::new(), footer: footer.clone() };
            let objects = Objects { footer, segment: segment.encode().into(), own: Vec::new() };
            let seq = writer.add_segment(objects).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            writer.checkpoint().await.unwrap();
        });
        let started = std::time::Instant::now();
        let report = block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        let took = started.elapsed();
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: a getrusage of this process, into `usage`.
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        // In bytes on macOS, in KiB on Linux.
        let most =
            if cfg!(target_os = "macos") { usage.ru_maxrss >> 20 } else { usage.ru_maxrss >> 10 };
        println!(
            "a pass over a base of {EXTENTS} extents and {FORKS} forks: {took:?}, deleting {}; \
             at most {most} MiB held",
            report.deleted
        );
    }

    /// A bucket that marks branch `child`'s record deleted just before the first read of `key`.
    struct DeletedAt {
        bucket: Direct,
        key: String,
        child: u64,
        done: AtomicBool,
    }

    impl Bucket for DeletedAt {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            if key == self.key && !self.done.swap(true, Ordering::Relaxed) {
                let record = Record { deleted: true, ..crate::record::Kind::Own.of(0) };
                let (key, bytes) = (layout::record(self.child), layout::encode(&record));
                self.bucket.put(&key, bytes, PutMode::Overwrite).now_or_never().unwrap().unwrap();
            }
            self.bucket.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
            self.bucket.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
            self.bucket.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            self.bucket.put(key, bytes, mode)
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// A bucket that runs `meanwhile` once, as a pass first lists the checkpoints of `branch`:
    /// having listed the records, as it starts to read that branch's state.
    struct Meanwhile<F> {
        bucket: Arc<Direct>,
        branch: u64,
        meanwhile: std::sync::Mutex<Option<F>>,
    }

    impl<F: FnOnce() + Send + 'static> Bucket for Meanwhile<F> {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            self.bucket.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
            self.bucket.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
            self.bucket.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
            if prefix == layout::checkpoints(self.branch)
                && let Some(meanwhile) = self.meanwhile.lock().unwrap().take()
            {
                meanwhile();
            }
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            self.bucket.put(key, bytes, mode)
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// TASK-158: everything between a pass's listing the records and its reading a branch's state:
    /// a snapshot of it, a restore of that, the point deleted, what the point held freed, and a
    /// checkpoint, which drops the fork deleted. The state the pass reads names none of it, and it
    /// listed neither record: only listing them again finds the restored branch, which keeps what
    /// it inherited. Found by the simulation, seed 5310 of its `restores`.
    #[test]
    fn forks_of_points_made_and_gone_within_a_pass_keep_what_they_hold() {
        let sim = SimBucket::new();
        let direct = Arc::new(Direct(sim.clone()));
        const POINT: u64 = 2;
        const RESTORED: u64 = 3;
        let writer = block_on(async {
            branch::create(&*direct, 1, 0).await.unwrap();
            Arc::new(Writer::open(direct.clone(), 1, 10, 0).await.unwrap())
        });
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        let extents = vec![(extent, b"held".to_vec())];
        block_on(async {
            let batch = Batch::from(Commit { tag: 1, extents, ..Commit::default() });
            let seq = writer.add_segment(batch.close(&mut writer.chain()).frame()).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            writer.checkpoint().await.unwrap();
        });
        let meanwhile = {
            let (direct, writer) = (direct.clone(), writer.clone());
            move || {
                let work = async {
                    let batch = Batch::from(Commit { tag: 2, ..Commit::default() });
                    let seq = writer
                        .add_segment(
                            batch
                                .fork(
                                    Child { branch: POINT, owner: 0, point: true },
                                    &mut writer.chain(),
                                )
                                .frame(),
                        )
                        .unwrap();
                    writer.upload(seq).await.unwrap();
                    writer.acknowledge().await.unwrap();
                    let origin = Origin { parent: 1, at: seq };
                    branch::fork_point(&*direct, RESTORED, POINT, origin, 0).await.unwrap();
                    branch::delete(&*direct, POINT, 20, 0).await.unwrap();
                    let frees = vec![extent];
                    let batch = Batch::from(Commit { tag: 3, frees, ..Commit::default() });
                    let seq = writer.add_segment(batch.close(&mut writer.chain()).frame()).unwrap();
                    writer.upload(seq).await.unwrap();
                    writer.acknowledge().await.unwrap();
                    writer.checkpoint().await.unwrap();
                };
                work.now_or_never().expect("a direct bucket answers at once");
            }
        };
        let during = Meanwhile {
            bucket: direct.clone(),
            branch: 1,
            meanwhile: std::sync::Mutex::new(Some(meanwhile)),
        };
        let now = Options { compact_below: 0.0, grace: 0 };
        block_on(collect(&during, 1, &now)).unwrap();
        block_on(async {
            let record = branch::record(&*direct, RESTORED).await.unwrap();
            let recovered = recovery::recover(&*direct, RESTORED, record.unwrap().origin()).await;
            let location = recovered.unwrap().state.extents[&extent].clone();
            assert_eq!(crate::read(&direct, extent, &location).await.unwrap(), b"held");
        });
    }

    /// A live branch whose recovery finds what it needs gone fails the pass, which deletes
    /// nothing; one deleted meanwhile is live no longer, and the pass goes on.
    #[test]
    fn branches_lost_fail_the_pass_unless_deleted() {
        // Branch 2, forked from 1, which has no record; and a segment of no branch.
        let origin = Origin { parent: 1, at: 0 };
        let stray = layout::segment(3, 0);
        let sim = SimBucket::new();
        let direct = Direct(sim.clone());
        block_on(async {
            branch::record_fork(&direct, child(2), origin).await.unwrap();
            let bytes = layout::with_footer(b"", &());
            assert!(matches!(bucket::create(&direct, &stray, bytes).await, Created::Created));
        });
        let before = block_on(keys(&direct));
        let lost = Error::Lost(layout::record(1));
        assert_eq!(block_on(collect(&direct, 1, &KEEP)), Err(lost));
        assert_eq!(block_on(keys(&direct)), before);

        let deleting = DeletedAt {
            bucket: Direct(sim),
            key: layout::record(1),
            child: 2,
            done: AtomicBool::new(false),
        };
        let report = block_on(collect(&deleting, 2, &KEEP)).unwrap();
        assert_eq!(report, Report { deleted: 1, compacted: 0 });
        assert!(!block_on(keys(&deleting)).contains(&stray));
    }
}
