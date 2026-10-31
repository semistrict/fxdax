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
//! Objects mostly freed are compacted: their live extents copied to their moves, one object,
//! written whole, before the object goes, a grace period later. Writers record moves in their next
//! checkpoints, after which the moves go too, unless a state's way to its extents still goes through
//! them.
//!
//! A pass reads records, recovers branches and deletes objects many at once; what forks inherited
//! together, their parent's state where they were forked, it reads once and takes once for them
//! all; and a collector reads a record again only once it changed ([`Collector`], TASK-92.7).

use crate::bucket::{self, Bucket, Meta};
use crate::layout::{self, Kind, Location, Moves, Origin, Record};
use crate::moves::{Moved, MovesMaps};
use crate::recovery::{self, Checkpoints, Layers, Recovered, State};
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
        let records = self.records(bucket).await?;
        let live = live(bucket, &records).await?;
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
                    (branch, read_record(bucket, known, &meta).await, meta.key)
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

/// The record at the key of `meta`, which a listing gave, if there is one, and the version read:
/// as `known` has it, if it read it at the version listed; or read now.
async fn read_record(
    bucket: &impl Bucket,
    known: &BTreeMap<String, (String, Record)>,
    meta: &Meta,
) -> Result<Option<(Option<String>, Record)>, Error> {
    if let Some(listed) = &meta.version.e_tag
        && let Some((version, record)) = known.get(&meta.key)
        && version == listed
    {
        return Ok(Some((Some(version.clone()), *record)));
    }
    let Some(object) = bucket::get(bucket, &meta.key).await else { return Ok(None) };
    Ok(Some((object.meta.version.e_tag, layout::decode(&object.bytes)?)))
}

async fn listing(bucket: &impl Bucket) -> BTreeMap<String, Meta> {
    bucket::list(bucket, "").await.into_iter().map(|meta| (meta.key.clone(), meta)).collect()
}

/// The live branches, recovered, several at once: those whose records are not deleted, and the
/// children of forks in their states that have no records yet. Forks of a branch at one segment
/// are recovered together, those without checkpoints of their own starting from the branch's
/// state there, read once for them ([`Checkpoints`]); and the states are folded into what the
/// pass needs of them as they are recovered, what forks inherited together once for them all,
/// rather than each fork's copy of it ([`Layers`]).
async fn live(bucket: &impl Bucket, records: &BTreeMap<u64, Record>) -> Result<Live, Error> {
    let checkpoints = Checkpoints::new(bucket, CHECKPOINTS_KEPT);
    let mut live = Live::default();
    let origin = |origin: Option<Origin>| origin.map(|origin| (origin.parent, origin.at));
    // Forks made at the same segment next to each other, in the same rounds.
    let mut queue: BTreeSet<(Option<(u64, u64)>, u64)> = records
        .iter()
        .filter(|(_, record)| !record.deleted)
        .map(|(&branch, record)| (origin(record.origin), branch))
        .collect();
    let mut taken = BTreeSet::new();
    while !queue.is_empty() {
        // Each once, queued by its record, or by its parent's state, or both.
        let round: Vec<_> = std::iter::from_fn(|| queue.pop_first())
            .filter(|&(_, branch)| taken.insert(branch))
            .take(RECOVERIES_AT_ONCE)
            .collect();
        let recoveries = round.into_iter().map(|(origin, branch)| {
            let origin = origin.map(|(parent, at)| Origin { parent, at });
            let checkpoints = &checkpoints;
            async move { (branch, recovery::recover_layers(checkpoints, branch, origin).await) }
        });
        let mut inherited = Inherited::default();
        for (branch, recovered) in futures::future::join_all(recoveries).await {
            let recovered = match recovered {
                Ok(recovered) => recovered,
                // Deleted since its record was read, what it needed may be gone: it is not live.
                Err(Error::Lost(lost)) => match branch::record(bucket, branch).await? {
                    Some(record) if !record.deleted => return Err(Error::Lost(lost)),
                    _ => continue,
                },
                Err(error) => return Err(error),
            };
            for fork in &recovered.state.own.forks {
                let record = records.get(&fork.child);
                if record.is_some_and(|record| record.deleted)
                    || (record.is_none() && !rules().forks_from_states)
                    || taken.contains(&fork.child)
                {
                    continue;
                }
                queue.insert((Some((branch, fork.at)), fork.child));
            }
            live.add(recovered, &mut inherited);
        }
        live.inherit(inherited);
    }
    Ok(live)
}

