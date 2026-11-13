// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The collector (`fxdax-gc`): the only thing that deletes.
//!
//! fxdax: it collects a cell at a time (doc-7, Garbage collection; TASK-395): a pass over a cell
//! takes the cell's collector lease, lists the cell's prefix, and reads, compacts and deletes what
//! is there alone, beside passes over other cells, which it neither fences nor waits for.
//! Everything it decides by is under the cell's prefix: its branches' objects, their moves, its
//! lease, and the references of forks of its branches in other cells ([`layout::reference`]). What
//! its branches inherited from other cells it reads there, and never deletes: those cells keep it,
//! by the references of the forks that inherited it. A sweep over every cell is a pass over each,
//! one after another ([`Collector::collect`]); the cells are found by delimiter listings, listing
//! none of their objects ([`crate::cell::listed`]).
//!
//! A pass takes its cell's lease, lists every object of the cell, and only then reads what
//! branches need. An object written after the listing is no candidate, which is what makes reading
//! the branches' states after it safe: what a state names was either listed, and is seen as
//! needed, or not, and is left.
//!
//! The live branches are those whose records are not deleted, and the children of forks in live
//! branches' states, of the cell, which get their records only once their forks are acknowledged.
//! What they need is what recovering them reads, the objects holding the extents their states
//! name, the moves the way to those goes through, and each one's segments past its end, which may
//! be uploaded and not yet in it. Segments go only once a checkpoint covers them, so that a reader
//! that finds one missing knows, from the checkpoints, whether it is the branch's end.
//!
//! fxdax: a fork of a branch of the cell to a child in another cell is kept by the fork point: the
//! parent's whole state at the end of the segment it was forked at, the objects and moves holding
//! the extents it names along their ways from where they were sealed, and what recovering it reads
//! (TASK-395.4). It is kept while a reference names it, whatever the child frees of it since: the
//! collector is conservative, reclaiming late rather than reading other cells to find what their
//! branches still hold. A fork point shared by many references, a point cloned many times, is
//! recovered once. The references are listed after the states, by their fork points' prefixes
//! ([`referenced`]). Forks in the live
//! branches' states to other cells keep their fork points too, as a fork's child may lack its
//! reference yet. A cell with no live branch and no reference is reclaimed whole, but its records
//! and its object; and its pass retires the references of its deleted children under their
//! parents' cells, computing them from the records' origins, never listing them: the one delete a
//! pass makes outside its cell.
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
//! A pass reads records, recovers branches, reads moves and compacts objects many at once, and
//! deletes objects in batches where the store deletes so (TASK-395.5); what forks inherited
//! together, their parent's state where they were forked, it reads once and takes once for them
//! all; and a collector reads a record again only once it changed ([`Collector`], TASK-92.7).
//!
//! fxdax: two passes delete nothing (TASK-264). A mark-only pass ([`Collector::mark_cell`]) is a
//! pass in every respect but that: it takes the cell's lease and compacts, and writes what it would
//! delete to a marks object of the cell's (`<C>gc/marks/<epoch>`, [`layout::Marks`]), which every
//! reader of the cell warns of reaching ([`crate::marks`]), and which the next such pass compares
//! what it finds doomed with: an object doomed before and kept now, the same object still, is a
//! bug, as no correct fork or recovery reaches what no live branch reached. Nothing marked goes, so
//! the bucket grows by what it would have reclaimed, the compacted objects and their moves both,
//! nor does it retire references. A dry run ([`Collector::dry_run_cell`]) writes nothing at all,
//! the lease neither: it says what a pass would delete, less what a pass would delete only for
//! compacting it, a grace period after, and what references it would retire.

use crate::bucket::{self, Bucket, ByteRange, Meta, Version};
use crate::layout::{self, Branch, Cell, Kind, Location, Marked, Marks, Moves, Origin};
use crate::moves::{Moved, MovesMaps};
use crate::record::{self, Record};
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

/// How many branches a pass recovers at once, and how many fork points' states.
const RECOVERIES_AT_ONCE: usize = 16;

/// How many of the checkpoints a pass read whole it keeps for the recoveries after: those of forks
/// of a base, recovered one after another, which start from the base's.
const CHECKPOINTS_KEPT: usize = 4;

/// How many objects a pass compacts at once, each read whole: what bounds the memory compacting
/// takes, as many objects' bytes, and their moves'.
const COMPACTIONS_AT_ONCE: usize = 8;

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
    /// The objects of the cell it deleted.
    pub deleted: usize,
    pub compacted: usize,
    /// fxdax: the references of the cell's deleted children it retired, under their parents' cells
    /// (TASK-395.4): deletes sent, each pass over a dead cell sending them again, as it can't tell
    /// those gone from those not without listing other cells.
    pub retired: usize,
}

impl std::ops::AddAssign for Report {
    fn add_assign(&mut self, other: Self) {
        self.deleted += other.deleted;
        self.compacted += other.compacted;
        self.retired += other.retired;
    }
}

/// fxdax: what a mark-only pass over a cell did ([`Collector::mark_cell`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marking {
    /// What it compacted; it deleted nothing, and retired nothing.
    pub report: Report,
    /// The marks it wrote, at [`layout::marks`] of the cell and their epoch: none if a later pass
    /// took the cell's lease from it, which stopped it as it stops a pass before deleting.
    pub marks: Option<Marks>,
    /// The objects the newest marks of the cell before named that it found kept, the same objects
    /// still: a bug, each warned of.
    pub resurrected: Vec<String>,
}

/// fxdax: what a dry run over a cell found ([`Collector::dry_run_cell`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DryRun {
    /// What a pass would delete, as listed, in the order of their keys; less what it would compact
    /// and so delete a grace period after.
    pub doomed: Vec<Meta>,
    /// What a pass would compact, as listed: what it deletes too, a grace period after, once no state's way to an extent goes through it.
    pub compactable: Vec<Meta>,
    /// The references, under other cells, a pass would retire, which may be gone already.
    pub retired: Vec<String>,
    /// Whether a pass took the cell's collector lease while it ran: what it read may be older than
    /// what that pass, deleting since, read, and what it found is no more than a guess.
    pub fenced: bool,
}

/// One pass of the collector over every cell, one after another, as `holder`, which must differ
/// from every other pass's: of a collector of its own, which remembers nothing of it
/// ([`Collector`]).
pub async fn collect(
    bucket: &impl Bucket,
    holder: u64,
    options: &Options,
) -> Result<Report, Error> {
    Collector::new(*options).collect(bucket, holder).await
}

/// fxdax: one pass of the collector over `cell`, as `holder`, which must differ from every other
/// pass's over the cell: of a collector of its own, which remembers nothing of it.
pub async fn collect_cell(
    bucket: &impl Bucket,
    cell: Cell,
    holder: u64,
    options: &Options,
) -> Result<Report, Error> {
    Collector::new(*options).collect_cell(bucket, cell, holder).await
}

/// The collector. fxdax: it remembers nothing from one pass to the next (TASK-395.4): what a pass
/// read of its cell goes with it, so that a sweep over millions of cells holds no more than the
/// cells it is passing over. A pass reads a record again only once it changed, as it lists the
/// records again (TASK-92.7); keeping what is worth keeping across passes, under the cell's prefix,
/// is TASK-395.6's.
pub struct Collector {
    options: Options,
}

/// fxdax: what a pass over a cell keeps as it reads it, and drops with it.
#[derive(Default)]
struct Remembered {
    /// The records read, by their keys, with the versions they were read at.
    records: BTreeMap<String, (String, Record)>,
    lengths: Lengths,
}

impl Collector {
    pub fn new(options: Options) -> Self {
        Self { options }
    }

    /// fxdax: a pass over every cell of `bucket`, one after another, as `holder`, which must differ
    /// from every other pass's over each ([`Collector::collect_cell`]): the cells found by
    /// delimiter listings ([`crate::cell::listed`]). A pass that fails stops none of the others;
    /// the first error is returned once all ran, what they did then unsaid. What it remembers of
    /// cells no longer there, it forgets.
    pub async fn collect(&mut self, bucket: &impl Bucket, holder: u64) -> Result<Report, Error> {
        let cells = crate::cell::listed(bucket).await;
        let mut report = Report::default();
        let mut failed = None;
        for cell in cells {
            match self.collect_cell(bucket, cell, holder).await {
                Ok(done) => report += done,
                Err(error) => drop(failed.get_or_insert(error)),
            }
        }
        failed.map_or(Ok(report), Err)
    }

    /// fxdax: one pass over `cell`, as `holder`, which must differ from every other pass's over the
    /// cell.
    pub async fn collect_cell(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
        holder: u64,
    ) -> Result<Report, Error> {
        if !passable(bucket, cell).await? {
            return Ok(Report::default());
        }
        let epoch = branch::take_lease(bucket, &layout::gc_lease(cell), holder, 0).await?;
        let mut remembered = Remembered::default();
        let Some(pass) = remembered.survey(bucket, cell, epoch, Writes::Some).await? else {
            return Ok(Report::default());
        };
        let mut report = Report::default();
        pass.compact(bucket, &self.options, &mut remembered.lengths, &mut report).await?;
        let doomed = pass.doomed(bucket, &self.options).await?;
        // A pass a later one took the lease from stops.
        if fenced(bucket, cell, epoch).await? {
            return Ok(report);
        }
        // Many at once, in batches where the store deletes so, in any order: none of it is what a
        // live branch needs, whatever else goes (TASK-395.5).
        let doomed: Vec<&str> = doomed.iter().map(|meta| meta.key.as_str()).collect();
        bucket::delete_all(bucket, &doomed).await;
        report.deleted += doomed.len();
        let retired = pass.retired();
        bucket::delete_all(bucket, &retired.iter().map(String::as_str).collect::<Vec<_>>()).await;
        report.retired += retired.len();
        Ok(report)
    }

    /// fxdax: one mark-only pass over `cell`, as `holder`, which must differ from every other
    /// pass's over the cell: a pass but for deleting, which writes what it would delete to its
    /// marks instead (TASK-264), and retires no reference.
    ///
    /// It takes the cell's collector lease, as a pass that deletes does (doc-7, Garbage collection,
    /// step 1). Its marks are ordered by it, each pass's epoch its own and later than every pass's
    /// over the cell before, so that readers, and the next mark-only pass, find the newest by their
    /// keys, and no two passes' marks share a key. A pass that deletes, which it then fences,
    /// deletes nothing, and the next deletes what it would have: a pass fenced is late, never
    /// wrong. And it stops, as a pass that deletes does (TASK-180), where it finds lost what a live
    /// branch needs once a pass that deletes took the lease from it, deleting since what it read
    /// live; a pass not holding the lease could not tell that from a bug. Fenced, it writes no
    /// marks, as a pass fenced deletes nothing: what it read may be older than what a later pass
    /// did. Compacting, which writes only create-only moves that other passes' stand beside, needs
    /// the lease no more than before.
    pub async fn mark_cell(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
        holder: u64,
    ) -> Result<Marking, Error> {
        if !passable(bucket, cell).await? {
            return Ok(Marking::default());
        }
        let epoch = branch::take_lease(bucket, &layout::gc_lease(cell), holder, 0).await?;
        let mut remembered = Remembered::default();
        let Some(pass) = remembered.survey(bucket, cell, epoch, Writes::Some).await? else {
            return Ok(Marking::default());
        };
        let mut report = Report::default();
        pass.compact(bucket, &self.options, &mut remembered.lengths, &mut report).await?;
        let doomed = pass.doomed(bucket, &self.options).await?;
        if fenced(bucket, cell, epoch).await? {
            return Ok(Marking { report, ..Marking::default() });
        }
        let resurrected =
            resurrected(bucket, cell, epoch, &self.options, &pass.objects, &doomed).await;
        let marked = doomed.into_iter().map(Marked::of).collect();
        let marks = Marks { epoch, grace: self.options.grace, marked };
        // Readers watch the cells' marks only while the bucket says it is marked: first.
        bucket::create(bucket, layout::MARKING, b"marking".to_vec()).await;
        // The epoch is this pass's taking of the lease alone: nothing else is at its key.
        bucket::create(bucket, &layout::marks(cell, epoch), layout::encode(&marks)).await;
        Ok(Marking { report, marks: Some(marks), resurrected })
    }

