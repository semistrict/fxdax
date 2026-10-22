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

use crate::bucket::{self, Bucket, Meta};
use crate::layout::{self, Kind, Location, Moves, Origin, Record};
use crate::moves::{Moved, MovesMaps};
use crate::recovery::{self, Recovered};
use crate::{Error, branch, rules};
use std::collections::{BTreeMap, BTreeSet};
use storage_device::ExtentId;

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

/// One pass of the collector, as `holder`, which must differ from every other pass's.
pub async fn collect(
    bucket: &impl Bucket,
    holder: u64,
    options: &Options,
) -> Result<Report, Error> {
    let epoch = branch::take_lease(bucket, layout::GC_LEASE, holder, 0).await?;
    let early = rules().candidates_first;
    let mut objects = if early { listing(bucket).await } else { BTreeMap::new() };
    let records = records(bucket).await?;
    let live = live(bucket, &records).await?;
    if !early {
        objects = listing(bucket).await;
    }
    let pass = Pass::new(bucket, objects, records, live).await?;
    let mut report = Report::default();
    pass.compact(bucket, options, &mut report).await?;
    let doomed = pass.doomed(bucket, options).await?;
    // A pass a later one took the lease from stops.
    if branch::lease_epoch(bucket, layout::GC_LEASE).await? != epoch {
        return Ok(report);
    }
    for key in doomed {
        bucket::delete(bucket, &key).await;
        report.deleted += 1;
    }
    Ok(report)
}

async fn listing(bucket: &impl Bucket) -> BTreeMap<String, Meta> {
    bucket::list(bucket, "").await.into_iter().map(|meta| (meta.key.clone(), meta)).collect()
}

/// The branches' records. A deleted branch's children got theirs before it was marked deleted, so
/// listing them again after reading them finds those of any it read deleted.
async fn records(bucket: &impl Bucket) -> Result<BTreeMap<u64, Record>, Error> {
    let mut records = BTreeMap::new();
    loop {
        let mut new = false;
        for meta in bucket::list(bucket, layout::BRANCHES).await {
            if let Kind::Record { branch } = layout::kind(&meta.key)
                && !records.contains_key(&branch)
                && let Some(object) = bucket::get(bucket, &meta.key).await
            {
                records.insert(branch, layout::decode::<Record>(&object.bytes)?);
                new = true;
            }
        }
        if !new {
            return Ok(records);
        }
    }
}

/// The live branches, recovered: those whose records are not deleted, and the children of forks
/// in their states that have no records yet.
async fn live(
    bucket: &impl Bucket,
    records: &BTreeMap<u64, Record>,
) -> Result<BTreeMap<u64, Recovered>, Error> {
    let mut live = BTreeMap::new();
    let mut queue: Vec<(u64, Option<Origin>)> = records
        .iter()
        .filter(|(_, record)| !record.deleted)
        .map(|(&branch, record)| (branch, record.origin))
        .collect();
    while let Some((branch, origin)) = queue.pop() {
        if live.contains_key(&branch) {
            continue;
        }
        let recovered = match recovery::recover(bucket, branch, origin).await {
            Ok(recovered) => recovered,
            // Deleted since its record was read, what it needed may be gone: it is not live.
            Err(Error::Lost(lost)) => match branch::record(bucket, branch).await? {
                Some(record) if !record.deleted => return Err(Error::Lost(lost)),
                _ => continue,
            },
            Err(error) => return Err(error),
        };
        for fork in &recovered.state.forks {
            let record = records.get(&fork.child);
            if record.is_some_and(|record| record.deleted)
                || (record.is_none() && !rules().forks_from_states)
            {
                continue;
            }
            queue.push((fork.child, Some(Origin { parent: branch, at: fork.at })));
        }
        live.insert(branch, recovered);
    }
    Ok(live)
}

/// What a pass found: every object listed, and what the live branches need of them.
struct Pass {
    objects: BTreeMap<String, Meta>,
    records: BTreeMap<u64, Record>,
    live: BTreeMap<u64, Recovered>,
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
        live: BTreeMap<u64, Recovered>,
    ) -> Result<Self, Error> {
        let touched =
            live.values().flat_map(|recovered| recovered.touched.iter().cloned()).collect();
        let mut named: BTreeMap<String, BTreeMap<ExtentId, Location>> = BTreeMap::new();
        let mut hops = BTreeSet::new();
        let mut maps = MovesMaps::default();
        for recovered in live.values() {
            for (&extent, location) in &recovered.state.extents {
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
        Ok(Self { objects, records, live, touched, named, hops })
    }

    /// Whether `seq` of `branch`, live, is at or past its end: perhaps uploaded and not yet in it.
    fn in_flight(&self, branch: u64, seq: u64) -> bool {
        self.live.get(&branch).is_some_and(|recovered| seq >= recovered.end)
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
                        && match self.live.get(&branch) {
                            None => true,
                            Some(recovered) => {
                                orphan(bucket, branch, recovered, epoch, seq).await?
                            }
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

/// Whether an extent object of `branch`, `recovered`, uploaded by the writer of epoch `of` before
/// its segment `seq`, if its key names that, that its state does not name, is one no segment will
/// name: an earlier writer's, once a later one's segment is in the branch; or one whose segment,
/// the writer's, is in the branch, whose state has freed it since, or which is void. fxdax: not one
/// whose segment may yet land, however far past it the branch's next sequence number is: the
/// branch's extents are committed in no order of their sequence numbers (TASK-79.3).
async fn orphan(
    bucket: &impl Bucket,
    branch: u64,
    recovered: &Recovered,
    of: u64,
    seq: Option<u64>,
) -> Result<bool, Error> {
    let state = &recovered.state;
    let current = if rules().orphans_after_successor {
        state.epoch
    } else {
        branch::lease_epoch(bucket, &layout::lease(branch)).await?
    };
    Ok(of < current || (of == state.epoch && seq.is_some_and(|seq| seq < recovered.end)))
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