/// What a pass needs of the live branches' states, folded in as they are recovered.
#[derive(Default)]
struct Live {
    branches: BTreeMap<u64, Ended>,
    /// What recovering them read.
    touched: BTreeSet<String>,
    /// Where the extents they name are: each place once, however many of them name it there.
    located: BTreeMap<ExtentId, BTreeSet<Location>>,
}

/// What deciding what goes needs of a live branch's state, its extents but: where its segments end,
/// and the highest epoch of those applied.
struct Ended {
    end: u64,
    epoch: u64,
}

/// The states forks inherited, as their recoveries read them, each with the extents of it every
/// fork inheriting it holds no longer where it inherited it: what no fork of a round recovered
/// needs of it.
#[derive(Default)]
struct Inherited {
    states: Vec<(Arc<State>, BTreeSet<ExtentId>)>,
}

impl Live {
    /// Folds in `recovered`, a live branch's: its own layer, and what it inherited into
    /// `inherited`, its round's, which [`Live::inherit`] folds in once for all its forks.
    fn add(&mut self, recovered: Recovered<Layers>, inherited: &mut Inherited) {
        let Recovered { state: Layers { inherited: from, own, shadowed }, end, touched, .. } =
            recovered;
        self.touched.extend(touched);
        for (extent, location) in own.extents {
            self.located.entry(extent).or_default().insert(location);
        }
        if let Some(from) = from {
            match inherited.states.iter_mut().find(|(state, _)| Arc::ptr_eq(state, &from)) {
                Some((_, unheld)) => unheld.retain(|extent| shadowed.contains(extent)),
                None => inherited.states.push((from, shadowed)),
            }
        }
        let ended = Ended { end, epoch: own.epoch };
        self.branches.insert(own.branch, ended);
    }

