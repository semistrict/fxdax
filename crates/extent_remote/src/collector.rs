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
//!
//! fxdax: two passes delete nothing (TASK-264). A mark-only pass ([`Collector::mark`]) is a pass in
//! every respect but that: it takes the lease and compacts, and writes what it would delete to a
//! marks object (`gc/marks/<epoch>`, [`layout::Marks`]), which every reader of the bucket warns of
//! reaching ([`crate::marks`]), and which the next such pass compares what it finds doomed with: an
//! object doomed before and kept now, the same object still, is a bug, as no correct fork or
//! recovery reaches what no live branch reached. Nothing marked goes, so the bucket grows by what
//! it would have reclaimed, the compacted objects and their moves both. A dry run
//! ([`Collector::dry_run`]) writes nothing at all, the lease neither: it says what a pass would
//! delete, less what a pass would delete only for compacting it, a grace period after.

use crate::bucket::{self, Bucket, ByteRange, Meta, Version};
use crate::layout::{self, Branch, Kind, Location, Marked, Marks, Moves, Origin};
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

/// How many objects' trailers a pass reads at once, to learn their data's lengths.
const TRAILERS_AT_ONCE: usize = 32;

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

/// fxdax: what a mark-only pass did ([`Collector::mark`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marking {
    /// What it compacted; it deleted nothing.
    pub report: Report,
    /// The marks it wrote, at [`layout::marks`] of their epoch: none if a later pass took the lease
    /// from it, which stopped it as it stops a pass before deleting.
    pub marks: Option<Marks>,
    /// The objects the newest marks before named that it found kept, the same objects still: a
    /// bug, each warned of.
    pub resurrected: Vec<String>,
}

/// fxdax: what a dry run found ([`Collector::dry_run`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DryRun {
    /// What a pass would delete, as listed, in the order of their keys; less what it would compact
    /// and so delete a grace period after.
    pub doomed: Vec<Meta>,
    /// What a pass would compact, as listed: what it deletes too, a grace period after, once no state's way to an extent goes through it.
    pub compactable: Vec<Meta>,
    /// Whether a pass took the collector's lease while it ran: what it read may be older than what
    /// that pass, deleting since, read, and what it found is no more than a guess.
    pub fenced: bool,
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
/// deleted branches, which stay, never do (TASK-92.7, TASK-174).
pub struct Collector {
    options: Options,
    /// The records read, by their keys, with the versions they were read at.
    records: BTreeMap<String, (String, Record)>,
    lengths: Lengths,
}

impl Collector {
    pub fn new(options: Options) -> Self {
        Self { options, records: BTreeMap::new(), lengths: Lengths::default() }
    }

    /// One pass, as `holder`, which must differ from every other pass's.
    pub async fn collect(&mut self, bucket: &impl Bucket, holder: u64) -> Result<Report, Error> {
        let epoch = branch::take_lease(bucket, layout::GC_LEASE, holder, 0).await?;
        let Some(pass) = self.survey(bucket, epoch, Writes::Some).await? else {
            return Ok(Report::default());
        };
        let mut report = Report::default();
        pass.compact(bucket, &self.options, &mut self.lengths, &mut report).await?;
        let doomed = pass.doomed(bucket, &self.options).await?;
        // A pass a later one took the lease from stops.
        if fenced(bucket, epoch).await? {
            return Ok(report);
        }
        // Many at once, in any order: none of it is what a live branch needs, whatever else goes.
        let deletes = futures::stream::iter(&doomed).map(|meta| bucket::delete(bucket, &meta.key));
        report.deleted += deletes.buffer_unordered(DELETES_AT_ONCE).count().await;
        Ok(report)
    }

    /// fxdax: one mark-only pass, as `holder`, which must differ from every other pass's: a pass
    /// but for deleting, which writes what it would delete to its marks instead (TASK-264).
    ///
    /// It takes the collector's lease, as a pass that deletes does (doc-7, Garbage collection, step
    /// 1). Its marks are ordered by it, each pass's epoch its own and later than every pass's
    /// before, so that readers, and the next mark-only pass, find the newest by their keys, and no
    /// two passes' marks share a key. A pass that deletes, which it then fences, deletes nothing,
    /// and the next deletes what it would have: a pass fenced is late, never wrong. And it stops, as
    /// a pass that deletes does (TASK-180), where it finds lost what a live branch needs once a pass
    /// that deletes took the lease from it, deleting since what it read live; a pass not holding the
    /// lease could not tell that from a bug. Fenced, it writes no marks, as a pass fenced deletes
    /// nothing: what it read may be older than what a later pass did. Compacting, which writes only
    /// create-only moves that other passes' stand beside, needs the lease no more than before.
    pub async fn mark(&mut self, bucket: &impl Bucket, holder: u64) -> Result<Marking, Error> {
        let epoch = branch::take_lease(bucket, layout::GC_LEASE, holder, 0).await?;
        let Some(pass) = self.survey(bucket, epoch, Writes::Some).await? else {
            return Ok(Marking::default());
        };
        let mut report = Report::default();
        pass.compact(bucket, &self.options, &mut self.lengths, &mut report).await?;
        let doomed = pass.doomed(bucket, &self.options).await?;
        if fenced(bucket, epoch).await? {
            return Ok(Marking { report, ..Marking::default() });
        }
        let resurrected = resurrected(bucket, epoch, &self.options, &pass.objects, &doomed).await;
        let marked = doomed.into_iter().map(Marked::of).collect();
        let marks = Marks { epoch, grace: self.options.grace, marked };
        // The epoch is this pass's taking of the lease alone: nothing else is at its key.
        bucket::create(bucket, &layout::marks(epoch), layout::encode(&marks)).await;
        Ok(Marking { report, marks: Some(marks), resurrected })
    }

    /// fxdax: what a pass would delete, writing nothing (TASK-264): not taking the collector's
    /// lease, which would fence a pass that deletes, running meanwhile, out of its deletes. It
    /// compacts nothing, and so finds doomed none of what a pass would delete only once it
    /// compacted it, a grace period after: no more than a pass would delete, and less. What a pass
    /// would compact, it says apart ([`DryRun::compactable`]). It goes by
    /// the lease's epoch as it starts, as a pass by the epoch it takes, to tell whether a pass that
    /// deletes ran meanwhile ([`DryRun::fenced`]). Where a pass would give up on a fork of a point
    /// deleted, writing its record, it takes the fork as given up on, as a pass mostly finds it.
    pub async fn dry_run(&mut self, bucket: &impl Bucket) -> Result<DryRun, Error> {
        let epoch = branch::lease_epoch(bucket, layout::GC_LEASE).await?;
        let Some(pass) = self.survey(bucket, epoch, Writes::None).await? else {
            return Ok(DryRun { fenced: true, ..DryRun::default() });
        };
        let doomed = pass.doomed(bucket, &self.options).await?.into_iter().cloned().collect();
        let compactable = pass.compactable(bucket, &self.options, &mut self.lengths).await?;
        let compactable = compactable.into_iter().map(|(meta, _)| meta.clone()).collect();
        Ok(DryRun { doomed, compactable, fenced: fenced(bucket, epoch).await? })
    }

