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
use crate::reader::{self, Moved, MovesMaps, Recovered};
use crate::{EPOCH_SHIFT, Error, branch, rules};
use std::collections::{BTreeMap, BTreeSet};
use storage_device::ExtentId;

#[derive(Clone, Copy, Debug)]
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
    let epoch = branch::take_lease(bucket, layout::GC_LEASE, holder).await?;
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
        let recovered = match reader::recover(bucket, branch, origin).await {
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
            if !below(layout::data_len(&object.bytes)?.0) {
                continue;
            }
            let mut data = Vec::new();
            let mut entries = Vec::new();
            for (&extent, location) in extents {
                let range = location.offset as usize..(location.offset + location.len) as usize;
                let bytes = object
                    .bytes
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
                Kind::Extent { branch, epoch, sequence } => {
                    !named
                        && match self.live.get(&branch) {
                            None => true,
                            Some(recovered) => {
                                orphan(bucket, branch, &recovered.state, epoch, sequence).await?
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

/// Whether an extent object of `branch`, uploaded by the writer of epoch `of`, of sequence number
/// `sequence`, that its state does not name, is one no segment will name: an earlier writer's, once
/// a later one's segment is in the branch, or one the writer minted and a segment of its, in the
/// branch, is past.
async fn orphan(
    bucket: &impl Bucket,
    branch: u64,
    state: &reader::State,
    of: u64,
    sequence: u64,
) -> Result<bool, Error> {
    let current = if rules().orphans_after_successor {
        state.epoch
    } else {
        branch::lease_epoch(bucket, &layout::lease(branch)).await?
    };
    Ok(of < current
        || (of == state.epoch && sequence >> EPOCH_SHIFT == of && sequence < state.next_sequence))
}