    /// fxdax: what a pass over `cell` would delete, writing nothing (TASK-264): not taking the
    /// cell's collector lease, which would fence a pass that deletes, running meanwhile, out of its
    /// deletes. It compacts nothing, and so finds doomed none of what a pass would delete only once
    /// it compacted it, a grace period after: no more than a pass would delete, and less. What a
    /// pass would compact, it says apart ([`DryRun::compactable`]), and what references it would
    /// retire ([`DryRun::retired`]). It goes by the lease's epoch as it starts, as a pass by the
    /// epoch it takes, to tell whether a pass that deletes ran meanwhile ([`DryRun::fenced`]).
    /// Where a pass would give up on a fork of a point deleted, writing its record, it takes the
    /// fork as given up on, as a pass mostly finds it.
    pub async fn dry_run_cell(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
    ) -> Result<DryRun, Error> {
        let epoch = branch::lease_epoch(bucket, &layout::gc_lease(cell)).await?;
        let mut remembered = Remembered::default();
        let Some(pass) = remembered.survey(bucket, cell, epoch, Writes::None).await? else {
            return Ok(DryRun { fenced: true, ..DryRun::default() });
        };
        let doomed = pass.doomed(bucket, &self.options).await?.into_iter().cloned().collect();
        let compactable = pass.compactable(bucket, &self.options, &mut remembered.lengths).await?;
        let compactable = compactable.into_iter().map(|(meta, _)| meta.clone()).collect();
        let retired = pass.retired();
        Ok(DryRun { doomed, compactable, retired, fenced: fenced(bucket, cell, epoch).await? })
    }
}