    /// What a pass of the collector's lease's `epoch` finds: every object listed, and what the live
    /// branches need of them; none if a later pass took the lease from it as it found what a live
    /// branch needs lost (TASK-180). It writes what giving up on forks needs, if `writes` says so.
    async fn survey(
        &mut self,
        bucket: &impl Bucket,
        epoch: u64,
        writes: Writes,
    ) -> Result<Option<Pass>, Error> {
        let early = rules().candidates_first;
        let mut objects = if early { listing(bucket).await } else { BTreeMap::new() };
        let mut records = self.records(bucket).await?;
        let mut live = Live::default();
        loop {
            if rules().collector_abandons {
                abandon(bucket, &mut records, writes).await?;
            }
            let checkpointed = match recover_live(bucket, &records, &mut live, writes).await {
                // Fenced, it read what the pass that fenced it, deleting since, may have found no
                // live branch needs: a point deleted, read live before, and its fork being made,
                // read after (TASK-180). It stops, as it would before deleting.
                Err(Error::Lost(_))
                    if rules().passes_fenced_stop && fenced(bucket, epoch).await? =>
                {
                    return Ok(None);
                }
                recovered => recovered?,
            };
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
        self.lengths.retain(&objects);
        Ok(Some(Pass::new(bucket, objects, records, live).await?))
    }

    /// The branches' records, read many at once, but those read before at the versions listed. A
    /// deleted branch's children got theirs before it was marked deleted, so listing them again
    /// after reading them finds those of any it read deleted.
    async fn records(&mut self, bucket: &impl Bucket) -> Result<BTreeMap<Branch, Record>, Error> {
        let mut records = BTreeMap::new();
        loop {
            let listed =
                bucket::list(bucket, layout::TENANTS).await.into_iter().filter_map(|meta| {
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

/// fxdax: whether a pass writes to the bucket what giving up on forks needs: a dry run writes
/// nothing (TASK-264).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writes {
    Some,
    None,
}

/// fxdax: the objects the newest marks before those of `epoch` named, that `objects`, listed by the
/// mark-only pass of `epoch`, holds still, the same objects, and that `doomed`, what it found no live
/// branch needs, does not: each warned of, loudly. No correct fork or recovery reaches what no live
/// branch reached, so each is a bug in the collector, which a pass deleting would have lost. Marks
/// of a shorter grace period than `options`' are compared with none: what a compacted object needs
/// to go, its moves aged, takes longer by a longer one. Marks it can't read, it warns of.
async fn resurrected(
    bucket: &impl Bucket,
    epoch: u64,
    options: &Options,
    objects: &BTreeMap<String, Meta>,
    doomed: &[&Meta],
) -> Vec<String> {
    let listed = bucket::list(bucket, layout::GC_MARKS).await;
    let before = listed.iter().rev().find(
        |meta| matches!(layout::kind(&meta.key), Kind::Marks { epoch: marked } if marked < epoch),
    );
    let Some(object) = (match before {
        Some(before) => bucket::get(bucket, &before.key).await,
        None => None,
    }) else {
        return Vec::new();
    };
    let marks: Marks = match layout::decode(&object.bytes) {
        Ok(marks) => marks,
        Err(error) => {
            let key = &object.meta.key;
            log::warn!(key, error:%; "fxdax-gc: {key}, the marks before, unread: {error}");
            return Vec::new();
        }
    };
    if marks.grace < options.grace {
        return Vec::new();
    }
    let doomed: BTreeSet<&str> = doomed.iter().map(|meta| meta.key.as_str()).collect();
    let mut resurrected = Vec::new();
    for marked in &marks.marked {
        let key = marked.key.as_str();
        if doomed.contains(key) || !objects.get(key).is_some_and(|meta| marked.is(meta)) {
            continue;
        }
        let (kind, before) = (layout::kind(key), marks.epoch);
        log::warn!(
            key, kind:?, marks = before, epoch;
            "fxdax-gc: RESURRECTED {key}: the mark-only pass {before:x} found no live branch needs \
             it, and the pass {epoch:x} finds one does: a bug in the collector, which deleting \
             would have lost it to"
        );
        resurrected.push(marked.key.clone());
    }
    resurrected
}

/// Whether a later pass took the collector's lease from the pass that took it at `epoch`.
async fn fenced(bucket: &impl Bucket, epoch: u64) -> Result<bool, Error> {
    Ok(branch::lease_epoch(bucket, layout::GC_LEASE).await? != epoch)
}

/// Gives up on the forks being made of points that were deleted, as `records` say, which it says
/// are deleted then, or made, if that came first ([`branch::abandon`]); without `writes`, takes them
/// as deleted, writing nothing.
async fn abandon(
    bucket: &impl Bucket,
    records: &mut BTreeMap<Branch, Record>,
    writes: Writes,
) -> Result<(), Error> {
    let gone = |point: Branch| records.get(&point).is_none_or(|record| record.deleted);
    let abandoned: Vec<Branch> = (records.iter())
        .filter(|(_, record)| !record.deleted && record.forking().is_some_and(gone))
        .map(|(&child, _)| child)
        .collect();
    for child in abandoned {
        let record = match writes {
            Writes::Some => branch::abandon(bucket, child).await?,
            Writes::None => Record { deleted: true, ..records[&child] },
        };
        records.insert(child, record);
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
    branch: Branch,
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
///
/// A branch is recovered once by each origin it is named by: a child its parent's state names, that
/// fork refused, has its record from another branch whose fork to its ID came first, a restore's,
/// which listing the records again may find only after the child was recovered by the state; what
/// the record's origin needs is what the child does (TASK-173).
///
/// A failed fork's claim no writer has, whose recovery finds what it names lost, it gives up,
/// writing its record so if `writes` says so.
async fn recover_live(
    bucket: &impl Bucket,
    records: &BTreeMap<Branch, Record>,
    live: &mut Live,
    writes: Writes,
) -> Result<bool, Error> {
    let checkpoints = Checkpoints::new(bucket, CHECKPOINTS_KEPT);
    let mut checkpointed = false;
    // Forks made at the same segment next to each other, in the same rounds.
    let mut queue: BTreeSet<(Option<Origin>, Branch)> = records
        .iter()
        .filter(|(_, record)| !record.deleted)
        .map(|(&branch, record)| (record.origin(), branch))
        .filter(|&(origin, branch)| !live.recovered(branch, origin))
        .collect();
    while !queue.is_empty() {
        // Each once by each origin, queued by its record, or by its parent's state, or both.
        let round: Vec<_> = std::iter::from_fn(|| queue.pop_first())
            .filter(|&(origin, branch)| live.take(branch, origin))
            .take(RECOVERIES_AT_ONCE)
            .collect();
        let recoveries = round.into_iter().map(|(origin, branch)| {
            let checkpoints = &checkpoints;
            async move {
                // Its lease, read after the listing and its recovery: what its writer may name
                // ([`behind`]), and whether one is taking it over ([`Ended::taking`]).
                let recovered = match recovery::recover_layers(checkpoints, branch, origin).await {
                    Ok(recovered) => {
                        match branch::lease_epoch(bucket, &layout::lease(branch)).await {
                            Ok(lease) => Ok((recovered, lease)),
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                (branch, recovered)
            }
        });
        let mut inherited = Inherited::default();
        for (branch, recovered) in futures::future::join_all(recoveries).await {
            let (recovered, lease) = match recovered {
                Ok(recovered) => recovered,
                // Deleted since its record was read, what it needed may be gone: it is not live.
                // Nor is a failed fork's claim no writer has: nothing ever read what it names.
                Err(Error::Lost(lost)) => match branch::record(bucket, branch).await? {
                    Some(record) if record.deleted => continue,
                    Some(record) if branch::lost_is_ok(bucket, branch, &record).await? => {
                        if writes == Writes::Some {
                            branch::abandon_claim(bucket, branch).await?;
                        }
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
                let origin = Some(Origin { parent: branch, at: fork.at });
                let recorded = records.contains_key(&fork.child);
                if recorded || !rules().forks_from_states || live.recovered(fork.child, origin) {
                    continue;
                }
                queue.insert((origin, fork.child));
            }
            checkpointed |= recovered.checkpoint.is_some();
            live.add(recovered, lease, &mut inherited);
        }
        live.inherit(inherited);
    }
    Ok(checkpointed)
}

/// Whether the writer of the branch `recovered` is of may name the extents it holds anywhere on
/// their ways from where they were sealed, before the moves its state followed: unless its state
/// is from a checkpoint of the writer holding its lease, of epoch `lease`, read after the
/// checkpoint was listed. The extents it inherited, its writer may name so whatever this says
/// ([`Live::inherit`]).
fn behind(recovered: &Recovered<Layers>, lease: u64) -> bool {
    rules().ways_from_seals && recovered.checkpointer != Some(lease)
}

/// What a pass needs of the live branches' states, folded in as they are recovered.
#[derive(Default)]
struct Live {
    /// Those recovered, or being, by the origins they were recovered from: each once by each.
    taken: BTreeSet<(Branch, Option<Origin>)>,
    /// Where each recovery of each ended.
    branches: BTreeMap<Branch, Vec<Ended>>,
    /// What recovering them read.
    touched: BTreeSet<String>,
    /// Where the extents they name are: each place once, however many of them name it there.
    located: BTreeMap<ExtentId, BTreeSet<Location>>,
    /// The moves they name extents in whose writers may name them anywhere on their ways from
    /// where they were sealed ([`behind`]).
    behind: BTreeSet<String>,
}

/// What deciding what goes needs of a live branch's state, its extents but: where its segments end,
/// the highest epoch of those applied, the branch's next sequence number, its fences, and whether
/// a writer is taking it over.
struct Ended {
    end: u64,
    epoch: u64,
    next_sequence: u64,
    fences: Vec<u64>,
    /// Whether its lease, read after the listing, is of an epoch past its state's: a writer taking
    /// it over, which may have read it before the last writer's segment landed, fences where it
    /// found its end, whatever covers it since (TASK-371).
    taking: bool,
}

impl Ended {
    /// fxdax: whether segment `seq` of the branch stays, whatever covers it (TASK-371): one of its
    /// fences, which a writer taken over must find for as long as it lives, its creates being its
    /// fencing (fences_kept); or any, while a writer takes the branch over (takeovers_kept).
    fn keeps(&self, seq: u64) -> bool {
        (rules().fences_kept && self.fences.binary_search(&seq).is_ok())
            || (rules().takeovers_kept && self.taking)
    }

    /// Whether segment `seq` of `branch`, which this recovery of it ended as it says, may land in
    /// it; or, with `writer`, an epoch and a sequence number, the extent that writer uploaded before
    /// that segment ([`Pass::may_land`]).
    async fn lands(
        &self,
        bucket: &impl Bucket,
        branch: Branch,
        seq: u64,
        writer: Option<(u64, u64)>,
    ) -> Result<bool, Error> {
        let Some((epoch, sequence)) = writer else { return Ok(seq >= self.end) };
        // The epoch of the writer whose segments land in the branch: of the last one in it.
        let current = if rules().orphans_after_successor {
            self.epoch
        } else {
            branch::lease_epoch(bucket, &layout::lease(branch)).await?
        };
        let ahead = match rules().extent_keys_name_segments {
            true => seq >= self.end,
            false => sequence >= self.next_sequence,
        };
        Ok(epoch >= current && (epoch != self.epoch || ahead))
    }
}

/// The states forks inherited, as their recoveries read them, each with the extents of it every
/// fork inheriting it holds no longer where it inherited it: what no fork of a round recovered
/// needs of it.
#[derive(Default)]
struct Inherited {
    states: Vec<(Arc<State>, BTreeSet<ExtentId>)>,
}

impl Live {
    /// Whether `branch` was recovered, or is being, from `origin`: or from any, without the rule
    /// that a branch is recovered by each origin it is named by (TASK-173).
    fn recovered(&self, branch: Branch, origin: Option<Origin>) -> bool {
        match rules().recoveries_by_origins {
            true => self.taken.contains(&(branch, origin)),
            false => self.taken.range((branch, None)..).next().is_some_and(|&(b, _)| b == branch),
        }
    }

    /// Takes the recovery of `branch` from `origin`, unless it was taken: returns whether it was
    /// not.
    fn take(&mut self, branch: Branch, origin: Option<Origin>) -> bool {
        !self.recovered(branch, origin) && self.taken.insert((branch, origin))
    }

    /// Folds in `recovered`, a live branch's, whose lease, read after it, is of epoch `lease`: its
    /// own layer, and what it inherited into `inherited`, its round's, which [`Live::inherit`]
    /// folds in once for all its forks.
    fn add(&mut self, recovered: Recovered<Layers>, lease: u64, inherited: &mut Inherited) {
        let behind = behind(&recovered, lease);
        let Recovered { state, end, touched, .. } = recovered;
        let State { branch, extents, epoch, next_sequence, fences, .. } = state;
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
        let taking = lease > epoch;
        let ended = Ended { end, epoch, next_sequence, fences, taking };
        self.branches.entry(branch).or_default().push(ended);
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

/// Whether `extents`, live in an object of `len` bytes of data, take less than `options`' fraction
/// of it.
fn below(extents: &BTreeMap<ExtentId, Location>, options: &Options, len: u64) -> bool {
    let used: u64 = extents.values().map(|location| location.len).sum();
    (used as f64) < options.compact_below * len as f64
}

/// The lengths of the data of the objects a pass may compact, as their trailers say, by their keys,
/// with the versions and sizes they were listed at: read once each, as they never change, and
/// forgotten once they are gone. The live extents of an object are measured against its data,
/// which its listed size, compressed (TASK-29), may be far below: before, a mostly dead segment
/// whose data compressed was never compacted (TASK-293).
#[derive(Default)]
struct Lengths(BTreeMap<String, (Version, u64, u64)>);

impl Lengths {
    /// The length of the data of the object `meta` lists, if it is still there.
    fn known(&self, meta: &Meta) -> Option<u64> {
        let (version, size, len) = self.0.get(&meta.key)?;
        (*version == meta.version && *size == meta.size).then_some(*len)
    }

    /// Learns the lengths of the data of the objects `metas` list, those not known yet, from their
    /// trailers, many at once.
    async fn learn(&mut self, bucket: &impl Bucket, metas: &[&Meta]) -> Result<(), Error> {
        let unknown = metas.iter().filter(|meta| self.known(meta).is_none());
        let reads = futures::stream::iter(unknown)
            .map(|meta| async move { (meta, trailer_len(bucket, meta).await) })
            .buffer_unordered(TRAILERS_AT_ONCE)
            .collect::<Vec<_>>()
            .await;
        for (meta, len) in reads {
            if let Some(len) = len? {
                self.0.insert(meta.key.clone(), (meta.version.clone(), meta.size, len));
            }
        }
        Ok(())
    }

    /// Forgets those of objects no longer listed as they were.
    fn retain(&mut self, objects: &BTreeMap<String, Meta>) {
        self.0.retain(|key, (version, size, _)| {
            objects.get(key).is_some_and(|meta| meta.version == *version && meta.size == *size)
        });
    }
}

/// The length of the data of the object `meta` lists, as its trailer says, if it is still there.
async fn trailer_len(bucket: &impl Bucket, meta: &Meta) -> Result<Option<u64>, Error> {
    let range = ByteRange::Suffix(layout::TRAILER);
    let Some(trailer) = bucket::get_range(bucket, &meta.key, range).await? else {
        return Ok(None);
    };
    let frame = layout::Frame::of_trailer(&trailer)?;
    if frame.size() != meta.size {
        let (key, size) = (&meta.key, meta.size);
        return Err(Error::Corrupt(format!(
            "{key}: {size} bytes, {} by its trailer",
            frame.size()
        )));
    }
    Ok(Some(frame.data_len))
}

/// What a pass found: every object listed, and what the live branches need of them.
struct Pass {
    objects: BTreeMap<String, Meta>,
    records: BTreeMap<Branch, Record>,
    /// Where each recovery of each live branch ended.
    branches: BTreeMap<Branch, Vec<Ended>>,
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
        records: BTreeMap<Branch, Record>,
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

    /// fxdax: whether segment `seq` of `branch` stays whatever covers it, by any recovery of the
    /// branch ([`Ended::keeps`], TASK-371).
    fn keeps(&self, branch: Branch, seq: u64) -> bool {
        self.branches.get(&branch).into_iter().flatten().any(|ended| ended.keeps(seq))
    }

    /// fxdax: whether the object of `kind`, which no live state names, may yet be named, by what
    /// its key declares (TASK-153.7): a segment, at or past the end of its branch, live, which may
    /// land in it; an extent stored on its own, its key naming the writer that uploaded it before
    /// the segment that names it, if that segment may land in the branch, live, as no later writer's
    /// is in it yet, and none is at its place. Nothing else may. A branch recovered by several
    /// origins may by any of its recoveries (TASK-173).
    async fn may_land(&self, bucket: &impl Bucket, kind: Kind<'_>) -> Result<bool, Error> {
        let (branch, seq, writer) = match kind {
            Kind::Segment { branch, seq } => (branch, seq, None),
            Kind::Extent { branch, epoch, seq, sequence } => (branch, seq, Some((epoch, sequence))),
            _ => return Ok(false),
        };
        for ended in self.branches.get(&branch).into_iter().flatten() {
            if ended.lands(bucket, branch, seq, writer).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The objects mostly freed, as the lengths of their data say ([`Lengths`]): those
    /// [`Pass::compact`] compacts. With the live extents of each, which take less than `options`'
    /// fraction of it.
    async fn compactable(
        &self,
        bucket: &impl Bucket,
        options: &Options,
        lengths: &mut Lengths,
    ) -> Result<Vec<(&Meta, &BTreeMap<ExtentId, Location>)>, Error> {
        let mut ours = Vec::new();
        for (key, extents) in &self.named {
            let collectable = match layout::kind(key) {
                kind @ Kind::Segment { .. } => {
                    !self.touched.contains(key) && !self.may_land(bucket, kind).await?
                }
                Kind::Moves { .. } => true,
                _ => false,
            };
            if collectable && !self.objects.contains_key(&layout::moves(key)) {
                ours.push((&self.objects[key], extents));
            }
        }
        lengths.learn(bucket, &ours.iter().map(|(meta, _)| *meta).collect::<Vec<_>>()).await?;
        // Those gone since they were listed, whose lengths it could not learn, are not compacted.
        ours.retain(|(meta, extents)| {
            lengths.known(meta).is_some_and(|len| below(extents, options, len))
        });
        Ok(ours)
    }

    /// Compacts the objects mostly freed: copies the live extents of each to its moves.
    async fn compact(
        &self,
        bucket: &impl Bucket,
        options: &Options,
        lengths: &mut Lengths,
        report: &mut Report,
    ) -> Result<(), Error> {
        for (meta, extents) in self.compactable(bucket, options, lengths).await? {
            let (key, moves) = (&meta.key, layout::moves(&meta.key));
            let Some(object) = bucket::get(bucket, key).await else { continue };
            // Its data, checked: compacting copies none that is corrupt under new CRCs.
            let object_data = layout::data(&object.bytes)?;
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

    /// What goes: everything listed that the live branches do not need, in the order of its keys.
    async fn doomed(&self, bucket: &impl Bucket, options: &Options) -> Result<Vec<&Meta>, Error> {
        let now = bucket.now();
        let aged = |meta: &Meta| meta.modified.saturating_add(options.grace) <= now;
        let moved_long_ago = |key: &str| self.objects.get(&layout::moves(key)).is_some_and(aged);
        let mut doomed = Vec::new();
        for (key, meta) in &self.objects {
            let named = self.named.contains_key(key);
            let kind = layout::kind(key);
            let goes = match kind {
                Kind::Checkpoint { .. } => !self.touched.contains(key),
                // Once a checkpoint covers it, if its extents are dead or moved; but a fence, or one
                // of a branch being taken over ([`Ended::keeps`]).
                Kind::Segment { branch, seq } => {
                    !self.may_land(bucket, kind).await?
                        && !self.keeps(branch, seq)
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
                // A deleted branch's record stays ([`Record::may_drop`], TASK-174). A lease goes
                // with its branch: writers do not take a deleted one.
                Kind::Record { branch } => {
                    self.records.get(&branch).is_some_and(Record::may_drop)
                        && !self.touched.contains(key)
                }
                Kind::Lease { branch } => {
                    !self.records.get(&branch).is_some_and(|record| !record.deleted)
                }
                // The collector's own: its lease, and its marks, which a mark-only pass never
                // deletes, and an operator removes (TASK-264). fxdax: a cell's object, which says
                // who holds it, stays as its records do, and a block of object IDs taken for good (TASK-364).
                Kind::GcLease
                | Kind::Marks { .. }
                | Kind::Cell { .. }
                | Kind::ObjectIds { .. }
                | Kind::Unknown => false,
            };
            if goes {
                doomed.push(meta);
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
    use crate::layout::test_branch;
    use crate::layout::{Child, child};
    use crate::store::Store;
    use crate::testing::{Direct, SimBucket};
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (freed, kept) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let own = vec![(freed.sequence, b"freed".to_vec()), (kept.sequence, b"kept".to_vec())];
            commit(&writer, Commit { tag: 1, own, ..Commit::default() }).await;
            let gone = writer.location(freed).unwrap().key;
            commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
            let (seq, next) = (writer.chain().next_seq(), writer.chain().next_sequence());
            let ahead =
                layout::extent(layout::TEST_CELL, writer.epoch(), seq, ExtentId::new(1, next));
            let bytes = layout::with_footer(b"ahead", &());
            assert!(matches!(bucket::create(&*bucket, &ahead, bytes).await, Created::Created));

            let before = keys(&*bucket).await;
            let report = collect(&*bucket, 1, &KEEP).await.unwrap();
            assert_eq!(report, Report { deleted: 1, compacted: 0 });
            let left: Vec<_> = before.into_iter().filter(|key| *key != gone).collect();
            assert_eq!(keys(&*bucket).await, left);
            assert!(left.contains(&ahead) && left.contains(&layout::record(test_branch(1))));
        });
    }

    /// TASK-141.2: a pass finding a branch of a newer format stops, deleting nothing, though what
    /// the other branches freed would go: what that branch needs, it can't read.
    #[test]
    fn passes_stop_at_branches_of_newer_formats() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let freed = ExtentId::new(1, writer.chain().next_sequence());
            let own = vec![(freed.sequence, b"freed".to_vec())];
            commit(&writer, Commit { tag: 1, own, ..Commit::default() }).await;
            commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
            let mut newer = (layout::FORMAT + 1).to_le_bytes().to_vec();
            newer.extend_from_slice(b"what a later format holds");
            let created = bucket::create(&*bucket, &layout::record(test_branch(2)), newer).await;
            assert!(matches!(created, Created::Created));

            let before = keys(&*bucket).await;
            let refused = collect(&*bucket, 1, &KEEP).await;
            assert_eq!(
                refused,
                Err(Error::Format { branch: test_branch(2), format: layout::FORMAT + 1 })
            );
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
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
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
        let segment = layout::segment(test_branch(1), writer.chain().next_seq());
        let created = bucket::create(&*bucket, &segment, objects.segment.read().unwrap()).await;
        assert!(matches!(created, Created::Created));
        let recovered = recovery::recover(&*bucket, test_branch(1), None).await.unwrap();
        let location = &recovered.state.extents[&late];
        crate::read(&bucket, late, location).await
    }

    /// An object compacted is compacted once: its moves stand while it waits out the grace
    /// period. A live branch's record stays.
    #[test]
    fn objects_are_compacted_once() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
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
            assert!(after.contains(&layout::record(test_branch(1))));
            let report = collect(&*bucket, 2, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 0, compacted: 0 });
            assert_eq!(keys(&*bucket).await, after);
            let location = writer.location(small).unwrap();
            assert_eq!(crate::read(&bucket, small, &location).await.unwrap(), b"small");
        });
    }

    /// TASK-293: a segment whose data compresses, its live extent a third of its data but more than
    /// half its stored bytes, is compacted: the collector measures what is live against its data,
    /// not its stored size, as it did before, leaving it uncompacted for as long as the extent
    /// lived.
    #[test]
    fn compressed_objects_are_compacted_by_their_data() {
        block_on(async {
            let bucket = store();
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (kept, freed) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let extents = vec![(kept, vec![b'k'; 4096]), (freed, vec![b'f'; 8192])];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            let segment = writer.location(kept).unwrap().key;
            commit(&writer, Commit { tag: 2, frees: vec![freed], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
            let stored = bucket.head(&segment).await.unwrap().size;
            assert!(stored < 2 * 4096, "{segment}: {stored} bytes stored");

            let compacting = Options { compact_below: 0.5, ..KEEP };
            let report = collect(&*bucket, 1, &compacting).await.unwrap();
            assert_eq!(report.compacted, 1);
            let after = keys(&*bucket).await;
            assert!(after.contains(&segment) && after.contains(&layout::moves(&segment)));
            let location = writer.location(kept).unwrap();
            assert_eq!(crate::read(&bucket, kept, &location).await.unwrap(), vec![b'k'; 4096]);
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
                branch::create(&*bucket, test_branch(1), 0).await.unwrap();
                let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
                let first = writer.chain().next_sequence();
                let (kept, freed) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
                // The kept extent's bytes compress, or not; the freed one's don't, from a xorshift
                // generator, so that the part is stored raw where the kept one's don't either.
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
                bucket.put(&segment, bytes.into(), PutMode::Overwrite).await.unwrap();

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
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let before = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let Compacted { kept, moved, sealed } = compacted(&*bucket, &before).await;
        let later = ExtentId::new(1, before.chain().next_sequence());
        let extents = vec![(later, b"later".to_vec())];
        commit(&before, Commit { tag: 3, extents, ..Commit::default() }).await;

        // The next writer recovers the branch from the checkpoint before the compaction.
        let writer = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
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
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Arc::new(Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap());
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
        let extents = vec![
            (kept, b"kept".to_vec()),
            (moved, vec![b'm'; 20_000]),
            (freed, vec![b'f'; 20_000]),
        ];
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
    /// Records are kept since (TASK-174): with them, the recovery finds the parent's segments gone
    /// instead, and starts again as for any segment gone; this is what it does should a record go.
    #[test]
    fn forks_recover_past_parents_records_their_own_checkpoints_let_go() {
        crate::set_rules(crate::Rules { records_kept: false, ..crate::Rules::ALL });
        let recovered = block_on(recover_as_a_fork_checkpoints());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(recovered, Ok(vec![ExtentId::new(1, 1 << 40)]));
        assert_eq!(block_on(recover_as_a_fork_checkpoints()), Ok(vec![ExtentId::new(1, 1 << 40)]));
    }

    /// A recovery that finds what it inherited lost not looking for a checkpoint that came since:
    /// it fails on the record.
    #[test]
    fn forks_recovering_past_parents_records_their_own_checkpoints_let_go_need_newest_checkpoints()
    {
        let rules =
            crate::Rules { newest_checkpoint: false, records_kept: false, ..crate::Rules::ALL };
        crate::set_rules(rules);
        let recovered = block_on(recover_as_a_fork_checkpoints());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(recovered, Err(Error::Lost(layout::record(test_branch(1)))));
    }

    /// The extents a fork's recovery finds, its parent deleted, the fork's writer checkpointing and
    /// a pass of the collector running as the recovery reads the parent's record.
    async fn recover_as_a_fork_checkpoints() -> Result<Vec<ExtentId>, Error> {
        use crate::layout::{Contents, Origin, Segment};
        use crate::writer::Objects;
        let direct = Arc::new(Direct(SimBucket::new()));
        let bucket =
            Arc::new(Hooked::new(direct.clone(), Hook::Getting(layout::record(test_branch(1)), 1)));
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let parent = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let extent = ExtentId::new(1, parent.chain().next_sequence());
        let extents = vec![(extent, b"held".to_vec())];
        commit(&parent, Commit { tag: 1, extents, ..Commit::default() }).await;
        let footer = parent
            .chain()
            .close(Contents { forks: vec![child(test_branch(2))], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = parent.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        parent.upload(at).await.unwrap();
        parent.acknowledge().await.unwrap();
        branch::delete(&*bucket, test_branch(1), 11, u64::MAX).await.unwrap();
        let forked = Arc::new(Writer::open(bucket.clone(), test_branch(2), 12, 0).await.unwrap());
        let passing = direct.clone();
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            forked.checkpoint().now_or_never().unwrap().unwrap();
            collect(&*passing, 1, &KEEP).now_or_never().unwrap().unwrap();
        }));
        let recovered = recovery::recover(
            &*bucket,
            test_branch(2),
            Some(Origin { parent: test_branch(1), at }),
        )
        .await?;
        Ok(recovered.state.extents.keys().copied().collect())
    }

    /// TASK-178: a deletion that finds a branch live, as a placement cut off does once the latest
    /// placement's deletion ended the lease, recovers it to give its forks' children their records;
    /// that other deletion finishing meanwhile, and a pass reclaiming what recovering the branch
    /// needed, a fork's, its parent's segments up to the fork among it, the deletion is done, as
    /// the other gave the children their records. Found by the simulation, seed 3953 of its
    /// `stale_placements` at SIM_SOAK=100, on main too.
    #[test]
    fn deletions_finished_by_others_meanwhile_are_done() {
        assert_eq!(block_on(delete_as_another_deletion_finishes()), Ok(()));
    }

    /// What deleting a fork gives, another deletion of it finishing and a pass of the collector
    /// running as it lists the fork's checkpoints, recovering it.
    async fn delete_as_another_deletion_finishes() -> Result<(), Error> {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(1), 0).await.unwrap();
        let parent = Writer::open(direct.clone(), test_branch(1), 10, 0).await.unwrap();
        let extent = ExtentId::new(1, parent.chain().next_sequence());
        let extents = vec![(extent, b"held".to_vec())];
        commit(&parent, Commit { tag: 1, extents, ..Commit::default() }).await;
        let footer = parent
            .chain()
            .close(Contents { forks: vec![child(test_branch(2))], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = parent.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        parent.upload(at).await.unwrap();
        parent.acknowledge().await.unwrap();
        commit(&parent, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
        parent.checkpoint().await.unwrap();
        Writer::open(direct.clone(), test_branch(2), 11, 0)
            .await
            .unwrap()
            .checkpoint()
            .await
            .unwrap();
        let passing = direct.clone();
        let bucket = Meanwhile {
            bucket: direct,
            branch: test_branch(2),
            meanwhile: std::sync::Mutex::new(Some(move || {
                branch::delete(&*passing, test_branch(2), 13, u64::MAX)
                    .now_or_never()
                    .unwrap()
                    .unwrap();
                collect(&*passing, 1, &KEEP).now_or_never().unwrap().unwrap();
            })),
        };
        branch::delete(&bucket, test_branch(2), 12, u64::MAX).await
    }

    /// TASK-179: a writer of a branch deleted meanwhile, whose lease the collector removed and a
    /// taking created again, at the writer's epoch, before finding the branch deleted, acknowledges
    /// nothing of a fork: the lease is another taking's. Found by the simulation, seed 9030 of its
    /// `bases_deleted_under_live_forks` at SIM_SOAK=100, where such a writer gave its forks'
    /// children records naming what the collector had reclaimed.
    #[test]
    fn writers_acknowledge_nothing_once_their_epoch_is_taken_again() {
        assert_eq!(block_on(acknowledge_past_a_lease_taken_again()), Err(Error::Fenced));
    }

    /// A writer finding the lease its by its epoch alone: it acknowledges its segment.
    #[test]
    fn writers_acknowledging_nothing_once_their_epoch_is_taken_again_need_leases_by_takings() {
        crate::set_rules(crate::Rules { leases_by_takings: false, ..crate::Rules::ALL });
        let acknowledged = block_on(acknowledge_past_a_lease_taken_again());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(acknowledged, Ok(Some(1)));
    }

    /// What a writer's acknowledgement of its segment of a fork gives, uploaded before its branch
    /// was deleted, a pass removed its lease, and a taking that read the record before the deletion
    /// took the lease again. A segment of a fork, which a writer acknowledges only once a read of
    /// its lease shows it its (forks_after_lease): what it creates is its fencing otherwise
    /// (TASK-371).
    async fn acknowledge_past_a_lease_taken_again() -> Result<Option<u64>, Error> {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        let bucket = store();
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let footer = writer
            .chain()
            .close(Contents { forks: vec![child(test_branch(2))], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let seq = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        writer.upload(seq).await.unwrap();
        let record = branch::record(&*bucket, test_branch(1)).await.unwrap().unwrap();
        let before = recovery::recover(&*bucket, test_branch(1), None).await.unwrap();
        branch::delete(&*bucket, test_branch(1), 11, u64::MAX).await.unwrap();
        collect(&*bucket, 1, &KEEP).await.unwrap();
        assert!(!keys(&*bucket).await.contains(&layout::lease(test_branch(1))));
        let taken = Writer::open_recovered(bucket.clone(), record, before, 12, 0).await;
        assert_eq!(taken.err(), Some(Error::Deleted));
        assert_eq!(
            branch::lease_epoch(&*bucket, &layout::lease(test_branch(1))).await,
            Ok(writer.epoch())
        );
        writer.acknowledge().await
    }

    /// TASK-371: a writer taken over, which acknowledges what it creates with no read of its lease,
    /// finds its create failing at its successor's fence, though the successor's checkpoint covers
    /// it and a pass ran: the collector keeps it. Its commit is not acknowledged, and the branch
    /// holds the successor's.
    #[test]
    fn writers_taken_over_find_their_successors_fences_kept() {
        let (uploaded, acknowledged, last) = block_on(create_where_a_successor_fenced());
        assert_eq!(
            (uploaded, acknowledged, last),
            (Err(Error::Fenced), Err(Error::Fenced), Some(2))
        );
    }

    /// The collector deleting the fence: the writer taken over creates its segment in its place,
    /// and acknowledges a commit the branch doesn't hold (specs/bucket's NoFencesKept).
    #[test]
    fn writers_taken_over_finding_their_successors_fences_need_fences_kept() {
        crate::set_rules(crate::Rules { fences_kept: false, ..crate::Rules::ALL });
        let created = block_on(create_where_a_successor_fenced());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(created, (Ok(()), Ok(Some(1)), Some(2)));
    }

    /// What a writer taken over gets of the upload and the acknowledgement of its commit, made
    /// after its successor fenced the branch, committed and checkpointed, and a pass ran; and the
    /// branch's last commit then.
    async fn create_where_a_successor_fenced()
    -> (Result<(), Error>, Result<Option<u64>, Error>, Option<u64>) {
        let bucket = store();
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let taken = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let successor = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
        commit(&successor, Commit { tag: 2, ..Commit::default() }).await;
        successor.checkpoint().await.unwrap();
        collect(&*bucket, 1, &KEEP).await.unwrap();
        let objects =
            Batch::from(Commit { tag: 1, ..Commit::default() }).close(&mut taken.chain()).frame();
        let seq = taken.add_segment(objects).unwrap();
        let uploaded = taken.upload(seq).await;
        let acknowledged = taken.acknowledge().await;
        let last =
            recovery::recover(&*bucket, test_branch(1), None).await.unwrap().state.last_commit;
        (uploaded, acknowledged, last)
    }

    /// TASK-371: a writer taking a branch over, which read it before the last writer's segment
    /// landed, finds the segment where it fences, though the last writer acknowledged and
    /// checkpointed it and a pass ran before it fenced: the collector keeps a branch's segments
    /// while its lease is of an epoch past its state's. Found by specs/bucket's TLA+, the model of
    /// NoFencesKept with fences_kept in.
    #[test]
    fn writers_taking_over_find_what_landed_after_they_read() {
        assert_eq!(block_on(take_over_past_a_pass()), Some(1));
    }

    /// The collector deleting it, the checkpoint covering it: the fence lands in its place, and
    /// the commit the last writer acknowledged is in no history (specs/bucket's NoTakeoversKept).
    #[test]
    fn writers_taking_over_finding_what_landed_after_they_read_need_takeovers_kept() {
        crate::set_rules(crate::Rules { takeovers_kept: false, ..crate::Rules::ALL });
        let last = block_on(take_over_past_a_pass());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(last, None);
    }

    /// The last commit of a writer taking branch 1 over, its last read before it fences, of the
    /// segments' listing, after the last writer's segment, in flight as it read the branch,
    /// landed, was acknowledged and checkpointed, and a pass ran.
    async fn take_over_past_a_pass() -> Option<u64> {
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(1), 0).await.unwrap();
        let last = Writer::open(direct.clone(), test_branch(1), 10, 0).await.unwrap();
        let objects =
            Batch::from(Commit { tag: 1, ..Commit::default() }).close(&mut last.chain()).frame();
        let seq = last.add_segment(objects).unwrap();
        let taking = Arc::new(Hooked::new(
            direct.clone(),
            Hook::Listing(layout::segments(test_branch(1)), 1),
        ));
        let passing = direct.clone();
        *taking.then.lock().unwrap() = Some(Box::new(move || {
            last.upload(seq).now_or_never().unwrap().unwrap();
            assert_eq!(last.acknowledge().now_or_never().unwrap(), Ok(Some(seq)));
            last.checkpoint().now_or_never().unwrap().unwrap();
            collect(&*passing, 1, &KEEP).now_or_never().unwrap().unwrap();
        }));
        let taken = Writer::open(taking.clone(), test_branch(1), 11, 0).await.unwrap();
        assert!(taking.then.lock().unwrap().is_none(), "the pass ran");
        taken.state().last_commit
    }

    /// TASK-180: a pass that read a point live, its records listed again finding a fork being made
    /// of it after another pass took the collector's lease, the point deleted and what it named
    /// reclaimed meanwhile, finds what the fork names lost: fenced, it stops, deleting nothing, as
    /// it would before deleting. Found by the simulation, seed 13797 of its `restores` at
    /// SIM_SOAK=100.
    #[test]
    fn passes_fenced_finding_forks_of_points_lost_stop() {
        assert_eq!(block_on(pass_fenced_as_a_point_goes()), Ok(Report::default()));
    }

    /// A pass failing on what it finds lost, fenced or not: it fails.
    #[test]
    fn passes_fenced_finding_forks_of_points_lost_stopping_need_passes_fenced_stop() {
        crate::set_rules(crate::Rules { passes_fenced_stop: false, ..crate::Rules::ALL });
        let passed = block_on(pass_fenced_as_a_point_goes());
        crate::set_rules(crate::Rules::ALL);
        let lost = Error::Lost("branch 1/1/4: what its recovery starts from keeps going".into());
        assert_eq!(passed, Err(lost));
    }

    /// What a pass gives, a point of branch 1 deleted, a later pass reclaiming what it named, and
    /// a fork of the point being made to branch 4, as the pass lists the records a second time,
    /// having read the point's before.
    async fn pass_fenced_as_a_point_goes() -> Result<Report, Error> {
        use crate::layout::{Contents, Origin, Segment};
        use crate::writer::Objects;
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(direct.clone(), test_branch(1), 10, 0).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        let extents = vec![(extent, b"held".to_vec())];
        commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
        let point = Child { branch: test_branch(3), owner: 0x33, point: true };
        let footer = writer.chain().close(Contents { forks: vec![point], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        writer.upload(at).await.unwrap();
        writer.acknowledge().await.unwrap();
        commit(&writer, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
        let bucket = Hooked::new(direct.clone(), Hook::Listing(layout::TENANTS.into(), 2));
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            let origin = Origin { parent: test_branch(1), at };
            branch::delete(&*direct, test_branch(3), 11, u64::MAX).now_or_never().unwrap().unwrap();
            collect(&*direct, 2, &KEEP).now_or_never().unwrap().unwrap();
            let left = keys(&*direct).now_or_never().unwrap();
            assert!(
                !left.contains(&layout::segment(test_branch(1), at)),
                "the point's state stayed"
            );
            let forked = branch::fork_point(&*direct, test_branch(4), test_branch(3), origin, 0x44)
                .now_or_never()
                .unwrap();
            assert_eq!(forked, Err(Error::Deleted));
        }));
        collect(&bucket, 1, &KEEP).await
    }

    /// TASK-173: a pass that recovers a child by its parent's state, the child having no record as
    /// it listed them, finds on listing them again the child's record of another origin: a restore
    /// of a point took the child's ID before the parent's writer recorded its fork, which was
    /// refused. It recovers the child again, by its record's origin, and keeps what that needs, the
    /// parent's segments up to the point, which its checkpoint since covers. Found by
    /// specs/bucket's TLA+ (TASK-165.2), the model BucketPointsDeep.
    #[test]
    fn children_restored_from_points_keep_their_origins_though_their_ids_were_forked_to() {
        assert_eq!(
            block_on(pass_as_a_child_is_restored()),
            Ok(Report { deleted: 2, compacted: 0 })
        );
    }

    /// A pass recovering each branch once, by its parent's state: it deletes what the restore's
    /// child needs, which fails every pass after.
    #[test]
    fn children_restored_from_points_keeping_their_origins_need_recoveries_by_origins() {
        crate::set_rules(crate::Rules { recoveries_by_origins: false, ..crate::Rules::ALL });
        let passed = block_on(pass_as_a_child_is_restored());
        crate::set_rules(crate::Rules::ALL);
        let lost = Error::Lost("branch 1/1/3: what its recovery starts from keeps going".into());
        assert_eq!(passed, Err(lost));
    }

    /// What a pass gives after one that, having listed the records, recovers branch 1 as its writer
    /// forks point 2, a restore of the point takes child 3, and the writer forks to 3, refused, the
    /// point is deleted and the writer checkpoints, dropping it.
    async fn pass_as_a_child_is_restored() -> Result<Report, Error> {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const PARENT: u64 = 1;
        const POINT: u64 = 2;
        const CHILD: u64 = 3;
        /// Forks `writer`'s branch to `child`: at the segment it returns, acknowledged.
        async fn fork(writer: &Writer<Direct>, child: Child) -> u64 {
            let footer =
                writer.chain().close(Contents { forks: vec![child], ..Contents::default() });
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            seq
        }
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(PARENT), 0).await.unwrap();
        let writer = Writer::open(direct.clone(), test_branch(PARENT), 10, 0).await.unwrap();
        // A commit after the writer's fence, which the collector keeps (TASK-371): what the
        // restore's child needs that a pass may delete.
        commit(&writer, Commit { tag: 1, ..Commit::default() }).await;
        let restoring = direct.clone();
        let meanwhile = move || {
            let work = async {
                let point = Child { branch: test_branch(POINT), owner: 0x22, point: true };
                let at = fork(&writer, point).await;
                let origin = Origin { parent: test_branch(PARENT), at };
                branch::fork_point(
                    &*restoring,
                    test_branch(CHILD),
                    test_branch(POINT),
                    origin,
                    0x33,
                )
                .await
                .unwrap();
                fork(&writer, Child { branch: test_branch(CHILD), owner: 0x34, point: false })
                    .await;
                assert!(writer.refused(test_branch(CHILD)));
                branch::delete(&*restoring, test_branch(POINT), 20, 0).await.unwrap();
                writer.checkpoint().await.unwrap();
            };
            work.now_or_never().expect("a direct bucket answers at once");
        };
        let during = Meanwhile {
            bucket: direct.clone(),
            branch: test_branch(PARENT),
            meanwhile: Mutex::new(Some(meanwhile)),
        };
        let now = Options { compact_below: 0.0, grace: 0 };
        collect(&during, 1, &now).await?;
        collect(&*direct, 2, &now).await
    }

    /// TASK-174: a root that took the ID of a fork's child, and was deleted, keeps its record:
    /// the fork's parent's writer, which acknowledged the fork before the parent's deletion ended
    /// its lease and was slow to record the child, finds it deleted once a pass ran, and makes
    /// none. Found by specs/bucket's TLA+ (TASK-165.2), the model BucketForks.
    #[test]
    fn deleted_roots_keep_their_records_from_slow_writers_forking_to_their_ids() {
        let (record, passed) = block_on(record_a_fork_to_a_deleted_root());
        let deleted = Record { deleted: true, ..crate::record::Kind::Root.of(0x21) };
        assert_eq!((record, passed), (Some(deleted), Ok(Report { deleted: 0, compacted: 0 })));
    }

    /// The collector dropping a deleted root's record: the slow writer makes it again, a fork's
    /// child's, naming its parent's segments, which the pass deleted, and the next pass fails.
    #[test]
    fn deleted_roots_keeping_their_records_need_records_kept() {
        crate::set_rules(crate::Rules { records_kept: false, ..crate::Rules::ALL });
        let (record, passed) = block_on(record_a_fork_to_a_deleted_root());
        crate::set_rules(crate::Rules::ALL);
        let made = crate::record::Kind::Fork(Origin { parent: test_branch(1), at: 1 }).of(0x22);
        assert_eq!(
            (record, passed),
            (Some(made), Err(Error::Lost(layout::record(test_branch(1)))))
        );
    }

    /// Child 2's record, and what a pass gives, once the writer of branch 1 forked to 2, which a
    /// root took and was deleted, and recorded the fork after branch 1's deletion and a pass.
    async fn record_a_fork_to_a_deleted_root() -> (Option<Record>, Result<Report, Error>) {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const PARENT: u64 = 1;
        const CHILD: u64 = 2;
        let direct = Arc::new(Direct(SimBucket::new()));
        let (slow, open) =
            Gated::new(direct.clone(), Gate::Put(layout::record(test_branch(CHILD))));
        let slow = Arc::new(slow);
        branch::create(&*direct, test_branch(PARENT), 0).await.unwrap();
        let writer = Writer::open(slow, test_branch(PARENT), 10, 0).await.unwrap();
        branch::create(&*direct, test_branch(CHILD), 0x21).await.unwrap();
        branch::delete(&*direct, test_branch(CHILD), 11, 0).await.unwrap();
        let forked = Child { branch: test_branch(CHILD), owner: 0x22, point: false };
        let footer = writer.chain().close(Contents { forks: vec![forked], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        writer.upload(at).await.unwrap();
        // The lease read, the fork's child's record waits.
        let mut acknowledging = Box::pin(writer.acknowledge());
        assert!((&mut acknowledging).now_or_never().is_none());
        branch::delete(&*direct, test_branch(PARENT), 12, 0).await.unwrap();
        let now = Options { compact_below: 0.0, grace: 0 };
        collect(&*direct, 1, &now).await.unwrap();
        open.send(()).unwrap();
        assert_eq!((&mut acknowledging).now_or_never(), Some(Ok(Some(at))));
        let record = branch::record(&*direct, test_branch(CHILD)).await.unwrap();
        (record, collect(&*direct, 2, &now).await)
    }

    /// TASK-183: two deletions of a branch at once, a fork's segment of its writer's in flight,
    /// never acknowledged. The first recovers the branch to the gap where the segment goes and
    /// fences it there; the segment, landing, finds the fence, and the second deletion, recovering
    /// the branch as the first is about to mark it deleted, finds no fork either: a pass after
    /// lets go of nothing a child needs, as no fork's child has a record. Found by specs/bucket's
    /// TLA+ (TASK-165.2), the model BucketForks once a deletion recovered in two steps.
    #[test]
    fn deletions_at_once_fence_out_forks_landing_between_them() {
        // The first pass, after both deletions' fences, deleted all of the branch.
        let fenced = (Err(Error::Fenced), None, Ok(Report { deleted: 0, compacted: 0 }));
        assert_eq!(block_on(delete_twice_as_a_fork_lands()), fenced);
    }

    /// A deletion fencing nothing: the segment lands, the second deletion recovers past it and,
    /// reading the record live still, records the fork's child once the first marked the branch
    /// deleted and a pass, which read no record of the child, deleted its origin; which fails every
    /// pass after.
    #[test]
    fn deletions_at_once_fencing_out_forks_landing_between_them_need_deletions_fence() {
        crate::set_rules(crate::Rules { deletions_fence: false, ..crate::Rules::ALL });
        let landed = block_on(delete_twice_as_a_fork_lands());
        crate::set_rules(crate::Rules::ALL);
        let child = crate::record::Kind::Fork(Origin { parent: test_branch(1), at: 2 }).of(0);
        let lost = Error::Lost("branch 1/1/2: what its recovery starts from keeps going".into());
        assert_eq!(landed, (Ok(()), Some(child), Err(lost)));
    }

    /// What the upload of branch 1's fork to child 2 gives, landing between two deletions'
    /// recoveries of 1: the first's, which then waits to mark 1 deleted, and the second's, which
    /// then waits to record the forks it found; the first marks 1 deleted, a pass runs, and the
    /// second goes on. The child's record, and what a pass after gives.
    async fn delete_twice_as_a_fork_lands()
    -> (Result<(), Error>, Option<Record>, Result<Report, Error>) {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const PARENT: u64 = 1;
        const CHILD: u64 = 2;
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(PARENT), 0).await.unwrap();
        let writer = Writer::open(direct.clone(), test_branch(PARENT), 10, 0).await.unwrap();
        commit(&writer, Commit { tag: 1, ..Commit::default() }).await;
        let footer = writer
            .chain()
            .close(Contents { forks: vec![child(test_branch(CHILD))], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let at = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        // The first deletion, recovered, waits to mark the branch deleted.
        let (marking, mark) =
            Gated::new(direct.clone(), Gate::Put(layout::record(test_branch(PARENT))));
        let marking = Arc::new(marking);
        let mut first =
            Box::pin(async move { branch::delete(&*marking, test_branch(PARENT), 11, 0).await });
        assert!((&mut first).now_or_never().is_none());
        let landed = writer.upload(at).await;
        // The second, recovered, waits to record the fork's child, if it found the fork.
        let (recording, record) =
            Gated::new(direct.clone(), Gate::Put(layout::record(test_branch(CHILD))));
        let recording = Arc::new(recording);
        let mut second =
            Box::pin(async move { branch::delete(&*recording, test_branch(PARENT), 12, 0).await });
        let second_done = (&mut second).now_or_never();
        mark.send(()).unwrap();
        first.await.unwrap();
        let now = Options { compact_below: 0.0, grace: 0 };
        collect(&*direct, 1, &now).await.unwrap();
        // Done already, finding no fork, the second deletion dropped its gate.
        let _ = record.send(());
        let deleted = match second_done {
            Some(deleted) => deleted,
            None => second.await,
        };
        assert_eq!(deleted, Ok(()));
        let record = branch::record(&*direct, test_branch(CHILD)).await.unwrap();
        (landed, record, collect(&*direct, 2, &now).await)
    }

    /// TASK-380: two deletions of a branch at once, as TASK-183's, its writer's segment of a fork
    /// in flight, the extent of its own the segment's commit sealed uploaded ahead of it, and the
    /// branch's segments to an earlier fork kept by that fork's child. The first deletion fences
    /// the gap where the segment goes and marks the branch deleted; a pass deletes that fence, and
    /// the extent, which no live branch needs; the segment lands where the fence was, and the
    /// second deletion, which read the record live before the first marked it, recovers past it.
    /// Reading the record again, deleted, it records no child. Found by the simulation, seed 197
    /// of `branches_deleted_twice_while_forking`, once a seed replayed as it ran.
    #[test]
    fn deletions_at_once_record_no_forks_landing_where_a_pass_deleted_a_fence() {
        assert_eq!(block_on(delete_twice_as_a_fork_lands_past_a_pass()), (Ok(()), None, vec![]));
    }

    /// A deletion recording the forks it recovered whatever the record says by then: the second
    /// gives the fork's child its record, which names the extent the pass deleted.
    #[test]
    fn deletions_at_once_recording_no_forks_landing_where_a_pass_deleted_a_fence_need_rereads() {
        crate::set_rules(crate::Rules { deletions_reread: false, ..crate::Rules::ALL });
        let landed = block_on(delete_twice_as_a_fork_lands_past_a_pass());
        crate::set_rules(crate::Rules::ALL);
        let child = crate::record::Kind::Fork(Origin { parent: test_branch(1), at: 2 }).of(0);
        // Minted from the writer's lease epoch, 1.
        let extent = ExtentId::new(1, 1 << crate::EPOCH_SHIFT);
        let gone = layout::extent(layout::TEST_CELL, 1, 2, extent);
        assert_eq!(landed, (Ok(()), Some(child), vec![gone]));
    }

    /// What the second of two deletions of branch 1 gives, its writer's segment forking to child
    /// 3, with an extent of its own, landing after a pass deleted what the first left, child 2 of
    /// an earlier fork keeping the branch's segments to that fork; child 3's record, and the keys
    /// of the extents its state names that are gone.
    async fn delete_twice_as_a_fork_lands_past_a_pass()
    -> (Result<(), Error>, Option<Record>, Vec<String>) {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const PARENT: u64 = 1;
        const KEPT: u64 = 2;
        const CHILD: u64 = 3;
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(PARENT), 0).await.unwrap();
        // The writer, whose fence is its segment 0, and whose create of its segment 2 waits.
        let at = 2;
        let (writing, landing) =
            Gated::new(direct.clone(), Gate::Put(layout::segment(test_branch(PARENT), at)));
        let writer = Writer::open(Arc::new(writing), test_branch(PARENT), 10, 0).await.unwrap();
        let footer = writer
            .chain()
            .close(Contents { forks: vec![child(test_branch(KEPT))], ..Contents::default() });
        let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
        let kept = writer.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
        writer.upload(kept).await.unwrap();
        writer.acknowledge().await.unwrap();
        // A commit sealing an extent on its own, then a fork to the child, in one segment.
        let own = vec![(writer.chain().next_sequence(), b"own".to_vec())];
        let forking = Batch::from(Commit { tag: 1, own, ..Commit::default() })
            .fork(child(test_branch(CHILD)), &mut writer.chain())
            .frame();
        assert_eq!(writer.add_segment(forking).unwrap(), at);
        let mut uploading = Box::pin(writer.upload(at));
        assert!((&mut uploading).now_or_never().is_none());
        // The second deletion, the lease ended and the record read live, waits to recover.
        let (gated, open) =
            Gated::new(direct.clone(), Gate::List(layout::checkpoints(test_branch(PARENT))));
        let gated = Arc::new(gated);
        let mut second =
            Box::pin(async move { branch::delete(&*gated, test_branch(PARENT), 12, 0).await });
        assert!((&mut second).now_or_never().is_none());
        branch::delete(&*direct, test_branch(PARENT), 11, 0).await.unwrap();
        let now = Options { compact_below: 0.0, grace: 0 };
        collect(&*direct, 1, &now).await.unwrap();
        landing.send(()).unwrap();
        assert_eq!((&mut uploading).now_or_never(), Some(Ok(())));
        open.send(()).unwrap();
        let deleted = second.await;
        let record = branch::record(&*direct, test_branch(CHILD)).await.unwrap();
        let mut gone = Vec::new();
        if let Some(record) = &record {
            let state = recovery::recover(&*direct, test_branch(CHILD), record.origin())
                .await
                .unwrap()
                .state;
            for location in state.extents.values() {
                if bucket::get(&*direct, &location.key).await.is_none() {
                    gone.push(location.key.clone());
                }
            }
        }
        (deleted, record, gone)
    }

    /// A bucket whose first request its gate names waits until the gate opens: what [`Gated::new`]
    /// gives with it opens it.
    struct Gated {
        bucket: Arc<Direct>,
        gate: Gate,
        closed: Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
    }

    /// The request a [`Gated`] bucket holds.
    enum Gate {
        /// The listing of this prefix.
        List(String),
        /// The put of an object at this key.
        Put(String),
    }

    impl Gated {
        fn new(bucket: Arc<Direct>, gate: Gate) -> (Self, futures::channel::oneshot::Sender<()>) {
            let (open, closed) = futures::channel::oneshot::channel();
            (Self { bucket, gate, closed: Mutex::new(Some(closed)) }, open)
        }

        /// What the request waits for, if `now` and the gate is yet to hold one.
        fn wait(&self, now: bool) -> Option<futures::channel::oneshot::Receiver<()>> {
            now.then(|| self.closed.lock().unwrap().take()).flatten()
        }
    }

    impl Bucket for Gated {
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
            let gate = self.wait(matches!(&self.gate, Gate::List(at) if at == prefix));
            async move {
                if let Some(gate) = gate {
                    gate.await.unwrap();
                }
                self.bucket.list(prefix).await
            }
        }

        fn put(
            &self,
            key: &str,
            bytes: bytes::Bytes,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            let gate = self.wait(matches!(&self.gate, Gate::Put(at) if at == key));
            async move {
                if let Some(gate) = gate {
                    gate.await.unwrap();
                }
                self.bucket.put(key, bytes, mode).await
            }
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// A bucket that runs `then` once, at the request its hook names, counted from when `then` was
    /// set.
    struct Hooked {
        bucket: Arc<Direct>,
        hook: Hook,
        then: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// How many gets of [`Hook::Getting`]'s key, or listings of [`Hook::Listing`]'s prefix, it
        /// answered with `then` set.
        asked: Mutex<usize>,
    }

    /// Where [`Hooked`] runs what it runs.
    enum Hook {
        /// As the create of a checkpoint takes effect, before it is answered.
        Checkpointed,
        /// Before the get of the object at this key of this number, from 1.
        Getting(String, usize),
        /// Before the listing of this prefix of this number, from 1.
        Listing(String, usize),
    }

    impl Hooked {
        fn new(bucket: Arc<Direct>, hook: Hook) -> Self {
            Self { bucket, hook, then: Mutex::default(), asked: Mutex::default() }
        }

        /// What it runs, if `now` and it has yet to.
        fn then(&self, now: bool) -> Option<Box<dyn FnOnce() + Send>> {
            now.then(|| self.then.lock().unwrap().take()).flatten()
        }

        /// What it runs, if this request, one of those it counts, is the `nth` with `then` set.
        fn nth(&self, counted: bool, nth: usize) -> Option<Box<dyn FnOnce() + Send>> {
            if !counted || self.then.lock().unwrap().is_none() {
                return None;
            }
            let mut asked = self.asked.lock().unwrap();
            *asked += 1;
            let now = *asked == nth;
            drop(asked);
            self.then(now)
        }
    }

    impl Bucket for Hooked {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            if let Hook::Getting(at, nth) = &self.hook
                && let Some(then) = self.nth(at == key, *nth)
            {
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
            if let Hook::Listing(at, nth) = &self.hook
                && let Some(then) = self.nth(at == prefix, *nth)
            {
                then();
            }
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: bytes::Bytes,
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
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
        let at = |asked: &str, kind: &str| {
            asked.strip_prefix("get ").is_some_and(|key| key.contains(&format!("/{kind}/")))
        };
        [
            counted(&|asked| at(asked, "branches")),
            counted(&|asked| at(asked, "checkpoints") && whole(asked)),
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
            branch::create(&**bucket, test_branch(1), 0).await.unwrap();
            Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap()
        });
        let first = writer.chain().next_sequence();
        let extents = (0..20).map(|n| (ExtentId::new(1, first + n), vec![n as u8; 8])).collect();
        let own: Vec<_> = (20..23).map(|n| (first + n, vec![n as u8; 16])).collect();
        let frees = own.iter().map(|(sequence, _)| ExtentId::new(1, *sequence)).collect();
        let children = (2..2 + forks + deleted).map(|id| child(test_branch(id))).collect();
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
            bucket.served(branch::delete(&**bucket, test_branch(child), 20, 0)).0.unwrap();
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
        // checkpoint covers that hold no extent its branches hold but its fence (TASK-371), its
        // extents freed, and the leases and deletions' fences of its children deleted (TASK-183).
        assert_eq!(passes, [[vec![9], vec![1], vec![9]], [vec![], vec![1], vec![]]]);

        // The fork deleted: its lease and its deletion's fence go.
        bucket.served(branch::delete(&*bucket, test_branch(2), 20, 0)).0.unwrap();
        let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, 3));
        report.unwrap();
        assert_eq!(asked(&rounds), [vec![1], vec![1], vec![2]]);
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let base = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let sequence = base.chain().next_sequence();
            let extents: Vec<_> = (0..3).map(|n| ExtentId::new(1, sequence + n)).collect();
            let own = extents.iter().map(|extent| (extent.sequence, b"held".to_vec())).collect();
            let held_in = base.chain().next_seq();
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks = Contents {
                forks: children.clone().map(|id| child(test_branch(id))).collect(),
                ..Contents::default()
            };
            let footer = base.chain().close(forks);
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = base.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            base.upload(seq).await.unwrap();
            base.acknowledge().await.unwrap();
            assert_eq!(base.checkpoint().await, Ok(Some(seq)));
            commit(&base, Commit { tag: 2, frees: extents.clone(), ..Commit::default() }).await;
            base.checkpoint().await.unwrap();
            for child in children.clone() {
                let writer =
                    Writer::open(bucket.clone(), test_branch(child), 10 + child, 0).await.unwrap();
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
                .map(|&extent| {
                    keys.contains(&layout::extent(layout::TEST_CELL, 1, held_in, extent))
                })
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
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let base = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let extent = ExtentId::new(1, base.chain().next_sequence());
            let held_in = base.chain().next_seq();
            let own = vec![(extent.sequence, b"held".to_vec())];
            commit(&base, Commit { tag: 1, own, ..Commit::default() }).await;
            let forks = Contents { forks: vec![child(test_branch(2))], ..Contents::default() };
            let footer = base.chain().close(forks);
            let segment = Segment { data: Vec::new(), footer: footer.clone() }.encode().into();
            let seq = base.add_segment(Objects { footer, segment, own: Vec::new() }).unwrap();
            base.upload(seq).await.unwrap();
            base.acknowledge().await.unwrap();
            assert_eq!(base.checkpoint().await, Ok(Some(seq)));
            commit(&base, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
            base.checkpoint().await.unwrap();
            // Its host took it, at a placement later than any since, and went.
            let lease = layout::lease(test_branch(2));
            branch::take_lease(&*bucket, &lease, 11, 7).await.unwrap();
            (extent, held_in)
        });
        let kept = || {
            block_on(keys(&*bucket)).contains(&layout::extent(
                layout::TEST_CELL,
                1,
                held_in,
                extent,
            ))
        };
        block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        assert!(kept(), "what the fork inherited went while it is live");

        let live = block_on(branch::live(&*bucket)).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), [test_branch(1), test_branch(2)]);
        block_on(branch::delete(&*bucket, test_branch(2), 12, u64::MAX)).unwrap();
        block_on(collect(&*bucket, 2, &KEEP)).unwrap();
        assert!(!kept(), "what the fork inherited stayed once it was deleted");
        let live = block_on(branch::live(&*bucket)).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), [test_branch(1)]);
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
            let forked = Child { branch: test_branch(CHILD), owner, point: false };
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
                branch::create(&*bucket, test_branch(branch), 0).await.unwrap();
            }
            let parent = Writer::open(bucket.clone(), test_branch(PARENT), 10, 0).await.unwrap();
            let extent = ExtentId::new(PARENT, parent.chain().next_sequence());
            let held_in = parent.chain().next_seq();
            let own = vec![(extent.sequence, b"held".to_vec())];
            commit(&parent, Commit { tag: 1, own, ..Commit::default() }).await;
            let at = fork(&parent, 0x32).await;
            parent.checkpoint().await.unwrap();
            commit(&parent, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
            parent.checkpoint().await.unwrap();
            let refused = Writer::open(bucket.clone(), test_branch(REFUSED), 11, 0).await.unwrap();
            fork(&refused, 0x31).await;
            assert!(refused.refused(test_branch(CHILD)));
            refused.checkpoint().await.unwrap();
            let record = branch::record(&*bucket, test_branch(CHILD)).await.unwrap().unwrap();
            assert_eq!(record.origin(), Some(Origin { parent: test_branch(PARENT), at }));
            (extent, held_in)
        });
        block_on(collect(&*bucket, 1, &KEEP)).unwrap();
        let key = layout::extent(layout::TEST_CELL, 1, held_in, extent);
        assert!(block_on(keys(&*bucket)).contains(&key), "what the child inherited went");
    }

    /// TASK-92.7: a benchmark of a pass over a base of 100,000 extents and 100 forks of it, none
    /// with a checkpoint of its own: how long the pass takes, and the most memory the process
    /// held. `cargo nextest run -p extent_remote --run-ignored only bench_passes_over_many_forks
    /// --no-capture`, in a process of its own, which nextest gives each test.
    #[test]
    #[ignore = "a benchmark (TASK-92.7): --run-ignored only, alone"]
    fn bench_passes_over_many_forks() {
        use crate::layout::{Contents, Segment};
        use crate::writer::Objects;
        const EXTENTS: u64 = 100_000;
        const FORKS: u64 = 100;
        let bucket = Arc::new(Direct(SimBucket::new()));
        block_on(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let extents =
                (0..EXTENTS).map(|n| (ExtentId::new(1, first + n), n.to_le_bytes().to_vec()));
            let commit = Commit { tag: 1, extents: extents.collect(), ..Commit::default() };
            let objects = Batch::from(commit).close(&mut writer.chain()).frame();
            let seq = writer.add_segment(objects).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
            let footer = writer.chain().close(Contents {
                forks: (2..2 + FORKS).map(|id| child(test_branch(id))).collect(),
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
        child: Branch,
        done: AtomicBool,
    }

    impl Bucket for DeletedAt {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            if key == self.key && !self.done.swap(true, Ordering::Relaxed) {
                let record = Record { deleted: true, ..crate::record::Kind::Own.of(0) };
                let (key, bytes) = (layout::record(self.child), layout::encode(&record));
                self.bucket
                    .put(&key, bytes.into(), PutMode::Overwrite)
                    .now_or_never()
                    .unwrap()
                    .unwrap();
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
            bytes: bytes::Bytes,
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
        branch: Branch,
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
            bytes: bytes::Bytes,
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
        assert_eq!(
            read_what_a_fork_of_a_point_made_and_gone_within_a_pass_holds(),
            Ok(b"held".to_vec())
        );
    }

    /// The pass not listing the records again after it read the states: it never finds the
    /// restored branch, and reclaims what it inherited. The simulation's
    /// `collection_needs_records_listed_after_states` searches seeds for the same, which a change
    /// of the trajectories moves (TASK-175's moved it from seed 5310 to 64216 of `restores`).
    #[test]
    fn forks_of_points_made_and_gone_within_a_pass_keeping_what_they_hold_need_records_relisted() {
        crate::set_rules(crate::Rules { records_relisted: false, ..crate::Rules::ALL });
        let read = read_what_a_fork_of_a_point_made_and_gone_within_a_pass_holds();
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What the restored branch of [`forks_of_points_made_and_gone_within_a_pass_keep_what_they_hold`]
    /// reads of the extent it inherited, once the pass is done.
    fn read_what_a_fork_of_a_point_made_and_gone_within_a_pass_holds() -> Result<Vec<u8>, Error> {
        let sim = SimBucket::new();
        let direct = Arc::new(Direct(sim.clone()));
        const POINT: u64 = 2;
        const RESTORED: u64 = 3;
        let writer = block_on(async {
            branch::create(&*direct, test_branch(1), 0).await.unwrap();
            Arc::new(Writer::open(direct.clone(), test_branch(1), 10, 0).await.unwrap())
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
                                    Child { branch: test_branch(POINT), owner: 0, point: true },
                                    &mut writer.chain(),
                                )
                                .frame(),
                        )
                        .unwrap();
                    writer.upload(seq).await.unwrap();
                    writer.acknowledge().await.unwrap();
                    let origin = Origin { parent: test_branch(1), at: seq };
                    branch::fork_point(
                        &*direct,
                        test_branch(RESTORED),
                        test_branch(POINT),
                        origin,
                        0,
                    )
                    .await
                    .unwrap();
                    branch::delete(&*direct, test_branch(POINT), 20, 0).await.unwrap();
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
            branch: test_branch(1),
            meanwhile: std::sync::Mutex::new(Some(meanwhile)),
        };
        let now = Options { compact_below: 0.0, grace: 0 };
        block_on(collect(&during, 1, &now)).unwrap();
        block_on(async {
            let record = branch::record(&*direct, test_branch(RESTORED)).await.unwrap();
            let recovered =
                recovery::recover(&*direct, test_branch(RESTORED), record.unwrap().origin())
                    .await?;
            crate::read(&direct, extent, &recovered.state.extents[&extent]).await
        })
    }

    /// A live branch whose recovery finds what it needs gone fails the pass, which deletes
    /// nothing; one deleted meanwhile is live no longer, and the pass goes on.
    #[test]
    fn branches_lost_fail_the_pass_unless_deleted() {
        // Branch 2, forked from 1, which has no record; and a segment of no branch.
        let origin = Origin { parent: test_branch(1), at: 0 };
        let stray = layout::segment(test_branch(3), 0);
        let sim = SimBucket::new();
        let direct = Direct(sim.clone());
        block_on(async {
            branch::record_fork(&direct, child(test_branch(2)), origin).await.unwrap();
            let bytes = layout::with_footer(b"", &());
            assert!(matches!(bucket::create(&direct, &stray, bytes).await, Created::Created));
        });
        let before = block_on(keys(&direct));
        let lost = Error::Lost(layout::record(test_branch(1)));
        assert_eq!(block_on(collect(&direct, 1, &KEEP)), Err(lost));
        assert_eq!(block_on(keys(&direct)), before);

        let deleting = DeletedAt {
            bucket: Direct(sim),
            key: layout::record(test_branch(1)),
            child: test_branch(2),
            done: AtomicBool::new(false),
        };
        let report = block_on(collect(&deleting, 2, &KEEP)).unwrap();
        assert_eq!(report, Report { deleted: 1, compacted: 0 });
        assert!(!block_on(keys(&deleting)).contains(&stray));
    }

    /// TASK-264: what the bucket holds, every object, the collector's lease included: its key, its
    /// version, size and time, and its bytes.
    async fn held(bucket: &impl Bucket) -> Vec<(Meta, Vec<u8>)> {
        let mut held = Vec::new();
        for meta in listing(bucket).await.into_values() {
            let bytes = bucket::get(bucket, &meta.key).await.unwrap().bytes;
            held.push((meta, bytes));
        }
        held
    }

    /// TASK-264: branch 1, of a pass already, so that the collector's lease is there, then of
    /// garbage: its fence, a segment of a small extent and a large, a segment freeing the large,
    /// all covered by a checkpoint, and an extent stored on its own, freed. The segment of the
    /// small extent is mostly freed: a pass compacting below half compacts it.
    async fn garbage(bucket: &Arc<Store>) {
        branch::create(&**bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        collect(&**bucket, 1, &KEEP).await.unwrap();
        let first = writer.chain().next_sequence();
        let (small, large, own) =
            (ExtentId::new(1, first), ExtentId::new(1, first + 1), ExtentId::new(1, first + 2));
        let large_bytes: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let extents = vec![(small, b"small".to_vec()), (large, large_bytes)];
        let owned = vec![(own.sequence, b"own".to_vec())];
        commit(&writer, Commit { tag: 1, extents, own: owned, ..Commit::default() }).await;
        let frees = vec![large, own];
        commit(&writer, Commit { tag: 2, frees, ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
    }

    /// The keys `garbage` makes that a pass deletes: the segment freeing, and the extent stored on
    /// its own; not the segment of the small extent, which is live, nor the fence, which the
    /// collector keeps (TASK-371).
    fn garbage_doomed() -> Vec<String> {
        let own = layout::extent(
            layout::TEST_CELL,
            1,
            1,
            ExtentId::new(1, (1 << crate::EPOCH_SHIFT) + 2),
        );
        vec![own, layout::segment(test_branch(1), 2)]
    }

    /// TASK-264: a dry run writes nothing, not even the collector's lease, and compacts nothing; it
    /// finds doomed what a pass compacting nothing deletes, and the object such a pass compacting
    /// below half compacts.
    #[test]
    fn dry_runs_write_nothing_and_find_what_passes_delete() {
        block_on(async {
            let bucket = store();
            garbage(&bucket).await;
            let before = held(&*bucket).await;
            let options = Options { compact_below: 0.5, grace: 0 };
            let dry = Collector::new(options).dry_run(&*bucket).await.unwrap();
            assert_eq!(held(&*bucket).await, before);
            let doomed: Vec<String> = dry.doomed.iter().map(|meta| meta.key.clone()).collect();
            assert_eq!(doomed, garbage_doomed());
            let compactable: Vec<String> =
                dry.compactable.into_iter().map(|meta| meta.key).collect();
            assert_eq!(compactable, [layout::segment(test_branch(1), 1)]);
            assert!(!dry.fenced);

            let keys_before = keys(&*bucket).await;
            let now = Options { compact_below: 0.0, grace: 0 };
            let report = collect(&*bucket, 2, &now).await.unwrap();
            assert_eq!(report, Report { deleted: 2, compacted: 0 });
            let after = keys(&*bucket).await;
            let deleted: Vec<String> =
                keys_before.into_iter().filter(|key| !after.contains(key)).collect();
            assert_eq!(deleted, doomed);
        });
    }

    /// TASK-264: a dry run a pass ran during, which took the collector's lease and deleted, says it
    /// was fenced.
    #[test]
    fn dry_runs_a_pass_ran_during_say_so() {
        block_on(async {
            let direct = Arc::new(Direct(SimBucket::new()));
            branch::create(&*direct, test_branch(1), 0).await.unwrap();
            // As the dry run reads the lease again, at its end.
            let during = Hooked::new(direct.clone(), Hook::Getting(layout::GC_LEASE.into(), 2));
            let passing = direct.clone();
            *during.then.lock().unwrap() = Some(Box::new(move || {
                let pass = collect(&*passing, 9, &KEEP).now_or_never();
                assert_eq!(pass, Some(Ok(Report::default())));
            }));
            let dry = Collector::new(KEEP).dry_run(&during).await.unwrap();
            assert_eq!(dry, DryRun { fenced: true, ..DryRun::default() });
        });
    }

    /// TASK-264: a mark-only pass compacts as a pass does, deletes nothing, and marks exactly what
    /// a pass deletes; the next marks the object compacted too, its moves aged, and nothing found
    /// kept that was marked. Marks are never collected, nor marked: a pass deleting leaves them.
    #[test]
    fn mark_only_passes_compact_and_mark_what_passes_delete() {
        let compacting = Options { compact_below: 0.5, grace: 0 };
        let deleted = block_on(async {
            let bucket = store();
            garbage(&bucket).await;
            let before = keys(&*bucket).await;
            let report = collect(&*bucket, 2, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 2, compacted: 1 });
            let after = keys(&*bucket).await;
            before.into_iter().filter(|key| !after.contains(key)).collect::<Vec<_>>()
        });
        assert_eq!(deleted, garbage_doomed());
        block_on(async {
            let bucket = store();
            garbage(&bucket).await;
            let before = held(&*bucket).await;
            let mut collector = Collector::new(compacting);
            let marking = collector.mark(&*bucket, 2).await.unwrap();
            assert_eq!(marking.report, Report { deleted: 0, compacted: 1 });
            assert_eq!(marking.resurrected, Vec::<String>::new());
            let marks = marking.marks.unwrap();
            let listed: BTreeMap<String, Meta> =
                before.iter().map(|(meta, _)| (meta.key.clone(), meta.clone())).collect();
            let expected: Vec<Marked> =
                deleted.iter().map(|key| Marked::of(&listed[key])).collect();
            assert_eq!(marks, Marks { epoch: 2, grace: 0, marked: expected });
            let stored = bucket::get(&*bucket, &layout::marks(2)).await.unwrap().bytes;
            assert_eq!(layout::decode::<Marks>(&stored), Ok(marks));
            // Nothing deleted: what was there is, but the collector's lease, taken again.
            let after = held(&*bucket).await;
            let kept = |held: &[(Meta, Vec<u8>)]| -> Vec<(Meta, Vec<u8>)> {
                held.iter().filter(|(meta, _)| meta.key != layout::GC_LEASE).cloned().collect()
            };
            let mut more: Vec<String> = after.iter().map(|(meta, _)| meta.key.clone()).collect();
            more.retain(|key| !listed.contains_key(key));
            let moved = layout::moves(&layout::segment(test_branch(1), 1));
            assert_eq!(more, [layout::marks(2), moved]);
            let left: Vec<_> = kept(&after)
                .into_iter()
                .filter(|(meta, _)| listed.contains_key(&meta.key))
                .collect();
            assert_eq!(left, kept(&before));

            let next = collector.mark(&*bucket, 3).await.unwrap();
            assert_eq!(next.report, Report { deleted: 0, compacted: 0 });
            assert_eq!(next.resurrected, Vec::<String>::new());
            let marked: Vec<String> =
                next.marks.unwrap().marked.into_iter().map(|marked| marked.key).collect();
            let mut expected = garbage_doomed();
            expected.push(layout::segment(test_branch(1), 1));
            expected.sort();
            assert_eq!(marked, expected);

            let report = collect(&*bucket, 4, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 3, compacted: 0 });
            let marks: Vec<String> = (keys(&*bucket).await.into_iter())
                .filter(|key| matches!(layout::kind(key), Kind::Marks { .. }))
                .collect();
            assert_eq!(marks, [layout::marks(2), layout::marks(3)]);
        });
    }

    /// TASK-264: a pass wrong as the collector was before TASK-79.3, going by the branch's next
    /// sequence number, marks an extent uploaded ahead of the segment that names it. The segment
    /// landing, a reader starting after the pass, which reads the marks first, warns as it reads
    /// the extent, each read of it; and the next mark-only pass, right, warns that it finds needed
    /// what was marked, the same object still.
    #[test]
    fn readers_and_mark_only_passes_warn_of_what_a_wrong_pass_marked() {
        let log = crate::marks::captured();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let bucket = Arc::new(Store::new(memory.clone(), ""));
        let (late, key) = block_on(async {
            branch::create(&*bucket, test_branch(1), 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
            let first = writer.chain().next_sequence();
            let (late, early) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let extents = vec![(early, b"early".to_vec())];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            // The writer's next segment, which names `late`, its object uploaded first.
            let own = vec![(late.sequence, b"late".to_vec())];
            let objects = Batch::from(Commit { tag: 2, own, ..Commit::default() })
                .close(&mut writer.chain())
                .frame();
            let [(key, bytes)] = &objects.own[..] else { panic!("{}", objects.own.len()) };
            let created = bucket::create(&*bucket, key, bytes.read().unwrap()).await;
            assert!(matches!(created, Created::Created));

            crate::set_rules(crate::Rules {
                extent_keys_name_segments: false,
                ..crate::Rules::ALL
            });
            let marking = Collector::new(KEEP).mark(&*bucket, 1).await;
            crate::set_rules(crate::Rules::ALL);
            let marked: Vec<String> =
                marking.unwrap().marks.unwrap().marked.into_iter().map(|m| m.key).collect();
            assert_eq!(marked, [key.as_str()]);
            let segment = layout::segment(test_branch(1), writer.chain().next_seq());
            let created = bucket::create(&*bucket, &segment, objects.segment.read().unwrap()).await;
            assert!(matches!(created, Created::Created));
            (late, key.clone())
        });
        let warnings = || -> Vec<String> {
            let lines = log.lock().unwrap();
            lines.iter().filter(|line| line.starts_with("WARN")).cloned().collect()
        };
        assert_eq!(warnings(), Vec::<String>::new());

        let reader = Arc::new(Store::new(memory.clone(), ""));
        let read = block_on(async {
            let recovered = recovery::recover(&*reader, test_branch(1), None).await.unwrap();
            crate::read(&reader, late, &recovered.state.extents[&late]).await
        });
        assert_eq!(read, Ok(b"late".to_vec()));
        let kind = layout::kind(&key);
        let saying = "a reader reached what the collector's newest mark-only pass found no live \
                      branch needs, and would have deleted";
        let reached = |request: &str| {
            format!(
                "WARN extent_remote: {request} {key}: {saying} (gc/marks/0000000000000001) \
                 key={key} kind={kind:?} request={request} marks=1"
            )
        };
        // Its tail, then its data.
        let read_twice = [reached("get-range"), reached("get-range")];
        assert_eq!(warnings(), read_twice);

        let next = block_on(Collector::new(KEEP).mark(&*bucket, 2)).unwrap();
        assert_eq!(next.resurrected, [key.as_str()]);
        assert_eq!(next.marks.unwrap().marked, Vec::<Marked>::new());
        let resurrected = format!(
            "WARN fxdax-gc: RESURRECTED {key}: the mark-only pass 1 found no live branch needs it, \
             and the pass 2 finds one does: a bug in the collector, which deleting would have lost \
             it to key={key} kind={kind:?} marks=1 epoch=2"
        );
        let [tail, data] = read_twice;
        assert_eq!(warnings(), [tail, data, resurrected]);
    }
}