    /// Folds in what forks inherited, `inherited`: each state once, but the extents of it none of
    /// them holds where they inherited them.
    fn inherit(&mut self, inherited: Inherited) {
        for (state, unheld) in inherited.states {
            for (extent, location) in &state.extents {
                if unheld.contains(extent) {
                    continue;
                }
                let located = self.located.entry(*extent).or_default();
                if !located.contains(location) {
                    located.insert(location.clone());
                }
            }
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
    /// data moved on.
    hops: BTreeSet<String>,
}

impl Pass {
    async fn new(
        bucket: &impl Bucket,
        objects: BTreeMap<String, Meta>,
        records: BTreeMap<u64, Record>,
        live: Live,
    ) -> Result<Self, Error> {
        let Live { branches, touched, located } = live;
        let mut named: BTreeMap<String, BTreeMap<ExtentId, Location>> = BTreeMap::new();
        let mut hops = BTreeSet::new();
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

    /// Whether `seq` of `branch`, live, is at or past its end: perhaps uploaded and not yet in it.
    fn in_flight(&self, branch: u64, seq: u64) -> bool {
        self.branches.get(&branch).is_some_and(|ended| seq >= ended.end)
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
                Kind::Segment { branch, seq } => {
                    !self.touched.contains(key) && !self.in_flight(branch, seq)
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
            let goes = match layout::kind(key) {
                Kind::Checkpoint { .. } => !self.touched.contains(key),
                // Once a checkpoint covers it, if its extents are dead or moved.
                Kind::Segment { branch, seq } => {
                    !self.in_flight(branch, seq)
                        && !self.touched.contains(key)
                        && (!named || moved_long_ago(key))
                }
                Kind::Extent { branch, epoch, seq, .. } => {
                    !named
                        && match self.branches.get(&branch) {
                            None => true,
                            Some(ended) => orphan(bucket, branch, ended, epoch, seq).await?,
                        }
                }
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
                    self.records.get(&branch).is_some_and(|record| record.deleted && !record.forked)
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

/// Whether an extent object of `branch`, `state`, uploaded by the writer of epoch `of` before
/// its segment `seq`, if its key names that, that its state does not name, is one no segment will
/// name: an earlier writer's, once a later one's segment is in the branch; or one whose segment,
/// the writer's, is in the branch, whose state has freed it since, or which is void. fxdax: not one
/// whose segment may yet land, however far past it the branch's next sequence number is: the
/// branch's extents are committed in no order of their sequence numbers (TASK-79.3).
async fn orphan(
    bucket: &impl Bucket,
    branch: u64,
    state: &Ended,
    of: u64,
    seq: Option<u64>,
) -> Result<bool, Error> {
    let current = if rules().orphans_after_successor {
        state.epoch
    } else {
        branch::lease_epoch(bucket, &layout::lease(branch)).await?
    };
    Ok(of < current || (of == state.epoch && seq.is_some_and(|seq| seq < state.end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::bucket::{ByteRange, Created, Error as BucketError, Object, PutMode, Version};
    use crate::sim::bucket::{Direct, SimBucket};
    use crate::store::Store;
    use crate::writer::Writer;
    use futures::FutureExt as _;
    use futures::executor::block_on;
    use object_store::memory::InMemory;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// No compaction, and compacted objects kept long.
    const KEEP: Options = Options { compact_below: 0.0, grace: 3_600_000 };

    fn store() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), ""))
    }

    /// `commit` in a segment of its own, uploaded and acknowledged.
    async fn commit(writer: &Writer<Store>, commit: Commit) {
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
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (freed, kept) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let own = vec![(freed, b"freed".to_vec()), (kept, b"kept".to_vec())];
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

    /// An extent stored on its own, uploaded ahead of the segment that names it, stays, though
    /// the branch's segments landed say its next sequence number is past it: as when the guest
    /// commits extents out of the order it minted them, or a store closes a segment between
    /// commits while extents are open (TASK-79.3). Its segment landing, the branch reads it.
    #[test]
    fn extents_ahead_of_their_segments_stay_past_the_next_sequence_number() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (late, early) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let extents = vec![(early, b"early".to_vec())];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            assert_eq!(writer.chain().next_sequence(), first + 2);
            // The writer's next segment, which names `late`, its object uploaded first.
            let own = vec![(late, b"late".to_vec())];
            let objects = Batch::from(Commit { tag: 2, own, ..Commit::default() })
                .close(&mut writer.chain())
                .frame();
            let [(key, bytes)] = &objects.own[..] else { panic!("{} objects", objects.own.len()) };
            assert!(matches!(
                bucket::create(&*bucket, key, bytes.read().unwrap()).await,
                Created::Created
            ));

            collect(&*bucket, 1, &KEEP).await.unwrap();
            assert!(keys(&*bucket).await.contains(key), "{key}, ahead of its segment, went");
            let segment = layout::segment(1, writer.chain().next_seq());
            let created = bucket::create(&*bucket, &segment, objects.segment.read().unwrap()).await;
            assert!(matches!(created, Created::Created));
            let recovered = recovery::recover(&*bucket, 1, None).await.unwrap();
            let location = &recovered.state.extents[&late];
            assert_eq!(crate::read(&bucket, late, location).await.unwrap(), b"late");
        });
    }

    /// An object compacted is compacted once: its moves stand while it waits out the grace
    /// period. A live branch's record stays.
    #[test]
    fn objects_are_compacted_once() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1).await.unwrap();
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

    /// Moves whose extents are all freed stay as long as the object they are of: a reader that
    /// finds it gone looks its extents up in them. They go after it.
    #[test]
    fn moves_outlive_what_they_are_of() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, 1).await.unwrap();
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
            branch::create(&**bucket, 1).await.unwrap();
            Writer::open(bucket.clone(), 1, 10, 0).await.unwrap()
        });
        let first = writer.chain().next_sequence();
        let extents = (0..20).map(|n| (ExtentId::new(1, first + n), vec![n as u8; 8])).collect();
        let own: Vec<_> =
            (20..23).map(|n| (ExtentId::new(1, first + n), vec![n as u8; 16])).collect();
        let frees = own.iter().map(|(extent, _)| *extent).collect();
        let children = (2..2 + forks + deleted).collect();
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
            branch::create(&*bucket, 1).await.unwrap();
            let base = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let sequence = base.chain().next_sequence();
            let extents: Vec<_> = (0..3).map(|n| ExtentId::new(1, sequence + n)).collect();
            let own = extents.iter().map(|&extent| (extent, b"held".to_vec())).collect();
            let held_in = base.chain().next_seq();
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks = Contents { forks: children.clone().collect(), ..Contents::default() };
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
            branch::create(&*bucket, 1).await.unwrap();
            let base = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, base.chain().next_sequence());
            let held_in = base.chain().next_seq();
            let own = vec![(extent, b"held".to_vec())];
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks = Contents { forks: [2].into_iter().collect(), ..Contents::default() };
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
            branch::create(&*bucket, 1).await.unwrap();
            let writer = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let extents =
                (0..EXTENTS).map(|n| (ExtentId::new(1, first + n), n.to_le_bytes().to_vec()));
            let commit = Commit { tag: 1, extents: extents.collect(), ..Commit::default() };
            let objects = Batch::from(commit).close(&mut writer.chain()).frame();
            let seq = writer.add_segment(objects).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            let footer = writer
                .chain()
                .close(Contents { forks: (2..2 + FORKS).collect(), ..Contents::default() });
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
                let record = Record { origin: None, forked: true, deleted: true };
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
            branch::record_fork(&direct, 2, origin).await.unwrap();
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