impl Remembered {
    /// What a pass over `cell` of the cell's collector lease's `epoch` finds: every object of the
    /// cell listed, and what the live branches and the fork points referred to need of them; none
    /// if a later pass took the lease from it as it found what a live branch needs lost
    /// (TASK-180). It writes what giving up on forks needs, if `writes` says so.
    async fn survey(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
        epoch: u64,
        writes: Writes,
    ) -> Result<Option<Pass>, Error> {
        let early = rules().candidates_first;
        let mut objects = if early { listing(bucket, cell).await } else { BTreeMap::new() };
        // Without the rule that they are listed after the states, with the candidates, and never
        // again, as a test shows it needed.
        let listed_first = match rules().references_relisted {
            true => None,
            false => Some(referenced(bucket, cell).await),
        };
        let checkpoints = Checkpoints::new(bucket, CHECKPOINTS_KEPT);
        let mut records = self.records(bucket, cell).await?;
        let mut live = Live::default();
        let read =
            match self.read(bucket, cell, &checkpoints, &mut records, &mut live, writes).await {
                Ok(()) => {
                    let referenced = match listed_first {
                        Some(listed) => listed,
                        None => referenced(bucket, cell).await,
                    };
                    let by = Points::Referred;
                    keep_points(bucket, &checkpoints, &mut live, &referenced, by)
                        .await
                        .map(|()| referenced)
                }
                Err(error) => Err(error),
            };
        let referenced = match read {
            Ok(referenced) => referenced,
            // Fenced, it read what the pass that fenced it, deleting since, may have found no live
            // branch needs: a point deleted, read live before, and its fork being made, read after
            // (TASK-180). It stops, as it would before deleting.
            Err(Error::Lost(_))
                if rules().passes_fenced_stop && fenced(bucket, cell, epoch).await? =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if !early {
            objects = listing(bucket, cell).await;
        }
        self.lengths.retain(&objects);
        Ok(Some(Pass::new(bucket, cell, objects, records, live, referenced).await?))
    }

    /// Reads the states of `cell`'s live branches into `live`, and of the fork points of their forks
    /// to other cells, then lists `records` again, going on with the new ones, while a state it
    /// read is from a checkpoint, until no new one appears. A pass fenced as it found what a live
    /// branch needs lost stops, where its caller sees it.
    async fn read(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
        checkpoints: &Checkpoints<'_, impl Bucket>,
        records: &mut BTreeMap<Branch, Record>,
        live: &mut Live,
        writes: Writes,
    ) -> Result<(), Error> {
        loop {
            if rules().collector_abandons {
                abandon(bucket, records, writes).await?;
            }
            let (checkpointed, points) =
                recover_live(bucket, cell, checkpoints, records, live, writes).await?;
            keep_points(bucket, checkpoints, live, &points, Points::Forked).await?;
            if !checkpointed || !rules().records_relisted {
                return Ok(());
            }
            let again = self.records(bucket, cell).await?;
            let found = again.keys().any(|branch| !records.contains_key(branch));
            records.extend(again);
            if !found {
                return Ok(());
            }
        }
    }

    /// The records of `cell`'s branches, read many at once, but those read before at the versions
    /// listed. A deleted branch's children got theirs before it was marked deleted, so listing
    /// them again after reading them finds those of any it read deleted.
    async fn records(
        &mut self,
        bucket: &impl Bucket,
        cell: Cell,
    ) -> Result<BTreeMap<Branch, Record>, Error> {
        let mut records = BTreeMap::new();
        loop {
            let listed = (bucket::list(bucket, &layout::records(cell)).await.into_iter())
                .filter_map(|meta| match layout::kind(&meta.key) {
                    Kind::Record { branch } if !records.contains_key(&branch) => {
                        Some((branch, meta))
                    }
                    _ => None,
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

/// fxdax: whether a pass over `cell` has anything to go by, checked before it takes the cell's
/// lease, which would be the cell's one object otherwise: the format of the cell's object, a GET,
/// which a pass of another format refuses ([`Error::Format`]) before writing anything. A cell with
/// none, its branches made without one, its first record's format; and, holding no record, whether
/// it holds anything (a reference, what a dead cell left).
async fn passable(bucket: &impl Bucket, cell: Cell) -> Result<bool, Error> {
    if crate::cell::read(bucket, cell).await?.is_some() {
        return Ok(true);
    }
    let listed = bucket::list(bucket, &layout::records(cell)).await;
    for meta in &listed {
        let Kind::Record { branch } = layout::kind(&meta.key) else { continue };
        if let Some(object) = bucket::get(bucket, &meta.key).await {
            Record::decode(branch, &object.bytes)?;
            return Ok(true);
        }
    }
    let listing = bucket::list_delimited(bucket, &cell.prefix()).await;
    Ok(!listing.objects.is_empty() || !listing.prefixes.is_empty())
}

/// fxdax: whether a pass writes to the bucket what giving up on forks needs: a dry run writes
/// nothing (TASK-264).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writes {
    Some,
    None,
}

/// fxdax: the objects the newest marks of `cell` before those of `epoch` named, that `objects`,
/// listed by the mark-only pass of `epoch`, holds still, the same objects, and that `doomed`, what
/// it found no live branch needs, does not: each warned of, loudly. No correct fork or recovery
/// reaches what no live branch reached, so each is a bug in the collector, which a pass deleting
/// would have lost. Marks of a shorter grace period than `options`' are compared with none: what a
/// compacted object needs to go, its moves aged, takes longer by a longer one. Marks it can't read,
/// it warns of.
async fn resurrected(
    bucket: &impl Bucket,
    cell: Cell,
    epoch: u64,
    options: &Options,
    objects: &BTreeMap<String, Meta>,
    doomed: &[&Meta],
) -> Vec<String> {
    let listed = bucket::list(bucket, &layout::marks_of(cell)).await;
    let before = listed.iter().rev().find(
        |meta| matches!(layout::kind(&meta.key), Kind::Marks { epoch: marked, .. } if marked < epoch),
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

/// Whether a later pass over `cell` took the cell's collector lease from the pass that took it at
/// `epoch`.
async fn fenced(bucket: &impl Bucket, cell: Cell, epoch: u64) -> Result<bool, Error> {
    Ok(branch::lease_epoch(bucket, &layout::gc_lease(cell)).await? != epoch)
}

/// Gives up on the forks being made of points that were deleted, as `records` say, which it says
/// are deleted then, or made, if that came first ([`branch::abandon`]); without `writes`, takes them
/// as deleted, writing nothing. fxdax: a point of another cell's than the pass's, it reads where it
/// is.
async fn abandon(
    bucket: &impl Bucket,
    records: &mut BTreeMap<Branch, Record>,
    writes: Writes,
) -> Result<(), Error> {
    let forking: Vec<(Branch, Branch)> = (records.iter())
        .filter(|(_, record)| !record.deleted)
        .filter_map(|(&child, record)| Some((child, record.forking()?)))
        .collect();
    for (child, point) in forking {
        let gone = match records.get(&point) {
            Some(record) => record.deleted,
            None if point.cell == child.cell => true,
            None => gone(bucket, point).await?,
        };
        if !gone {
            continue;
        }
        let record = match writes {
            Writes::Some => branch::abandon(bucket, child).await?,
            Writes::None => Record { deleted: true, ..records[&child] },
        };
        records.insert(child, record);
    }
    Ok(())
}

/// fxdax: whether `point`, of another cell's, is gone: its record deleted, or none.
async fn gone(bucket: &impl Bucket, point: Branch) -> Result<bool, Error> {
    Ok(branch::record(bucket, point).await?.is_none_or(|record| record.deleted))
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

/// fxdax: every object under `cell`'s prefix, by its key, but the references of forks of its
/// branches, which are never candidates, nor named by any state.
async fn listing(bucket: &impl Bucket, cell: Cell) -> BTreeMap<String, Meta> {
    (bucket::list(bucket, &cell.prefix()).await.into_iter())
        .filter(|meta| !matches!(layout::kind(&meta.key), Kind::Reference { .. }))
        .map(|meta| (meta.key.clone(), meta))
        .collect()
}

/// fxdax: the fork points the references under `cell` name, by delimiter listings: the parents
/// with references, then each one's fork points, none of their children (doc-7, The layout). Once:
/// a listing's pages are read one after another, but what it shows is a fork point's prefix, not a
/// reference, and a clone's reference and its point's own share it, `<C>refs/<origin>/<at>/`, the
/// clone's written before the point's is retired; so the prefix holds a key all along, and a
/// listing reaching it at any time shows it (a listing of the references whole would not: the
/// clone's behind its cursor, the point's ahead, retired).
async fn referenced(bucket: &impl Bucket, cell: Cell) -> BTreeSet<Origin> {
    let mut points = BTreeSet::new();
    for prefix in bucket::list_delimited(bucket, &layout::references(cell)).await.prefixes {
        let Some(parent) = layout::referenced_parent(cell, &prefix) else { continue };
        for at in bucket::list_delimited(bucket, &prefix).await.prefixes {
            if let Some(at) = layout::referenced_at(parent, &at) {
                points.insert(Origin { parent, at });
            }
        }
    }
    points
}

/// fxdax: what keeps a fork point a pass keeps ([`keep_points`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Points {
    /// A fork of a live branch's state to a child in another cell, which may have no reference
    /// yet: its state lost is what the fork's child needs lost, which stops the pass.
    Forked,
    /// A reference: its state lost stops the pass if a child named under it lives, its record, read
    /// then, naming the fork point as its origin; else nothing was lost, the children deleted,
    /// being made of a point deleted, claims, or having no record.
    Referred,
}

/// fxdax: keeps in `live` the states of the fork points `points` it keeps none of yet, each
/// recovered once, however many forks name it, several at once ([`Live::keep_point`]): the
/// extents each names, and what recovering it read (doc-7, A pass over a cell, step 5). Those of
/// other cells are read there.
async fn keep_points(
    bucket: &impl Bucket,
    checkpoints: &Checkpoints<'_, impl Bucket>,
    live: &mut Live,
    points: &BTreeSet<Origin>,
    by: Points,
) -> Result<(), Error> {
    let unkept: Vec<Origin> =
        points.iter().filter(|&origin| live.points.insert(*origin)).copied().collect();
    let recoveries = futures::stream::iter(unkept)
        .map(|origin| async move {
            (origin, recovery::recover_at(checkpoints, origin.parent, origin.at).await)
        })
        .buffer_unordered(RECOVERIES_AT_ONCE)
        .collect::<Vec<_>>()
        .await;
    for (origin, recovered) in recoveries {
        match recovered {
            Ok((state, touched)) => live.keep_point(&state, touched),
            Err(Error::Lost(lost)) if by == Points::Referred => {
                if let Some(child) = needing(bucket, origin).await? {
                    return Err(Error::Lost(format!("{lost}, which {child} lives on")));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// fxdax: a child the references of the fork point `origin` name that lives on it: its record not
/// deleted, a fork's or a point's, naming `origin` as its origin. Its references listed, and their
/// children's records read, in their cells: asked only of a fork point whose state is lost.
async fn needing(bucket: &impl Bucket, origin: Origin) -> Result<Option<Branch>, Error> {
    let listed = bucket::list(bucket, &layout::fork_point(origin.parent, origin.at)).await;
    for meta in listed {
        let Kind::Reference { child, .. } = layout::kind(&meta.key) else { continue };
        let Some(record) = branch::record(bucket, child).await? else { continue };
        let lives = match record.kind {
            record::Kind::Fork(from) | record::Kind::Point(from) => from == origin,
            _ => false,
        };
        if lives && !record.deleted {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

/// Recovers the live branches of `cell` `live` has not yet, several at once, folding them into it:
/// those whose records are not deleted, and the children of forks in their states that have no
/// records yet, of the cell. Returns whether one has a checkpoint of its own, which a fork deleted
/// may have gone from; and the fork points of the forks in their states to children of other cells,
/// whose states the pass keeps, as those children may have no references yet. Forks of a branch at
/// one segment are recovered together, those without checkpoints of their own starting from the
/// branch's state there, read once for them ([`Checkpoints`]); and the states are folded into what
/// the pass needs of them as they are recovered, what forks inherited together once for them all,
/// rather than each fork's copy of it ([`Layers`]).
///
/// A branch is recovered once by each origin it is named by: a child its parent's state names, that
/// fork refused, has its record from another branch whose fork to its ID came first, a restore's,
/// which listing the records again may find only after the child was recovered by the state; what
/// the record's origin needs is what the child does (TASK-173).
///
/// A failed fork's claim no writer has, whose recovery finds what it names lost, it gives up,
/// writing its record so if `writes` says so; and so a fork of a point still being made, its point
/// deleted, read again then (TASK-395.4): what the pass read of it before, the point live, may be
/// older than what recovering it read.
async fn recover_live(
    bucket: &impl Bucket,
    cell: Cell,
    checkpoints: &Checkpoints<'_, impl Bucket>,
    records: &BTreeMap<Branch, Record>,
    live: &mut Live,
    writes: Writes,
) -> Result<(bool, BTreeSet<Origin>), Error> {
    let mut checkpointed = false;
    let mut points = BTreeSet::new();
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
        let recoveries = round.into_iter().map(|(origin, branch)| async move {
            // Its lease, read after the listing and its recovery: what its writer may name
            // ([`behind`]), and whether one is taking it over ([`pinned`]).
            let recovered = match recovery::recover_layers(checkpoints, branch, origin).await {
                Ok(recovered) => match branch::lease_epoch(bucket, &layout::lease(branch)).await {
                    Ok(lease) => Ok((recovered, lease)),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            (branch, recovered)
        });
        let mut inherited = Inherited::default();
        for (branch, recovered) in futures::future::join_all(recoveries).await {
            let (recovered, lease) = match recovered {
                Ok(recovered) => recovered,
                Err(Error::Lost(lost)) => match lost_is_ok(bucket, branch, writes).await? {
                    true => continue,
                    false => return Err(Error::Lost(lost)),
                },
                Err(error) => return Err(error),
            };
            // A child with a record is queued by it, as it says: one whose ID another branch took
            // as the fork was made has another origin than its parent's state says (TASK-79.82).
            // fxdax: one of another cell's, its fork point kept, which its reference, if it has one
            // yet, keeps too: the child's cell's pass recovers it (TASK-395.4).
            for fork in &recovered.state.forks {
                let origin = Origin { parent: branch, at: fork.at };
                if !rules().forks_from_states {
                    continue;
                }
                if fork.child.cell != cell {
                    points.insert(origin);
                    continue;
                }
                if records.contains_key(&fork.child) || live.recovered(fork.child, Some(origin)) {
                    continue;
                }
                queue.insert((Some(origin), fork.child));
            }
            checkpointed |= recovered.checkpoint.is_some();
            live.add(recovered, lease, &mut inherited);
        }
        live.inherit(inherited);
    }
    Ok((checkpointed, points))
}

/// Whether recovering `branch` finding what it needs lost lost nothing, as its record, read again,
/// says: deleted since it was read, what it needed may be gone; a failed fork's claim no writer
/// has, which nothing ever read, given up on; fxdax: a fork of a point still being made, its point
/// deleted, given up on (TASK-395.4), unless it was made first, as [`abandon`] would have: the rule
/// `collector_abandons` both. Gives up on them, writing their
/// records so, if `writes` says so.
async fn lost_is_ok(bucket: &impl Bucket, branch: Branch, writes: Writes) -> Result<bool, Error> {
    let Some(record) = branch::record(bucket, branch).await? else { return Ok(true) };
    if record.deleted {
        return Ok(true);
    }
    if branch::lost_is_ok(bucket, branch, &record).await? {
        if writes == Writes::Some {
            branch::abandon_claim(bucket, branch).await?;
        }
        return Ok(true);
    }
    let Some(point) = record.forking().filter(|_| rules().collector_abandons) else {
        return Ok(false);
    };
    if !gone(bucket, point).await? {
        return Ok(false);
    }
    match writes {
        Writes::Some => Ok(branch::abandon(bucket, branch).await?.deleted),
        Writes::None => Ok(true),
    }
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
    /// fxdax: the fork points whose states it keeps, each once ([`Live::keep_point`]).
    points: BTreeSet<Origin>,
}

/// fxdax: the segments of a live branch the collector keeps whatever covers them (TASK-388), as a
/// recovery of it, `state`, and the epoch of its lease, read after the listing, `lease`, show:
/// every position a writer of the branch, taken over or taking it over, may still create at. A
/// writer acknowledges what it created with no read of its lease, its creates its fencing
/// (TASK-371): it creates its segments one after another and acknowledges them in order, so it
/// acknowledges nothing past the first position where its create fails, and nothing it
/// acknowledged is lost if that position held, from when its successor read the branch, an object
/// not its own, which stays for as long as the writer may live. Those positions, for every writer:
///
/// - while a writer takes the branch over, its lease of an epoch past the state's, every segment:
///   it may have read the branch before the last writer's segment landed, and fences where it
///   found the end (`pins_takings`);
/// - every fence, of every epoch: where a writer taken over by the one that put it meets it, its
///   own segments, applied, before (`pins_fences`, `pins_old_fences`);
/// - every stray, a segment read that is neither a fence nor of the epoch of the fence before it:
///   where a writer taken over meets it before its successor's fence, landed there first, void
///   (`pins_void_strays`) or late (`pins_late_strays`).
///
/// A writer's own segments, but its fence, go once a checkpoint covers them: it creates each once.
/// Nothing says when no writer of an epoch can be left, so pins stay, a few objects a takeover.
pub fn pinned<P>(state: &State<P>, lease: u64) -> Pinned {
    if rules().pins_takings && lease > state.epoch {
        return Pinned::All;
    }
    let last = state.fences.last().copied();
    let fences = (state.fences.iter().copied())
        .filter(|&seq| rules().pins_fences && (rules().pins_old_fences || Some(seq) == last));
    let strays = (state.strays.iter())
        .filter(|stray| match stray.late {
            true => rules().pins_late_strays,
            false => rules().pins_void_strays,
        })
        .map(|stray| stray.seq);
    Pinned::Only(fences.chain(strays).collect())
}

/// The segments of a branch the collector keeps whatever covers them ([`pinned`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pinned {
    All,
    Only(BTreeSet<u64>),
}

impl Pinned {
    pub fn contains(&self, seq: u64) -> bool {
        match self {
            Pinned::All => true,
            Pinned::Only(pinned) => pinned.contains(&seq),
        }
    }
}

/// What deciding what goes needs of a live branch's state, its extents but: where its segments end,
/// the highest epoch of those applied, the branch's next sequence number, and its pins.
struct Ended {
    end: u64,
    epoch: u64,
    next_sequence: u64,
    pinned: Pinned,
}

impl Ended {
    /// fxdax: whether segment `seq` of the branch stays, whatever covers it ([`pinned`]).
    fn keeps(&self, seq: u64) -> bool {
        self.pinned.contains(seq)
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
        let pinned = pinned(&state, lease);
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
        let ended = Ended { end, epoch, next_sequence, pinned };
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

    /// fxdax: keeps `state`, a fork point's, whole, for the forks of it to other cells, which
    /// `touched` was read to recover (TASK-395.4): the extents it names, along their ways from where
    /// they were sealed, as a child with no checkpoint of its own names them where it read them;
    /// and what recovering it read, through which such a child recovers, unless the rule that it is
    /// kept is off, as a test shows it needed. What its forks freed of it since is no matter: the
    /// collector is conservative.
    fn keep_point(&mut self, state: &State, touched: BTreeSet<String>) {
        if rules().fork_points_kept {
            self.touched.extend(touched);
        }
        let behind = rules().ways_from_seals;
        for (extent, location) in &state.extents {
            if behind {
                self.behind_at(location);
            }
            let located = self.located.entry(*extent).or_default();
            if !located.contains(location) {
                located.insert(location.clone());
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

/// Compacts the object at `key`, of which `extents` are live: copies them to its moves, unless it
/// is gone. Whether it did.
async fn compact_object(
    bucket: &impl Bucket,
    key: &str,
    extents: &BTreeMap<ExtentId, Location>,
) -> Result<bool, Error> {
    let moves = layout::moves(key)?;
    let Some(object) = bucket::get(bucket, key).await else { return Ok(false) };
    // Its data, checked: compacting copies none that is corrupt under new CRCs.
    let object_data = layout::data(&object.bytes)?;
    let mut data = Vec::new();
    let mut entries = Vec::new();
    for (&extent, location) in extents {
        let range = location.offset as usize..(location.offset + location.len) as usize;
        let bytes = object_data
            .get(range)
            .ok_or_else(|| Error::Corrupt(format!("{extent:?} past the end of {key}")))?;
        let to = Location { key: moves.clone(), offset: data.len() as u64, len: location.len };
        entries.push((extent, to));
        data.extend_from_slice(bytes);
    }
    // Another pass's moves of it, if there are some, stand.
    let bytes = layout::with_footer(&data, &Moves { extents: entries });
    bucket::create(bucket, &moves, bytes).await;
    Ok(true)
}

/// What a pass over a cell found: every object of the cell listed, and what the live branches and
/// the fork points it keeps need of them.
struct Pass {
    cell: Cell,
    objects: BTreeMap<String, Meta>,
    records: BTreeMap<Branch, Record>,
    /// Where each recovery of each live branch ended.
    branches: BTreeMap<Branch, Vec<Ended>>,
    /// What recovering the live branches and the fork points read.
    touched: BTreeSet<String>,
    /// The objects holding the extents the live branches and the fork points name, and those
    /// extents in them.
    named: BTreeMap<String, BTreeMap<ExtentId, Location>>,
    /// The moves the way to those extents goes through, whose maps are needed even once their
    /// data moved on: from where the states name them, or, where a writer may name them before
    /// that, from where they were sealed.
    hops: BTreeSet<String>,
    /// fxdax: the fork points the references under the cell name, as listed after the states.
    referenced: BTreeSet<Origin>,
}

impl Pass {
    async fn new(
        bucket: &impl Bucket,
        cell: Cell,
        objects: BTreeMap<String, Meta>,
        records: BTreeMap<Branch, Record>,
        live: Live,
        referenced: BTreeSet<Origin>,
    ) -> Result<Self, Error> {
        let Live { branches, touched, located, behind, .. } = live;
        let mut named: BTreeMap<String, BTreeMap<ExtentId, Location>> = BTreeMap::new();
        // The moves on the ways there from where the extents were sealed, those named among them:
        // each moves is at the key of the moves of what it is of.
        let mut hops = BTreeSet::new();
        for key in &behind {
            let mut key = key.clone();
            while let Some(from) = layout::moved_from(&key) {
                if objects.contains_key(&key) {
                    hops.insert(key);
                }
                key = from;
            }
        }
        // Each place once, however many branches name it, followed along its moves a hop at a
        // time: the moves the ways reach at a hop read at once, as the listing says which there
        // are (TASK-395.5). fxdax: a place in another cell, and its moves, are no candidates of
        // this one's, which its listing doesn't hold (TASK-395.4).
        let mut maps = MovesMaps::default();
        let mut ways: Vec<(ExtentId, Location)> = (located.iter())
            .flat_map(|(&extent, locations)| locations.iter().map(move |at| (extent, at.clone())))
            .collect();
        while !ways.is_empty() {
            let mut onward = Vec::new();
            for (extent, location) in ways {
                if objects.contains_key(&location.key) {
                    named.entry(location.key.clone()).or_default().insert(extent, location.clone());
                }
                let moves = layout::moves(&location.key)?;
                if objects.contains_key(&moves) {
                    hops.insert(moves);
                    onward.push((extent, location));
                }
            }
            maps.read_listed(bucket, onward.iter().map(|(_, at)| at.key.clone())).await?;
            ways = (onward.into_iter())
                .filter_map(|(extent, location)| match maps.moved_read(extent, &location.key) {
                    Moved::To(to) => Some((extent, to)),
                    Moved::No | Moved::Dropped => None,
                })
                .collect();
        }
        Ok(Self { cell, objects, records, branches, touched, named, hops, referenced })
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
            if collectable && !self.objects.contains_key(&layout::moves(key)?) {
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
    /// [`COMPACTIONS_AT_ONCE`] at once, in any order (TASK-395.5): each writes the moves of an
    /// object of its own, create-only, which no other compaction of the pass reads, as an object
    /// is compacted only where no moves of it were listed, and moves only where they were, never
    /// both of one way. They all end before the pass works out what goes, as one at a time: what
    /// goes is of the listing before, which none of their moves is in.
    async fn compact(
        &self,
        bucket: &impl Bucket,
        options: &Options,
        lengths: &mut Lengths,
        report: &mut Report,
    ) -> Result<(), Error> {
        let compactable = self.compactable(bucket, options, lengths).await?;
        let compactions = futures::stream::iter(compactable)
            .map(|(meta, extents)| compact_object(bucket, &meta.key, extents))
            .buffer_unordered(COMPACTIONS_AT_ONCE);
        let mut compactions = std::pin::pin!(compactions);
        while let Some(compacted) = compactions.next().await {
            report.compacted += usize::from(compacted?);
        }
        Ok(())
    }

    /// What goes: everything listed that the live branches and the fork points kept do not need, in
    /// the order of its keys. fxdax: a cell with no live branch and no reference, all of it but its
    /// records and its object, and the collector's own.
    async fn doomed(&self, bucket: &impl Bucket, options: &Options) -> Result<Vec<&Meta>, Error> {
        let now = bucket.now();
        let aged = |meta: &Meta| meta.modified.saturating_add(options.grace) <= now;
        let moved_long_ago = |key: &str| {
            layout::moves(key).ok().and_then(|moves| self.objects.get(&moves)).is_some_and(aged)
        };
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
                Kind::Moves { cell, of } => {
                    !self.hops.contains(key)
                        && if named {
                            moved_long_ago(key)
                        } else {
                            !self.objects.contains_key(&format!("{}{of}", cell.prefix()))
                                && aged(meta)
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
                // deletes, and an operator removes (TASK-264); and references, which the passes of
                // their children's cells retire (TASK-395.4). fxdax: a cell's object, which says
                // who holds it, stays as its records do, and a block of object IDs taken for good
                // (TASK-364).
                Kind::GcLease { .. }
                | Kind::Marks { .. }
                | Kind::Marking
                | Kind::Reference { .. }
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

    /// fxdax: the references the pass retires, under other cells (doc-7, References; TASK-395.4):
    /// once its cell holds no live branch and no reference of its own, those of its deleted
    /// children of origins in other cells, each at the key its record's origin gives, never listed.
    /// Nothing anywhere then inherits from them: a fork of one of them in the cell would have a
    /// record there, and one in another cell a reference there. Without the rule that the cell's
    /// references hold them, as a test shows it needed, once each child is deleted.
    fn retired(&self) -> Vec<String> {
        let dead = self.records.values().all(|record| record.deleted)
            && self.branches.is_empty()
            && self.referenced.is_empty();
        if rules().references_held && !dead {
            return Vec::new();
        }
        (self.records.iter())
            .filter(|(_, record)| record.deleted)
            .filter_map(|(&child, record)| {
                let origin = record.origin().filter(|origin| origin.parent.cell != self.cell)?;
                Some(layout::reference(origin.parent, origin.at, child))
            })
            .collect()
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

    /// The keys of every object of the bucket but the collector's leases.
    async fn keys(bucket: &impl Bucket) -> Vec<String> {
        (bucket::list(bucket, "").await.into_iter())
            .map(|meta| meta.key)
            .filter(|key| !matches!(layout::kind(key), Kind::GcLease { .. }))
            .collect()
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
            assert_eq!(report, Report { deleted: 1, compacted: 0, retired: 0 });
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
            assert!(after.contains(&segment) && after.contains(&layout::moves(&segment).unwrap()));
            assert!(after.contains(&layout::record(test_branch(1))));
            let report = collect(&*bucket, 2, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 0, compacted: 0, retired: 0 });
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
            assert!(after.contains(&segment) && after.contains(&layout::moves(&segment).unwrap()));
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
                assert!(
                    after.contains(&segment) && !after.contains(&layout::moves(&segment).unwrap())
                );
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
        assert!(
            !left.contains(&sealed)
                && left.contains(&layout::moves(&layout::moves(&sealed).unwrap()).unwrap())
        );
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
    /// and acknowledges a commit the branch doesn't hold (specs/bucket's NoPinsFences).
    #[test]
    fn writers_taken_over_finding_their_successors_fences_need_them_pinned() {
        crate::set_rules(crate::Rules { pins_fences: false, ..crate::Rules::ALL });
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

    /// TASK-388: a writer taken over by two in turn, its next segment's create in flight where
    /// the first of them fenced: the collector pins every fence, not only the last, and the create
    /// fails there.
    #[test]
    fn writers_taken_over_twice_find_the_first_successors_fence_pinned() {
        let created = block_on(create_where_the_first_of_two_successors_fenced());
        assert_eq!(created, (Err(Error::Fenced), Err(Error::Fenced), Some(4)));
    }

    /// The collector pinning the last fence alone: the writer taken over creates its segment where
    /// the first successor's fence was, and acknowledges a commit the branch doesn't hold
    /// (specs/bucket's NoPinsOldFences).
    #[test]
    fn writers_taken_over_twice_finding_the_first_successors_fence_need_old_fences_pinned() {
        crate::set_rules(crate::Rules { pins_old_fences: false, ..crate::Rules::ALL });
        let created = block_on(create_where_the_first_of_two_successors_fenced());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(created, (Ok(()), Ok(Some(1)), Some(4)));
    }

    /// What the writer of lease epoch 1 gets of the upload and the acknowledgement of its commit 3
    /// at its segment 1, where the writer of epoch 2 fenced, the writer of epoch 3 fencing after
    /// it, committing 4 and checkpointing, and a pass running. And the branch's last commit then.
    async fn create_where_the_first_of_two_successors_fenced()
    -> (Result<(), Error>, Result<Option<u64>, Error>, Option<u64>) {
        let bucket = store();
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let taken = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
        let last = Writer::open(bucket.clone(), test_branch(1), 12, 0).await.unwrap();
        commit(&last, Commit { tag: 4, ..Commit::default() }).await;
        last.checkpoint().await.unwrap();
        collect(&*bucket, 1, &KEEP).await.unwrap();
        let objects =
            Batch::from(Commit { tag: 3, ..Commit::default() }).close(&mut taken.chain()).frame();
        let seq = taken.add_segment(objects).unwrap();
        assert_eq!(seq, 1);
        let uploaded = taken.upload(seq).await;
        let acknowledged = taken.acknowledge().await;
        let last =
            recovery::recover(&*bucket, test_branch(1), None).await.unwrap().state.last_commit;
        (uploaded, acknowledged, last)
    }

    /// TASK-383: a writer taken over, its next segment's create in flight where a stray is, an
    /// earlier writer's segment landed late, void, its successor fencing past it, committing and
    /// checkpointing, and a pass running: the collector keeps the void segment with the fence, and
    /// the create fails there. Found by the simulation, seed 453 of
    /// `stale_creates_landing_past_deletions`, once a seed replayed as it ran.
    #[test]
    fn writers_taken_over_find_void_segments_before_their_successors_fences_kept() {
        let created = block_on(create_where_a_void_segment_was());
        assert_eq!(created, (Err(Error::Displaced), Err(Error::Displaced), Some(4)));
    }

    /// The collector deleting the stray, covered: the writer taken over creates its
    /// segment in its place, below its successor's fence, and acknowledges a commit the branch
    /// doesn't hold (specs/bucket's NoPinsVoidStrays).
    #[test]
    fn writers_taken_over_finding_void_segments_before_their_successors_fences_need_them_pinned() {
        crate::set_rules(crate::Rules { pins_void_strays: false, ..crate::Rules::ALL });
        let created = block_on(create_where_a_void_segment_was());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(created, (Ok(()), Ok(Some(2)), Some(4)));
    }

    /// What a writer taken over, of lease epoch 2, gets of the upload and the acknowledgement of
    /// its commit 3 at its segment 2, where the writer of epoch 1 before it, which it took the
    /// branch from, landed a segment late, void; after a third writer took the branch, fencing past
    /// that, committed 4 and checkpointed, and a pass ran. And the branch's last commit then.
    async fn create_where_a_void_segment_was()
    -> (Result<(), Error>, Result<Option<u64>, Error>, Option<u64>) {
        let bucket = store();
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let first = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let taken = Writer::open(bucket.clone(), test_branch(1), 11, 0).await.unwrap();
        // The first's segments 1 and 2, of which 2 alone lands, past its successor's fence at 1.
        for tag in [1, 2] {
            let objects =
                Batch::from(Commit { tag, ..Commit::default() }).close(&mut first.chain()).frame();
            let seq = first.add_segment(objects).unwrap();
            if seq == 2 {
                first.upload(seq).await.unwrap();
            }
        }
        let successor = Writer::open(bucket.clone(), test_branch(1), 12, 0).await.unwrap();
        commit(&successor, Commit { tag: 4, ..Commit::default() }).await;
        successor.checkpoint().await.unwrap();
        collect(&*bucket, 1, &KEEP).await.unwrap();
        let objects =
            Batch::from(Commit { tag: 3, ..Commit::default() }).close(&mut taken.chain()).frame();
        let seq = taken.add_segment(objects).unwrap();
        assert_eq!(seq, 2);
        let uploaded = taken.upload(seq).await;
        let acknowledged = taken.acknowledge().await;
        let last =
            recovery::recover(&*bucket, test_branch(1), None).await.unwrap().state.last_commit;
        (uploaded, acknowledged, last)
    }

    /// TASK-383, the other stray: a writer whose taking of the lease came before its successor's
    /// fences late, after the successor's fence, which its fence follows, before finding the lease
    /// taken; a third writer takes the branch, fencing past it, commits and checkpoints, and a pass
    /// runs. The writer taken over, whose next segment goes where the late fence is, finds its
    /// create failing there: the collector keeps it. Found by specs/bucket's TLA+, the model
    /// BucketStrays once void segments alone were kept.
    #[test]
    fn writers_taken_over_find_late_fences_before_their_successors_fences_kept() {
        let created = block_on(create_where_a_late_fence_was());
        assert_eq!(created, (Err(Error::Displaced), Err(Error::Displaced), Some(4)));
    }

    /// The collector deleting the late fence, covered: the writer taken over creates its segment
    /// in its place and acknowledges a commit the branch doesn't hold.
    #[test]
    fn writers_taken_over_finding_late_fences_before_their_successors_fences_need_them_pinned() {
        crate::set_rules(crate::Rules { pins_late_strays: false, ..crate::Rules::ALL });
        let created = block_on(create_where_a_late_fence_was());
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(created, (Ok(()), Ok(Some(1)), Some(4)));
    }

    /// What the writer of lease epoch 2 gets of the upload and the acknowledgement of its commit 3
    /// at its segment 1, where the writer of epoch 1 fenced late, after its fence at 0; after a
    /// third writer took the branch, committed 4 and checkpointed, and a pass ran. And the branch's
    /// last commit then.
    async fn create_where_a_late_fence_was()
    -> (Result<(), Error>, Result<Option<u64>, Error>, Option<u64>) {
        let direct = Arc::new(Direct(SimBucket::new()));
        branch::create(&*direct, test_branch(1), 0).await.unwrap();
        // The first, its lease taken, waits to recover the branch.
        let (late, open) =
            Gated::new(direct.clone(), Gate::List(layout::checkpoints(test_branch(1))));
        let late = Arc::new(late);
        let mut first = Box::pin(Writer::open(late, test_branch(1), 10, 0));
        assert!((&mut first).now_or_never().is_none());
        let taken = Writer::open(direct.clone(), test_branch(1), 11, 0).await.unwrap();
        open.send(()).unwrap();
        assert_eq!(first.await.err(), Some(Error::Fenced));
        let successor = Writer::open(direct.clone(), test_branch(1), 12, 0).await.unwrap();
        commit(&successor, Commit { tag: 4, ..Commit::default() }).await;
        successor.checkpoint().await.unwrap();
        collect(&*direct, 1, &KEEP).await.unwrap();
        let objects =
            Batch::from(Commit { tag: 3, ..Commit::default() }).close(&mut taken.chain()).frame();
        let seq = taken.add_segment(objects).unwrap();
        assert_eq!(seq, 1);
        let uploaded = taken.upload(seq).await;
        let acknowledged = taken.acknowledge().await;
        let last =
            recovery::recover(&*direct, test_branch(1), None).await.unwrap().state.last_commit;
        (uploaded, acknowledged, last)
    }

    /// TASK-371: a writer taking a branch over, which read it before the last writer's segment
    /// landed, finds the segment where it fences, though the last writer acknowledged and
    /// checkpointed it and a pass ran before it fenced: the collector keeps a branch's segments
    /// while its lease is of an epoch past its state's. Found by specs/bucket's TLA+, the model of
    /// NoFencesKept with fences_kept in (since TASK-388, NoPinsTakings).
    #[test]
    fn writers_taking_over_find_what_landed_after_they_read() {
        assert_eq!(block_on(take_over_past_a_pass()), Some(1));
    }

    /// The collector deleting it, the checkpoint covering it: the fence lands in its place, and
    /// the commit the last writer acknowledged is in no history (specs/bucket's NoPinsTakings).
    #[test]
    fn writers_taking_over_finding_what_landed_after_they_read_need_it_pinned() {
        crate::set_rules(crate::Rules { pins_takings: false, ..crate::Rules::ALL });
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
    /// SIM_SOAK=100. TASK-395.4: it reads the point's record again then, finds it deleted, and gives
    /// the fork up, as the pass that fenced it would have, rather than failing: fenced, it deletes
    /// nothing all the same.
    #[test]
    fn passes_fenced_finding_forks_of_points_lost_stop() {
        assert_eq!(block_on(pass_fenced_as_a_point_goes(Raced::Fork)), Ok(Report::default()));
    }

    /// TASK-395.4: a pass that finds a fork of a point being made lost, its point read again
    /// deleted, gives the fork up, whether or not fenced: not live, it lost nothing, as the
    /// collector's recoveries, unlike specs/bucket's, read the bucket at many times (doc-7, For the
    /// code, beyond the model). Its record says so after.
    #[test]
    fn passes_finding_forks_of_points_lost_give_them_up_once_their_points_are_deleted() {
        crate::set_rules(crate::Rules { passes_fenced_stop: false, ..crate::Rules::ALL });
        let passed = block_on(pass_fenced_as_a_point_goes(Raced::Fork));
        crate::set_rules(crate::Rules::ALL);
        assert_eq!(passed, Ok(Report::default()));
    }

    /// TASK-180, since TASK-395.4 gives up on such a fork: a pass finding a failed fork's claim lost,
    /// one a writer has, landed with what it names reclaimed by a pass that fenced it, stops,
    /// deleting nothing, as it would before deleting.
    #[test]
    fn passes_fenced_finding_claims_lost_stop() {
        assert_eq!(block_on(pass_fenced_as_a_point_goes(Raced::Claim)), Ok(Report::default()));
    }

    /// A pass failing on what it finds lost, fenced or not: it fails.
    #[test]
    fn passes_fenced_finding_forks_of_points_lost_stopping_need_passes_fenced_stop() {
        crate::set_rules(crate::Rules { passes_fenced_stop: false, ..crate::Rules::ALL });
        let passed = block_on(pass_fenced_as_a_point_goes(Raced::Claim));
        crate::set_rules(crate::Rules::ALL);
        let lost = Error::Lost("branch 1/1/4: what its recovery starts from keeps going".into());
        assert_eq!(passed, Err(lost));
    }

    /// What lands as [`pass_fenced_as_a_point_goes`] lists the records a second time.
    #[derive(Clone, Copy)]
    enum Raced {
        /// A fork of the point being made, which finds it deleted.
        Fork,
        /// A failed fork's claim, of the point's origin, which a writer takes.
        Claim,
    }

    /// What a pass gives, a point of branch 1 deleted, a later pass reclaiming what it named, and
    /// a fork of the point being made to branch 4, or a claim of 4 landing, as the pass lists the
    /// records a second time, having read the point's before.
    async fn pass_fenced_as_a_point_goes(raced: Raced) -> Result<Report, Error> {
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
        let bucket =
            Hooked::new(direct.clone(), Hook::Listing(layout::records(layout::TEST_CELL), 2));
        let raced_on = direct.clone();
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            let direct = raced_on;
            let origin = Origin { parent: test_branch(1), at };
            branch::delete(&*direct, test_branch(3), 11, u64::MAX).now_or_never().unwrap().unwrap();
            collect(&*direct, 2, &KEEP).now_or_never().unwrap().unwrap();
            let left = keys(&*direct).now_or_never().unwrap();
            assert!(
                !left.contains(&layout::segment(test_branch(1), at)),
                "the point's state stayed"
            );
            match raced {
                Raced::Fork => {
                    let forked =
                        branch::fork_point(&*direct, test_branch(4), test_branch(3), origin, 0x44)
                            .now_or_never()
                            .unwrap();
                    assert_eq!(forked, Err(Error::Deleted));
                }
                Raced::Claim => {
                    let claim = crate::record::Kind::Claim(origin).of(0x44);
                    let key = layout::record(test_branch(4));
                    let created = bucket::create(&*direct, &key, layout::encode(&claim));
                    assert!(matches!(created.now_or_never().unwrap(), Created::Created));
                    let lease = layout::lease(test_branch(4));
                    branch::take_lease(&*direct, &lease, 12, 0).now_or_never().unwrap().unwrap();
                }
            }
        }));
        let passed = collect(&bucket, 1, &KEEP).await;
        if let (Raced::Fork, Ok(_)) = (raced, &passed) {
            let record = branch::record(&*direct, test_branch(4)).await.unwrap().unwrap();
            assert!(record.deleted && record.forking().is_some(), "{record:?}: given up");
        }
        passed
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
            Ok(Report { deleted: 2, compacted: 0, retired: 0 })
        );
    }

    /// A pass recovering each branch once, by its parent's state: it deletes what the restore's
    /// child needs, which fails every pass after, unless the restore's reference keeps it.
    #[test]
    fn children_restored_from_points_keeping_their_origins_need_recoveries_by_origins() {
        // TASK-395.1: the restore's reference keeps its origin's state at the fork point too: with
        // references gone, as specs/bucket's NoRecoveriesByOrigins has it.
        let rules =
            crate::Rules { recoveries_by_origins: false, references: false, ..crate::Rules::ALL };
        crate::set_rules(rules);
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
        assert_eq!(
            (record, passed),
            (Some(deleted), Ok(Report { deleted: 0, compacted: 0, retired: 0 }))
        );
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
        let fenced =
            (Err(Error::Fenced), None, Ok(Report { deleted: 0, compacted: 0, retired: 0 }));
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

        fn list_delimited(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<crate::bucket::Listing, crate::bucket::Error>> + Send
        {
            self.bucket.list_delimited(prefix)
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
        /// Before the put of a key under this prefix of this number, from 1.
        Putting(String, usize),
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

        fn list_delimited(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<crate::bucket::Listing, crate::bucket::Error>> + Send
        {
            self.bucket.list_delimited(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: bytes::Bytes,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            if let Hook::Putting(at, nth) = &self.hook
                && let Some(then) = self.nth(key.starts_with(at.as_str()), *nth)
            {
                then();
            }
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
            assert!(!left.contains(&segment) && left.contains(&layout::moves(&segment).unwrap()));
            collect(&*bucket, 3, &now).await.unwrap();
            assert!(!keys(&*bucket).await.contains(&layout::moves(&segment).unwrap()));
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
    /// deletes what goes many at once. TASK-395.4: a second pass of the same collector reads every
    /// record again, as the first: a collector keeps nothing of a cell from one pass to the next,
    /// so that a sweep over every cell holds no more than the cell it passes over (TASK-395.6 is to
    /// keep what is worth keeping under the cell's prefix). Each pass reads one record first, its
    /// format, before taking the cell's lease.
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
        let first = [vec![1, 9], vec![1], vec![9]];
        assert_eq!(passes, [first, [vec![1, 9], vec![1], vec![]]]);

        // The fork deleted: its lease and its deletion's fence go.
        bucket.served(branch::delete(&*bucket, test_branch(2), 20, 0)).0.unwrap();
        let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, 3));
        report.unwrap();
        assert_eq!(asked(&rounds), [vec![1, 9], vec![1], vec![2]]);
    }

    /// TASK-395.5: how many segments [`mostly_freed`] makes.
    const SEGMENTS: u64 = 5;

    /// Branch 1, [`SEGMENTS`] segments of it each holding a small extent it keeps and a large one
    /// it freed after, and a checkpoint past them: what a pass compacts, a moves each, and the
    /// next, the moves aged, deletes. The segments' keys.
    async fn mostly_freed<B: Bucket>(bucket: Arc<B>) -> Vec<String> {
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), test_branch(1), 10, 0).await.unwrap();
        let (mut segments, mut frees) = (Vec::new(), Vec::new());
        for n in 0..SEGMENTS {
            let first = writer.chain().next_sequence();
            let (small, large) = (ExtentId::new(1, first), ExtentId::new(1, first + 1));
            let large_bytes: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
            let extents = vec![(small, b"small".to_vec()), (large, large_bytes)];
            commit(&writer, Commit { tag: n + 1, extents, ..Commit::default() }).await;
            segments.push(writer.location(small).unwrap().key);
            frees.push(large);
        }
        commit(&writer, Commit { tag: SEGMENTS + 1, frees, ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
        segments
    }

    /// TASK-395.5: a pass compacts the objects mostly freed at once, reading them whole in one
    /// round, and the next reads their moves' footers in one round and deletes them in one: no
    /// request waits on another of its kind. Before, each compaction and each moves' footer was a
    /// round of its own.
    #[test]
    fn passes_compact_and_read_moves_at_once() {
        let bucket = SimBucket::new();
        let (segments, _) = bucket.served(mostly_freed(bucket.clone()));
        let mut collector = Collector::new(Options { compact_below: 0.5, grace: 0 });
        // How many of the requests `asking` each round asked, of the rounds asking any.
        let counted = |rounds: &[Vec<String>], asking: &dyn Fn(&str) -> bool| -> Vec<u64> {
            let counts = rounds.iter().map(|round| round.iter().filter(|r| asking(r)).count());
            counts.filter(|&count| count > 0).map(|count| count as u64).collect()
        };
        let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, 1));
        assert_eq!(report.unwrap().compacted, SEGMENTS as usize);
        let whole = |asked: &str| segments.iter().any(|segment| asked == format!("get {segment}"));
        assert_eq!(counted(&rounds, &whole), [SEGMENTS]);

        let (report, rounds) = bucket.served_in_rounds(collector.collect(&*bucket, 2));
        assert_eq!(
            report.unwrap(),
            Report { deleted: SEGMENTS as usize, compacted: 0, retired: 0 }
        );
        // A footer is read by its range, once its trailer, the object's end, says where it is.
        let footers = |asked: &str| {
            let moves = |segment: &String| format!("get {} ", layout::moves(segment).unwrap());
            let of_moves = segments.iter().any(|segment| asked.starts_with(&moves(segment)));
            of_moves && !asked.contains(" last ")
        };
        assert_eq!(counted(&rounds, &footers), [SEGMENTS]);
        assert_eq!(counted(&rounds, &|asked| asked.starts_with("delete ")), [SEGMENTS]);
        let left = bucket.keys();
        assert_eq!(segments.iter().filter(|segment| left.contains(segment)).count(), 0);
    }

    /// TASK-395.5: over S3, a pass deletes what goes in one DeleteObjects, no DELETE: the S3
    /// server and the store count one batch, of the segments [`mostly_freed`] makes, which the
    /// pass before compacted.
    #[test]
    fn passes_delete_in_batches_over_s3() {
        use crate::s3_server::{BUCKET, S3Server};
        use crate::store::Request;
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let server = S3Server::start(None);
            let url = format!("s3://{BUCKET}/collector");
            let client = &crate::client::Client::DEFAULT;
            let (objects, prefix) =
                crate::store::object_store_with(&url, server.options(), client).unwrap();
            let bucket = Arc::new(Store::new(objects, &prefix));
            let segments = mostly_freed(bucket.clone()).await;
            let mut collector = Collector::new(Options { compact_below: 0.5, grace: 0 });
            let report = collector.collect(&*bucket, 1).await.unwrap();
            assert_eq!(report.compacted, SEGMENTS as usize);

            let (served, stats) = (server.requests(), bucket.stats());
            let report = collector.collect(&*bucket, 2).await.unwrap();
            assert_eq!(report, Report { deleted: SEGMENTS as usize, compacted: 0, retired: 0 });
            let (served_after, stats_after) = (server.requests(), bucket.stats());
            let served =
                |kind| served_after.get(kind).unwrap_or(&0) - served.get(kind).unwrap_or(&0);
            let counted = |request| stats_after.requests(request) - stats.requests(request);
            assert_eq!((served("delete-objects"), served("delete")), (1, 0));
            assert_eq!((counted(Request::DeleteBatch), counted(Request::Delete)), (1, 0));
            let left = keys(&*bucket).await;
            assert_eq!(segments.iter().filter(|segment| left.contains(segment)).count(), 0);
        });
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

        let live = block_on(branch::live(&*bucket, layout::TEST_CELL)).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), [test_branch(1), test_branch(2)]);
        block_on(branch::delete(&*bucket, test_branch(2), 12, u64::MAX)).unwrap();
        block_on(collect(&*bucket, 2, &KEEP)).unwrap();
        assert!(!kept(), "what the fork inherited stayed once it was deleted");
        let live = block_on(branch::live(&*bucket, layout::TEST_CELL)).unwrap();
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

        fn list_delimited(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<crate::bucket::Listing, BucketError>> + Send {
            self.bucket.list_delimited(prefix)
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

        fn list_delimited(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<crate::bucket::Listing, crate::bucket::Error>> + Send
        {
            self.bucket.list_delimited(prefix)
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
    /// TASK-395.1: since every clone writes a reference, listed after the states, which finds it as
    /// well, the rule shows needed only with references gone too, as specs/bucket's
    /// NoRecordsRelisted has it.
    #[test]
    fn forks_of_points_made_and_gone_within_a_pass_keeping_what_they_hold_need_records_relisted() {
        let rules =
            crate::Rules { records_relisted: false, references: false, ..crate::Rules::ALL };
        crate::set_rules(rules);
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
        assert_eq!(report, Report { deleted: 1, compacted: 0, retired: 0 });
        assert!(!block_on(keys(&deleting)).contains(&stray));
    }

    /// TASK-264: what the bucket holds, every object, the collector's lease included: its key, its
    /// version, size and time, and its bytes.
    async fn held(bucket: &impl Bucket) -> Vec<(Meta, Vec<u8>)> {
        let mut held = Vec::new();
        for meta in bucket::list(bucket, "").await {
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
            let dry =
                Collector::new(options).dry_run_cell(&*bucket, layout::TEST_CELL).await.unwrap();
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
            assert_eq!(report, Report { deleted: 2, compacted: 0, retired: 0 });
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
            let during =
                Hooked::new(direct.clone(), Hook::Getting(layout::gc_lease(layout::TEST_CELL), 2));
            let passing = direct.clone();
            *during.then.lock().unwrap() = Some(Box::new(move || {
                let pass = collect(&*passing, 9, &KEEP).now_or_never();
                assert_eq!(pass, Some(Ok(Report::default())));
            }));
            let dry = Collector::new(KEEP).dry_run_cell(&during, layout::TEST_CELL).await.unwrap();
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
            assert_eq!(report, Report { deleted: 2, compacted: 1, retired: 0 });
            let after = keys(&*bucket).await;
            before.into_iter().filter(|key| !after.contains(key)).collect::<Vec<_>>()
        });
        assert_eq!(deleted, garbage_doomed());
        block_on(async {
            let bucket = store();
            garbage(&bucket).await;
            let before = held(&*bucket).await;
            let mut collector = Collector::new(compacting);
            let marking = collector.mark_cell(&*bucket, layout::TEST_CELL, 2).await.unwrap();
            assert_eq!(marking.report, Report { deleted: 0, compacted: 1, retired: 0 });
            assert_eq!(marking.resurrected, Vec::<String>::new());
            let marks = marking.marks.unwrap();
            let listed: BTreeMap<String, Meta> =
                before.iter().map(|(meta, _)| (meta.key.clone(), meta.clone())).collect();
            let expected: Vec<Marked> =
                deleted.iter().map(|key| Marked::of(&listed[key])).collect();
            assert_eq!(marks, Marks { epoch: 2, grace: 0, marked: expected });
            let stored =
                bucket::get(&*bucket, &layout::marks(layout::TEST_CELL, 2)).await.unwrap().bytes;
            assert_eq!(layout::decode::<Marks>(&stored), Ok(marks));
            // Nothing deleted: what was there is, but the collector's lease, taken again.
            let after = held(&*bucket).await;
            let kept = |held: &[(Meta, Vec<u8>)]| -> Vec<(Meta, Vec<u8>)> {
                held.iter()
                    .filter(|(meta, _)| meta.key != layout::gc_lease(layout::TEST_CELL))
                    .cloned()
                    .collect()
            };
            let mut more: Vec<String> = after.iter().map(|(meta, _)| meta.key.clone()).collect();
            more.retain(|key| !listed.contains_key(key));
            let moved = layout::moves(&layout::segment(test_branch(1), 1)).unwrap();
            // And the word that the bucket is marked, which readers go by (TASK-395.4's review).
            let marking = layout::MARKING.to_owned();
            assert_eq!(more, [marking, layout::marks(layout::TEST_CELL, 2), moved]);
            let left: Vec<_> = kept(&after)
                .into_iter()
                .filter(|(meta, _)| listed.contains_key(&meta.key))
                .collect();
            assert_eq!(left, kept(&before));

            let next = collector.mark_cell(&*bucket, layout::TEST_CELL, 3).await.unwrap();
            assert_eq!(next.report, Report { deleted: 0, compacted: 0, retired: 0 });
            assert_eq!(next.resurrected, Vec::<String>::new());
            let marked: Vec<String> =
                next.marks.unwrap().marked.into_iter().map(|marked| marked.key).collect();
            let mut expected = garbage_doomed();
            expected.push(layout::segment(test_branch(1), 1));
            expected.sort();
            assert_eq!(marked, expected);

            let report = collect(&*bucket, 4, &compacting).await.unwrap();
            assert_eq!(report, Report { deleted: 3, compacted: 0, retired: 0 });
            let marks: Vec<String> = (keys(&*bucket).await.into_iter())
                .filter(|key| matches!(layout::kind(key), Kind::Marks { .. }))
                .collect();
            assert_eq!(
                marks,
                [layout::marks(layout::TEST_CELL, 2), layout::marks(layout::TEST_CELL, 3)]
            );
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
            let marking = Collector::new(KEEP).mark_cell(&*bucket, layout::TEST_CELL, 1).await;
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
        let marks = layout::marks(layout::TEST_CELL, 1);
        let reached = |request: &str| {
            format!(
                "WARN extent_remote: {request} {key}: {saying} ({marks}) key={key} kind={kind:?} \
                 request={request} marks=1"
            )
        };
        // Its tail, then its data.
        let read_twice = [reached("get-range"), reached("get-range")];
        assert_eq!(warnings(), read_twice);

        let next =
            block_on(Collector::new(KEEP).mark_cell(&*bucket, layout::TEST_CELL, 2)).unwrap();
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

    // fxdax: the collector per cell, and fork references (TASK-395.2, .3, .4).

    /// Branch `id` of tenant 1's cell `cell`: each of these tests' branches a cell of its own, as
    /// hosts make them.
    fn celled(cell: u64, id: u64) -> Branch {
        Branch { cell: layout::Cell { tenant: 1, id: cell }, id }
    }

    /// The child of a writable fork to `branch`, its owner its ID.
    fn forked(branch: Branch) -> Child {
        Child { branch, owner: 0x100 | branch.id, point: false }
    }

    /// Has `writer` commit `extents` in a segment, then fork to `child` in the next, uploaded and
    /// acknowledged, which records the child: the fork's segment.
    async fn commit_and_fork<B: Bucket>(
        writer: &Writer<B>,
        extents: Vec<(ExtentId, Vec<u8>)>,
        child: Child,
    ) -> u64 {
        commit(writer, Commit { tag: 1, extents, ..Commit::default() }).await;
        let forking = Batch::from(Commit { tag: 2, ..Commit::default() });
        let at = writer.add_segment(forking.fork(child, &mut writer.chain()).frame()).unwrap();
        writer.upload(at).await.unwrap();
        writer.acknowledge().await.unwrap();
        at
    }

    /// The position in `asked`, requests as [`SimBucket::served`] gives them, of the first that is
    /// `request`.
    fn position(asked: &[String], request: &str) -> usize {
        asked
            .iter()
            .position(|asked| asked == request)
            .unwrap_or_else(|| panic!("{request}: {asked:#?}"))
    }

    /// TASK-395.3: a writable fork's child of another cell gets its reference, under its parent's
    /// cell, before its record, the writer writing both; of bytes unique to the fork, its owner's,
    /// which a deletion recording the fork again writes again, the same. A child of the parent's own
    /// cell gets none: that cell's pass reads its record.
    #[test]
    fn writable_forks_refer_before_their_childrens_records() {
        let bucket = SimBucket::new();
        let (parent, other, own) = (celled(1, 1), celled(2, 2), celled(1, 3));
        let (writer, _) = bucket.served(async {
            branch::create(&*bucket, parent, 0).await.unwrap();
            Writer::open(bucket.clone(), parent, 10, 0).await.unwrap()
        });
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        let (at, asked) = bucket.served(commit_and_fork(
            &writer,
            vec![(extent, b"held".to_vec())],
            forked(other),
        ));
        let reference = layout::reference(parent, at, other);
        let referred = position(&asked, &format!("put {reference}"));
        assert!(
            referred < position(&asked, &format!("put {}", layout::record(other))),
            "{asked:#?}"
        );
        let bytes = bucket.served(bucket::get(&*bucket, &reference)).0.unwrap().bytes;
        assert_eq!(bytes, layout::encode(&layout::Reference::of(forked(other).owner)));

        let (at, asked) = bucket.served(commit_and_fork(&writer, Vec::new(), forked(own)));
        let refers = |asked: &[String]| -> Vec<String> {
            let put = |asked: &&String| asked.starts_with("put ") && asked.contains("/refs/");
            asked.iter().filter(put).cloned().collect()
        };
        assert_eq!(refers(&asked), Vec::<String>::new(), "within the cell, at {at}");

        // The parent's deletion records the forks of its state again, their references first.
        let (deleted, asked) = bucket.served(branch::delete(&*bucket, parent, 20, 0));
        deleted.unwrap();
        assert_eq!(refers(&asked), [format!("put {reference}")]);
        let references: Vec<String> =
            bucket.keys().into_iter().filter(|key| key.contains("/refs/")).collect();
        assert_eq!(references, [reference]);
    }

    /// TASK-395.3: a clone of a point writes its reference, under the cell of the point's origin,
    /// first: before its record, and before it reads the point's; whatever its cell, the
    /// origin's own too.
    #[test]
    fn clones_refer_before_their_records_and_their_points_read() {
        use crate::layout::Origin;
        let bucket = SimBucket::new();
        let (origin, point) = (Origin { parent: celled(1, 1), at: 3 }, celled(2, 2));
        // The point's record, of an origin no bucket here has: what the clones read of it.
        let point_record = layout::encode(&crate::record::Kind::Point(origin).of(0x22));
        bucket.served(bucket::create(&*bucket, &layout::record(point), point_record));
        for clone in [celled(3, 3), celled(1, 4)] {
            let (forked, asked) =
                bucket.served(branch::fork_point(&*bucket, clone, point, origin, 0x33));
            forked.unwrap();
            let reference = layout::reference(origin.parent, origin.at, clone);
            let referred = position(&asked, &format!("put {reference}"));
            let recorded = position(&asked, &format!("put {}", layout::record(clone)));
            let read = position(&asked, &format!("get {}", layout::record(point)));
            assert!(referred < recorded && recorded < read, "{asked:#?}");
            let bytes = bucket.served(bucket::get(&*bucket, &reference)).0.unwrap().bytes;
            assert_eq!(bytes, layout::encode(&layout::Reference::of(0x33)));
        }
    }

    /// What reading `extent` gives `branch`, recovered from its record's origin.
    async fn read_inherited<B: Bucket>(
        bucket: &Arc<B>,
        branch: Branch,
        extent: ExtentId,
    ) -> Result<Vec<u8>, Error> {
        let record = branch::record(&**bucket, branch).await?.ok_or(Error::NoBranch)?;
        let recovered = recovery::recover(&**bucket, branch, record.origin()).await?;
        let location = recovered.state.extents.get(&extent).cloned();
        let location = location.ok_or_else(|| Error::Lost(format!("{extent:?} of {branch}")))?;
        crate::read(bucket, extent, &location).await
    }

    /// Sweeps over every cell, deleting what is free at once, until two in a row delete nothing:
    /// failing as a pass finds what a live branch needs lost.
    async fn sweep(bucket: &impl Bucket) -> Result<(), Error> {
        let now = Options { compact_below: 0.0, grace: 0 };
        crate::testing::collect_until_quiet(bucket, &now).await.map(drop)
    }

    /// TASK-395.4: a fork to another cell keeps its parent's state at the fork point in the
    /// parent's cell, by its reference, the parent deleted and having freed what the child
    /// inherited, its checkpoint covering the segments the child, with no checkpoint of its own,
    /// recovers through.
    #[test]
    fn fork_points_keep_what_children_in_other_cells_inherit() {
        assert_eq!(block_on(read_forked_from_a_deleted_parent()), Ok(b"held".to_vec()));
    }

    /// Without references, the parent's cell's pass reclaims what the child inherited: it reads its
    /// parent deleted, and lists nothing of the child's cell.
    #[test]
    fn fork_points_kept_for_children_in_other_cells_need_references() {
        crate::set_rules(crate::Rules { references: false, ..crate::Rules::ALL });
        let read = block_on(read_forked_from_a_deleted_parent());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// Keeping the extents of a fork point's state alone, not what recovering it reads: the child,
    /// with no checkpoint of its own, recovers through the parent's segments, which the parent's
    /// checkpoint covers, gone.
    #[test]
    fn fork_points_kept_for_children_in_other_cells_need_what_recovering_them_reads() {
        crate::set_rules(crate::Rules { fork_points_kept: false, ..crate::Rules::ALL });
        let read = block_on(read_forked_from_a_deleted_parent());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What a fork to another cell reads of what it inherited, its parent having freed it,
    /// checkpointed and been deleted, sweeps having run until they did nothing more.
    async fn read_forked_from_a_deleted_parent() -> Result<Vec<u8>, Error> {
        let bucket = store();
        let (parent, child) = (celled(1, 1), celled(2, 2));
        branch::create(&*bucket, parent, 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        commit_and_fork(&writer, vec![(extent, b"held".to_vec())], forked(child)).await;
        commit(&writer, Commit { tag: 3, frees: vec![extent], ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
        drop(writer);
        branch::delete(&*bucket, parent, 20, 0).await.unwrap();
        sweep(&*bucket).await?;
        read_inherited(&bucket, child, extent).await
    }

    /// TASK-395.3, TASK-395.4: a chain of forks across cells, a root's, its child's, its
    /// grandchild's: the child deleted, its cell keeps its own state at the grandchild's fork
    /// point, and its reference under the root's cell stays, the child's cell holding the
    /// grandchild's; the root frees and checkpoints, and the grandchild, with no checkpoint, reads
    /// what it inherited through both. The grandchild deleted too, the passes retire the
    /// references down the chain, and the cells of the dead are reclaimed whole but their records:
    /// no reference is left, and the root's cell holds its own alone.
    #[test]
    fn references_hold_while_their_childrens_cells_hold_references() {
        let (read, bucket) = block_on(read_through_a_deleted_child());
        assert_eq!(read, Ok(b"held".to_vec()));
        block_on(async {
            let grandchild = celled(3, 3);
            branch::delete(&*bucket, grandchild, 22, 0).await.unwrap();
            sweep(&*bucket).await.unwrap();
        });
        let left = block_on(keys(&*bucket));
        let references: Vec<&String> = left.iter().filter(|key| key.contains("/refs/")).collect();
        assert_eq!(references, Vec::<&String>::new());
        for cell in [2, 3] {
            let prefix = layout::Cell { tenant: 1, id: cell }.prefix();
            let kept: Vec<&String> = (left.iter())
                .filter(|key| key.starts_with(&prefix))
                .filter(|key| {
                    !matches!(layout::kind(key), Kind::Record { .. } | Kind::Marks { .. })
                })
                .collect();
            assert_eq!(kept, Vec::<&String>::new(), "cell {cell}, dead, reclaimed whole");
        }
    }

    /// The child's reference retired once the child is deleted, though its cell holds the
    /// grandchild's: the root's cell's pass reclaims what the grandchild inherited through it.
    #[test]
    fn references_held_by_their_childrens_cells_need_references_held() {
        crate::set_rules(crate::Rules { references_held: false, ..crate::Rules::ALL });
        let (read, _) = block_on(read_through_a_deleted_child());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What a grandchild, forked to cell 3 from a child forked to cell 2 from a root in cell 1,
    /// reads of what it inherited from the root: the child deleted, the root having freed it and
    /// checkpointed, sweeps having run until they did nothing more. With the bucket.
    async fn read_through_a_deleted_child() -> (Result<Vec<u8>, Error>, Arc<Store>) {
        let bucket = store();
        let (root, child, grandchild) = (celled(1, 1), celled(2, 2), celled(3, 3));
        branch::create(&*bucket, root, 0).await.unwrap();
        let writer = Writer::open(bucket.clone(), root, 10, 0).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        commit_and_fork(&writer, vec![(extent, b"held".to_vec())], forked(child)).await;
        let forking = Writer::open(bucket.clone(), child, 11, 0).await.unwrap();
        commit_and_fork(&forking, Vec::new(), forked(grandchild)).await;
        drop(forking);
        branch::delete(&*bucket, child, 21, 0).await.unwrap();
        sweep(&*bucket).await.unwrap();
        commit(&writer, Commit { tag: 3, frees: vec![extent], ..Commit::default() }).await;
        writer.checkpoint().await.unwrap();
        let read = match sweep(&*bucket).await {
            Ok(()) => read_inherited(&bucket, grandchild, extent).await,
            Err(error) => Err(error),
        };
        (read, bucket)
    }

    /// TASK-395.3: a deletion that records the child of a fork across cells, its writer having
    /// recorded none, the fork's segment landed but never acknowledged, writes its reference before
    /// it marks the parent deleted: a pass over the parent's cell between the two keeps the fork
    /// point, by the parent's state; after, by the reference.
    #[test]
    fn deletions_refer_before_they_mark_parents_deleted() {
        assert_eq!(block_on(read_as_a_deletion_refers()), Ok(b"held".to_vec()));
    }

    /// The references after the parent marked deleted, as specs/bucket's NoReferencesFirst has it:
    /// a pass over the parent's cell between the two reads it deleted, and finds no reference.
    #[test]
    fn deletions_referring_before_they_mark_parents_deleted_need_references_first() {
        crate::set_rules(crate::Rules { references_first: false, ..crate::Rules::ALL });
        let read = block_on(read_as_a_deletion_refers());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What the child reads of what it inherited, its parent's deletion recording it, sweeps
    /// running as the deletion writes the reference.
    async fn read_as_a_deletion_refers() -> Result<Vec<u8>, Error> {
        let direct = Arc::new(Direct(SimBucket::new()));
        let (parent, child) = (celled(1, 1), celled(2, 2));
        branch::create(&*direct, parent, 0).await.unwrap();
        let writer = Writer::open(direct.clone(), parent, 10, 0).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        commit(
            &writer,
            Commit { tag: 1, extents: vec![(extent, b"held".to_vec())], ..Commit::default() },
        )
        .await;
        // The fork's segment, uploaded, never acknowledged: its writer records nothing.
        let forking = Batch::from(Commit { tag: 2, ..Commit::default() });
        let at =
            writer.add_segment(forking.fork(forked(child), &mut writer.chain()).frame()).unwrap();
        writer.upload(at).await.unwrap();
        drop(writer);
        let bucket = Hooked::new(direct.clone(), Hook::Putting(layout::references(parent.cell), 1));
        let sweeping = direct.clone();
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            let now = Options { compact_below: 0.0, grace: 0 };
            let pass = collect_cell(&*sweeping, parent.cell, 1, &now);
            pass.now_or_never().unwrap().unwrap();
        }));
        branch::delete(&bucket, parent, 20, 0).await.unwrap();
        assert!(bucket.then.lock().unwrap().is_none(), "the pass ran");
        sweep(&*direct).await?;
        read_inherited(&direct, child, extent).await
    }

    /// TASK-395.3: a clone writes its reference before it reads its point: a clone that read the
    /// point live, the point and its origin deleted and swept away as it finishes, the point's
    /// reference retired, keeps what it inherited, by its reference.
    #[test]
    fn clones_keep_what_they_inherit_as_their_points_go() {
        assert_eq!(block_on(read_as_a_point_goes_under_a_clone()), Ok(b"held".to_vec()));
    }

    /// The clone's reference written once it is made, as specs/bucket's NoClonesReferFirst has
    /// it: the sweeps between find neither the point nor a reference.
    #[test]
    fn clones_keeping_what_they_inherit_as_their_points_go_need_clones_refer_first() {
        crate::set_rules(crate::Rules { clones_refer_first: false, ..crate::Rules::ALL });
        let read = block_on(read_as_a_point_goes_under_a_clone());
        crate::set_rules(crate::Rules::ALL);
        assert!(matches!(read, Err(Error::Lost(_))), "{read:?}");
    }

    /// What a clone in cell 3 of a point in cell 2 of a branch in cell 1 reads of what it inherited,
    /// the point and the branch deleted, and passes run over their cells, the point's then the
    /// branch's, as the clone makes its record made, having read the point live. Not the clone's:
    /// its pass would give the clone up, its point deleted, which loses nothing.
    async fn read_as_a_point_goes_under_a_clone() -> Result<Vec<u8>, Error> {
        use crate::layout::Origin;
        let direct = Arc::new(Direct(SimBucket::new()));
        let (origin_branch, point, clone) = (celled(1, 1), celled(2, 2), celled(3, 3));
        branch::create(&*direct, origin_branch, 0).await.unwrap();
        let writer = Writer::open(direct.clone(), origin_branch, 10, 0).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        let made = Child { branch: point, owner: 0x22, point: true };
        let at = commit_and_fork(&writer, vec![(extent, b"held".to_vec())], made).await;
        drop(writer);
        let origin = Origin { parent: origin_branch, at };
        // Before the update that makes the fork, its record's second put.
        let bucket = Hooked::new(direct.clone(), Hook::Putting(layout::record(clone), 2));
        let sweeping = direct.clone();
        *bucket.then.lock().unwrap() = Some(Box::new(move || {
            let now = Options { compact_below: 0.0, grace: 0 };
            let deleted = async {
                branch::delete(&*sweeping, point, 20, 0).await.unwrap();
                branch::delete(&*sweeping, origin_branch, 21, 0).await.unwrap();
                collect_cell(&*sweeping, point.cell, 1, &now).await.unwrap();
                collect_cell(&*sweeping, origin_branch.cell, 1, &now).await.unwrap();
            };
            deleted.now_or_never().unwrap();
        }));
        branch::fork_point(&bucket, clone, point, origin, 0x33).await?;
        assert!(bucket.then.lock().unwrap().is_none(), "the sweep ran");
        sweep(&*direct).await?;
        read_inherited(&direct, clone, extent).await
    }

    /// Cells numbered from 0x100, `count` of them, each a branch of its own, `garbage` freed in it
    /// and checkpointed past: what a pass over each deletes.
    async fn garbage_cells(bucket: &Arc<SimBucket>, count: u64) {
        for cell in 0x100..0x100 + count {
            let branch = celled(cell, cell);
            branch::create(&**bucket, branch, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), branch, 10, 0).await.unwrap();
            let extent = ExtentId::new(cell, writer.chain().next_sequence());
            let extents = vec![(extent, b"garbage".to_vec())];
            commit(&writer, Commit { tag: 1, extents, ..Commit::default() }).await;
            commit(&writer, Commit { tag: 2, frees: vec![extent], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
        }
    }

    /// TASK-395.4: a pass over a cell, its one branch a fork of a branch in another cell, deleted,
    /// in a bucket of `others` cells more: the requests it makes, and the objects it deleted.
    fn pass_over_a_dead_fork(others: u64) -> (Vec<String>, Report, Branch, Branch, u64) {
        let bucket = SimBucket::new();
        let (parent, child) = (celled(1, 1), celled(2, 2));
        let at = bucket
            .served(async {
                garbage_cells(&bucket, others).await;
                branch::create(&*bucket, parent, 0).await.unwrap();
                let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
                let extent = ExtentId::new(1, writer.chain().next_sequence());
                let at = commit_and_fork(&writer, vec![(extent, vec![1; 64])], forked(child)).await;
                let forking = Writer::open(bucket.clone(), child, 11, 0).await.unwrap();
                let own = ExtentId::new(2, forking.chain().next_sequence());
                commit(
                    &forking,
                    Commit { tag: 3, extents: vec![(own, vec![2; 64])], ..Commit::default() },
                )
                .await;
                forking.checkpoint().await.unwrap();
                drop(forking);
                branch::delete(&*bucket, child, 21, 0).await.unwrap();
                at
            })
            .0;
        let options = Options { compact_below: 0.0, grace: 0 };
        let (report, asked) = bucket.served(collect_cell(&*bucket, child.cell, 7, &options));
        (asked, report.unwrap(), parent, child, at)
    }

    /// TASK-395.4's AC #1: a pass over a cell lists nothing outside the cell's prefix, writes and
    /// deletes nothing outside it but the one reference it retires, of its deleted child's fork
    /// across cells; and what it asks is the same in a bucket of many more cells: it grows with
    /// the cell, not the bucket.
    #[test]
    fn passes_over_a_cell_list_and_delete_within_it_alone() {
        let (asked, report, parent, child, at) = pass_over_a_dead_fork(3);
        let prefix = child.cell.prefix();
        let retired = layout::reference(parent, at, child);
        for request in &asked {
            let (verb, key) = request.split_once(' ').unwrap();
            let key = key.split(' ').next().unwrap();
            match verb {
                "list" | "list-delimited" | "put" => assert!(key.starts_with(&prefix), "{request}"),
                "delete" => assert!(key.starts_with(&prefix) || key == retired, "{request}"),
                _ => {}
            }
        }
        let lists = asked.iter().filter(|asked| asked.starts_with("list")).count();
        assert!(lists > 0, "{asked:#?}");
        assert!(asked.contains(&format!("delete {retired}")), "{asked:#?}");
        assert_eq!(report.retired, 1);
        assert!(report.deleted > 0, "{report:?}");
        let (many, many_report, ..) = pass_over_a_dead_fork(30);
        assert_eq!(many_report, report);
        assert_eq!(many.len(), asked.len(), "{asked:#?}\n{many:#?}");
    }

    /// TASK-395.4's AC #2: a fork point many references name, a point cloned many times, is
    /// recovered once a pass: the pass over its cell asks the same, its references listed by
    /// delimiter, however many clones there are.
    #[test]
    fn fork_points_shared_by_many_references_are_recovered_once() {
        use crate::layout::Origin;
        let asked = |clones: u64| {
            let bucket = SimBucket::new();
            let (origin_branch, point) = (celled(1, 1), celled(2, 2));
            let at = bucket
                .served(async {
                    branch::create(&*bucket, origin_branch, 0).await.unwrap();
                    let writer = Writer::open(bucket.clone(), origin_branch, 10, 0).await.unwrap();
                    let extent = ExtentId::new(1, writer.chain().next_sequence());
                    let made = Child { branch: point, owner: 0x22, point: true };
                    let at = commit_and_fork(&writer, vec![(extent, vec![1; 64])], made).await;
                    commit(&writer, Commit { tag: 3, frees: vec![extent], ..Commit::default() })
                        .await;
                    writer.checkpoint().await.unwrap();
                    let origin = Origin { parent: origin_branch, at };
                    for clone in 0..clones {
                        let clone = celled(0x10 + clone, 0x10 + clone);
                        branch::fork_point(&*bucket, clone, point, origin, clone.id).await.unwrap();
                    }
                    at
                })
                .0;
            let options = Options { compact_below: 0.0, grace: 0 };
            let (report, asked) =
                bucket.served(collect_cell(&*bucket, origin_branch.cell, 7, &options));
            report.unwrap();
            // The fork point's state, the checkpoint at it the point's recording wrote.
            let point = format!("get {}", layout::checkpoint(origin_branch, at));
            let read = asked.iter().filter(|asked| **asked == point).count();
            (asked.len(), read)
        };
        let (one, read) = asked(1);
        assert_eq!(read, 1, "the fork point's checkpoint");
        assert_eq!(asked(8), (one, read));
    }

    /// TASK-395.4: a dry run over a dead cell says what reference a pass would retire, deleting it
    /// not; a mark-only pass retires none either; a pass does.
    #[test]
    fn dry_runs_and_mark_only_passes_retire_nothing() {
        let bucket = store();
        let (parent, child) = (celled(1, 1), celled(2, 2));
        let at = block_on(async {
            branch::create(&*bucket, parent, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
            let at = commit_and_fork(&writer, Vec::new(), forked(child)).await;
            branch::delete(&*bucket, child, 21, 0).await.unwrap();
            at
        });
        let reference = layout::reference(parent, at, child);
        let mut collector = Collector::new(KEEP);
        let dry = block_on(collector.dry_run_cell(&*bucket, child.cell)).unwrap();
        assert_eq!(dry.retired, std::slice::from_ref(&reference));
        let marking = block_on(collector.mark_cell(&*bucket, child.cell, 1)).unwrap();
        assert_eq!(marking.report.retired, 0);
        assert!(block_on(keys(&*bucket)).contains(&reference));
        let report = block_on(collector.collect_cell(&*bucket, child.cell, 2)).unwrap();
        assert_eq!(report.retired, 1);
        assert!(!block_on(keys(&*bucket)).contains(&reference));
    }

    /// TASK-395.4's AC #1, a live fork's cell (TASK-395.4's review): a pass over the cell of a
    /// fork of a branch in another cell, the fork live with no checkpoint of its own, lists nothing
    /// outside its cell, reading its parent's state by the checkpoint at the fork point alone,
    /// which recording the fork wrote; whether the parent had a checkpoint before the fork, or
    /// none. The fork reads what it inherited.
    #[test]
    fn passes_over_a_live_forks_cell_list_nothing_of_its_parents() {
        for checkpointed in [true, false] {
            let bucket = SimBucket::new();
            let (parent, child) = (celled(1, 1), celled(2, 2));
            let (extent, at) = bucket
                .served(async {
                    branch::create(&*bucket, parent, 0).await.unwrap();
                    let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
                    let extent = ExtentId::new(1, writer.chain().next_sequence());
                    if checkpointed {
                        commit(&writer, Commit { tag: 9, ..Commit::default() }).await;
                        writer.checkpoint().await.unwrap();
                    }
                    let held = vec![(extent, b"held".to_vec())];
                    (extent, commit_and_fork(&writer, held, forked(child)).await)
                })
                .0;
            let options = Options { compact_below: 0.0, grace: 0 };
            let (report, asked) = bucket.served(collect_cell(&*bucket, child.cell, 7, &options));
            report.unwrap();
            let prefix = child.cell.prefix();
            let outside: Vec<&String> = (asked.iter())
                .filter(|asked| asked.starts_with("list"))
                .filter(|asked| !asked.split(' ').nth(1).unwrap().starts_with(&prefix))
                .collect();
            assert_eq!(outside, Vec::<&String>::new(), "checkpointed {checkpointed}");
            let point = format!("get {}", layout::checkpoint(parent, at));
            assert!(asked.contains(&point), "{asked:#?}");
            let direct = Arc::new(Direct(bucket.clone()));
            assert_eq!(block_on(read_inherited(&direct, child, extent)), Ok(b"held".to_vec()));
        }
    }

    /// TASK-395.4's review: a clone refused by a record of its child's ID there before, of another
    /// origin or deleted with none, takes back the reference it wrote, which no record's origin
    /// names; one refused by a record of its own origin leaves it, that record's to retire.
    #[test]
    fn clones_refused_by_records_there_take_their_references_back() {
        use crate::layout::Origin;
        use crate::record::Kind as Made;
        let bucket = Direct(SimBucket::new());
        let (origin, point) = (Origin { parent: celled(1, 1), at: 3 }, celled(2, 2));
        let other = Origin { parent: celled(1, 1), at: 4 };
        let record = layout::record(point);
        block_on(bucket::create(&bucket, &record, layout::encode(&Made::Point(origin).of(0x22))));
        let found = |made: crate::record::Record| layout::encode(&made);
        let cases = [
            (celled(3, 3), found(Made::Fork(other).of(0x44)), Error::IdTaken(celled(3, 3)), false),
            (
                celled(4, 4),
                found(crate::record::Record { deleted: true, ..Made::Own.of(0x45) }),
                Error::Deleted,
                false,
            ),
            (celled(5, 5), found(Made::Fork(origin).of(0x46)), Error::IdTaken(celled(5, 5)), true),
        ];
        for (clone, there, refused, stays) in cases {
            block_on(bucket::create(&bucket, &layout::record(clone), there));
            let forked = block_on(branch::fork_point(&bucket, clone, point, origin, 0x33));
            assert_eq!(forked, Err(refused), "{clone}");
            let reference = layout::reference(origin.parent, origin.at, clone);
            let left = block_on(bucket::get(&bucket, &reference)).is_some();
            assert_eq!(left, stays, "{clone}");
        }
    }

    /// TASK-395.4's review: a pass over a cell holding nothing takes no lease there, which would be
    /// its one object; one over a cell of another format's records refuses before taking it.
    #[test]
    fn passes_take_no_lease_of_cells_empty_or_of_other_formats() {
        let bucket = Direct(SimBucket::new());
        let (empty, older) = (layout::Cell { tenant: 1, id: 7 }, layout::Cell { tenant: 1, id: 8 });
        let options = Options { compact_below: 0.0, grace: 0 };
        assert_eq!(block_on(collect_cell(&bucket, empty, 1, &options)), Ok(Report::default()));
        let branch = Branch { cell: older, id: 9 };
        let mut format8 = 8u32.to_le_bytes().to_vec();
        format8.extend_from_slice(b"what format 8 held");
        block_on(bucket::create(&bucket, &layout::record(branch), format8));
        let refused = block_on(collect_cell(&bucket, older, 1, &options));
        assert_eq!(refused, Err(Error::Format { branch, format: 8 }));
        // A cell's object of format 8, whatever its records', refused as such.
        let celled = layout::Cell { tenant: 1, id: 9 };
        block_on(bucket::create(&bucket, &layout::cell(celled), 8u32.to_le_bytes().to_vec()));
        let refused = block_on(collect_cell(&bucket, celled, 1, &options));
        let branch = Branch { cell: celled, id: 0 };
        assert_eq!(refused, Err(Error::Format { branch, format: 8 }));
        let leases: Vec<String> = (bucket.0.keys().into_iter())
            .filter(|key| matches!(layout::kind(key), Kind::GcLease { .. }))
            .collect();
        assert_eq!(leases, Vec::<String>::new());
    }

    /// TASK-395.4's review, found by the simulation (seed 11 of `restores`): a fork across cells
    /// recorded again, by a later writer of its parent or the parent's deletion, after the parent
    /// checkpointed past the fork point and passes reclaimed its segments up to there, the
    /// checkpoint at the fork point kept: it records nothing more, recovering nothing of the
    /// parent there, which it could not. Before, recording it again recovered the parent at the
    /// fork point from its segments, gone, and failed the writer's acknowledgement.
    #[test]
    fn forks_across_cells_recorded_again_recover_nothing_of_their_parents() {
        block_on(async {
            let bucket = store();
            let (parent, child) = (celled(1, 1), celled(2, 2));
            branch::create(&*bucket, parent, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, writer.chain().next_sequence());
            let at =
                commit_and_fork(&writer, vec![(extent, b"held".to_vec())], forked(child)).await;
            commit(&writer, Commit { tag: 3, frees: vec![extent], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
            sweep(&*bucket).await.unwrap();
            let left = keys(&*bucket).await;
            assert!(!left.contains(&layout::segment(parent, at)), "the fork's segment went");
            assert!(left.contains(&layout::checkpoint(parent, at)), "the fork point kept");
            let origin = crate::layout::Origin { parent, at };
            assert_eq!(branch::record_fork(&*bucket, forked(child), origin).await, Ok(()));
            // Its checkpoint at the fork point gone too, as nothing keeps it for a record of the
            // fork's owner no reference keeps, a failed fork's claim landing late (seed 11's).
            let point = layout::checkpoint(parent, at);
            bucket::delete_all(&*bucket, &[point.as_str()]).await;
            assert_eq!(branch::record_fork(&*bucket, forked(child), origin).await, Ok(()));
            drop(writer);
            assert_eq!(branch::delete(&*bucket, parent, 20, 0).await, Ok(()));
        });
    }

    /// TASK-395.11: a fork across cells whose parent's state at the fork point is gone, as a
    /// writer's state naming a fork its branch's checkpoints don't (sim restores seed 11), records
    /// its child deleted, as a failed fork's cleanup does: no reader recovers it, nor fails on it.
    /// (Here the parent's own checkpoint names the fork, which a pass over its cell then keeps,
    /// so none runs: seed 11's did not.)
    #[test]
    fn forks_across_cells_of_a_state_gone_are_recorded_deleted() {
        block_on(async {
            let bucket = store();
            let (parent, child, failed) = (celled(1, 1), celled(2, 2), celled(2, 3));
            branch::create(&*bucket, parent, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), parent, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, writer.chain().next_sequence());
            let at =
                commit_and_fork(&writer, vec![(extent, b"held".to_vec())], forked(child)).await;
            commit(&writer, Commit { tag: 3, frees: vec![extent], ..Commit::default() }).await;
            writer.checkpoint().await.unwrap();
            sweep(&*bucket).await.unwrap();
            let origin = crate::layout::Origin { parent, at };
            // The fork forgotten: no record, no reference, no checkpoint at its point.
            let gone = [
                layout::record(child),
                layout::reference(parent, at, child),
                layout::checkpoint(parent, at),
            ];
            bucket::delete_all(&*bucket, &gone.iter().map(String::as_str).collect::<Vec<_>>())
                .await;
            assert!(!keys(&*bucket).await.contains(&layout::segment(parent, at)));
            assert_eq!(branch::record_fork(&*bucket, forked(child), origin).await, Ok(()));
            let recorded = branch::record(&*bucket, child).await.unwrap().unwrap();
            assert_eq!((recorded.deleted, recorded.origin()), (true, Some(origin)));
            assert!(!keys(&*bucket).await.contains(&layout::checkpoint(parent, at)));
            // A failed fork there to another cell is deleted from the first, not claimed.
            let owner = forked(failed).owner;
            let deleted = branch::delete_fork(
                &*bucket,
                failed,
                crate::record::Kind::Fork(origin),
                owner,
                30,
                0,
            );
            assert_eq!(deleted.await, Ok(true));
            let recorded = branch::record(&*bucket, failed).await.unwrap().unwrap();
            assert_eq!((recorded.deleted, recorded.origin()), (true, Some(origin)));
            drop(writer);
            assert_eq!(branch::delete(&*bucket, parent, 20, 0).await, Ok(()));
        });
    }
}
