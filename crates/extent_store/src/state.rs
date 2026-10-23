// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An open branch's state, as its store holds it ([`State`]), which its lock guards: its extents,
//! epoch and root, and an owner of each part of the rest, which keeps that part's invariants: the
//! room its extents take ([`Space`]), what it changed since its last commit ([`Uncommitted`]), the
//! gate writes pass ([`Gate`]), its segments ([`Segments`]), its extents' own files
//! ([`OwnFiles`]), its index ([`IndexLog`]), what its next commit makes unreferenced
//! ([`Unreferenced`]); fxdax: what it keeps for its uploader ([`Uploads`]), the frees it keeps
//! ([`KeptExtents`]), and what views refuse ([`Sealed`]).

use crate::dir::{Dir, File};
use crate::durability::Syncs;
use crate::framed::{Files, Framed};
use crate::index::IndexLog;
use crate::kept::{Kept, KeptExtents};
use crate::layers::Extents;
use crate::layout::{Change, Data, EXTENTS, Extent, FileKey, Piece, branch_dir, extent_path};
use crate::segments::Segments;
use crate::store::{Info, NoSpace, TooSmall};
use crate::sync::rules;
use crate::uploads::Uploads;
use crate::views::{Keeping, NotHeld, Sealed};
use anyhow::{Error, bail};
use std::collections::HashMap;
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent};

/// An open branch's state. See the module's docs.
pub(crate) struct State<F> {
    /// Its extents: over the state it shares with its parent at the fork and its forks, if any
    /// (TASK-31).
    pub extents: Extents,
    /// The room they take.
    pub space: Space,
    /// The last epoch committed.
    pub epoch: u64,
    /// The root as of the last commit.
    pub root: Vec<u8>,
    pub next_sequence: u64,
    /// What it changed since the last commit, which the next records.
    pub uncommitted: Uncommitted<F>,
    /// The gate writes pass.
    pub gate: Gate,
    /// The segments it tracks, and the one taking writes.
    pub segments: Segments<F>,
    /// The files of its extents stored on their own.
    pub own_files: OwnFiles<F>,
    /// The index the next commit appends its record to.
    pub index: IndexLog<F>,
    /// The files the next commit makes unreferenced.
    pub unreferenced: Unreferenced,
    /// fxdax: what an uploader takes, if one does.
    pub uploads: Option<Uploads>,
    /// fxdax: the extents freed whose data stays.
    pub kept: KeptExtents,
    /// fxdax: the extents views refuse, sealed after their commits.
    pub sealed: Sealed,
    /// Whether a commit failed, after which the store's state may differ from what its files say,
    /// so it does nothing more.
    pub failed: bool,
    /// Whether it was closed ([`Store::close`](crate::Store::close)): it reads what it holds, and
    /// changes nothing.
    pub closed: bool,
    /// fxdax: the segments closed between commits being framed, holding no lock
    /// ([`Store::close_now`](crate::Store::close_now)): a fork, and the branch's closing, wait for
    /// them (TASK-41.1).
    pub framings: usize,
}

/// Whom a read is for: the store's own, of what it holds; the uploader's, of what it holds or
/// keeps; or a view's, as of the commit of `epoch`, of what that commit and those since `since`
/// held, which the store holds or keeps, and of what it pins, if `pinned`: decided in the critical
/// section that locates it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reader {
    Store,
    Uploader,
    View { epoch: u64, since: u64, pinned: bool },
}

impl<F: File> State<F> {
    /// Runs `change`, which changes the state ahead of the files it writes: failing part way, it
    /// leaves the state other than the files say, and fails the store, which does nothing more.
    pub fn failing<T>(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let changed = change(self);
        self.failed |= changed.is_err();
        changed
    }

    /// `extent`'s entry, which `reader` may read: of what the store holds, or keeps, if not the
    /// store's own; a view's, of what its commits hold (not open, not sealed after its commit, not
    /// freed before the one it holds since), or it pins. Fails with `NoExtent` otherwise.
    pub fn entry_for(&self, extent: ExtentId, reader: Reader) -> Result<Extent, Error> {
        if let Reader::View { epoch, .. } = reader
            && (self.sealed.after(extent, epoch) || self.uncommitted.is_open(extent))
        {
            bail!(NoExtent(extent));
        }
        let kept = |kept: &&Kept| match reader {
            Reader::Store => false,
            Reader::Uploader => true,
            // Freed by a commit after the one the view holds since, which held it; or pinned.
            Reader::View { since, pinned, .. } => {
                pinned || kept.freed > since || !rules().views_hold_their_commit
            }
        };
        match (self.extents.get(&extent), self.kept.get(&extent)) {
            (Some(entry), _) => Ok(entry.clone()),
            (None, Some(held)) if kept(&held) => Ok(held.entry.clone()),
            // Kept for another, freed before what the view holds: what it let go of.
            (None, Some(_)) if matches!(reader, Reader::View { .. }) => {
                Err(Error::new(NoExtent(extent)).context(NotHeld(extent)))
            }
            _ => bail!(NoExtent(extent)),
        }
    }

    /// Tracks the segments of `entry`'s data, as the branch lets it go: those it inherited and held
    /// as the state it shares holds them, it holds otherwise from now on.
    pub fn track(&mut self, entry: &Extent) {
        let Data::Pieces(pieces) = &entry.data else { return };
        let base = self.extents.base();
        for piece in pieces {
            self.segments.track(piece.segment, || base.map_or(0, |base| base.live(piece.segment)));
        }
    }

    /// Lets the data of `extent`, freed, go.
    pub fn forget(&mut self, extent: ExtentId, entry: Extent) {
        self.sealed.forget(extent);
        match entry.data {
            Data::Pieces(pieces) => {
                for piece in &pieces {
                    self.segments.let_go(piece);
                }
            }
            Data::Own => {
                self.own_files.forget(extent);
                self.unreferenced.released.push(FileKey::Own(extent));
            }
            Data::Remote => {}
        }
    }

    /// fxdax: the bytes the branch's files take that no extent holds: those of its segments no
    /// extent is in, and those writes since the last commit wrote over (TASK-79.2).
    pub fn garbage(&self) -> u64 {
        self.segments.dead() + self.uncommitted.overwritten()
    }

    /// Fails with [`NoSpace`] unless a write may add `grows` bytes, and write over `overwrites`.
    pub fn check_room(&self, grows: u64, overwrites: u64) -> Result<(), NoSpace> {
        self.space.check(grows, self.kept.for_views(), self.garbage() + overwrites)
    }

    /// What the store holds, as doc-4's `INFO` reports it.
    pub fn info(&self) -> Info {
        self.space.info(self.next_sequence, self.kept.for_views(), self.garbage())
    }

    /// Lets the kept extents freed by commits up to `published`, the last published to views, go
    /// that no one needs any more: neither the uploader, nor views, as `keeping` says.
    /// Those freed since are for the commit that frees them to decide: a view may yet open as of
    /// the last.
    pub fn trim_kept(&mut self, keeping: &Keeping, published: u64) {
        for (extent, kept) in self.kept.trim(keeping, published) {
            self.forget(extent, kept.entry);
        }
    }
}

/// The room a branch's extents take: the most they may hold, as its record says
/// ([`Store::set_capacity`](crate::Store::set_capacity)), the bytes they hold, and those writes
/// in progress may add. A write takes room only if what they hold and may add, with what views
/// keep of the frees, which counts as used, fits. fxdax: and with what the branch's files take
/// that no extent holds (garbage: data freed, or written over, which cleaning has yet to take
/// back) past the capacity's worth, which counts as used too (TASK-79.2): so that the branch's
/// files take about twice its capacity at most, however fast its writer frees and writes again,
/// which then writes no faster than cleaning takes back what is past that.
pub(crate) struct Space {
    capacity: u64,
    /// The bytes extents hold.
    stored: u64,
    /// fxdax: of those, the bytes of the extents the branch wrote (TASK-49).
    own: u64,
    /// The bytes writes in progress may add.
    reserved: u64,
}

impl Space {
    /// The room of a branch of `capacity` bytes whose extents hold `stored`, `own` of them its own.
    pub fn new(capacity: u64, stored: u64, own: u64) -> Self {
        Self { capacity, stored, own, reserved: 0 }
    }

    /// The most bytes its extents may hold.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Fails with [`NoSpace`] unless a write may add `grows` bytes, views keeping `kept_for_views`
    /// of the frees, and the branch's files taking `garbage` bytes no extent holds.
    pub fn check(&self, grows: u64, kept_for_views: u64, garbage: u64) -> Result<(), NoSpace> {
        let used = self.stored + kept_for_views + self.past_slack(garbage);
        if used + self.reserved + grows > self.capacity {
            return Err(NoSpace);
        }
        Ok(())
    }

    /// fxdax: of `garbage`, what is past the capacity's worth, which counts as used (TASK-79.2).
    fn past_slack(&self, garbage: u64) -> u64 {
        garbage.saturating_sub(self.capacity)
    }

    /// Reserves `grows` bytes for a write in progress, which may add them.
    pub fn reserve(&mut self, grows: u64) {
        self.reserved += grows;
    }

    /// Lets the `grows` bytes a write reserved go: it is done.
    pub fn unreserve(&mut self, grows: u64) {
        self.reserved -= grows;
    }

    /// The extents hold `len` bytes more, of the branch's own if `own`.
    pub fn added(&mut self, len: u64, own: bool) {
        self.stored += len;
        if own {
            self.own += len;
        }
    }

    /// The extents hold `len` bytes less, of the branch's own if `own`.
    pub fn removed(&mut self, len: u64, own: bool) {
        self.stored -= len;
        if own {
            self.own -= len;
        }
    }

    /// Fails with [`TooSmall`] unless `capacity` holds what the extents hold, with what writes in
    /// progress may add.
    pub fn check_capacity(&self, capacity: u64) -> Result<(), TooSmall> {
        let held = self.stored + self.reserved;
        if capacity < held {
            return Err(TooSmall { capacity, held });
        }
        Ok(())
    }

    pub fn set_capacity(&mut self, capacity: u64) {
        self.capacity = capacity;
    }

    /// What the store holds, as doc-4's `INFO` reports it: `next_sequence` its next sequence
    /// number, views keeping `kept_for_views` of the frees, which take room as the extents did,
    /// and the branch's files `garbage` bytes no extent holds, what is past the capacity's worth
    /// of which room is not available either.
    pub fn info(&self, next_sequence: u64, kept_for_views: u64, garbage: u64) -> Info {
        let used = self.stored + kept_for_views;
        let unavailable = used + self.past_slack(garbage) + self.reserved;
        Info {
            capacity: self.capacity,
            used,
            available: self.capacity.saturating_sub(unavailable),
            next_sequence,
            own: self.own,
            inherited: self.stored - self.own,
        }
    }
}

/// Where an open extent's data goes: segments, or a raw file of its own.
pub(crate) enum Placement<F> {
    Segment,
    Own(Arc<F>),
}

impl<F> Clone for Placement<F> {
    fn clone(&self) -> Self {
        match self {
            Self::Segment => Self::Segment,
            Self::Own(file) => Self::Own(file.clone()),
        }
    }
}

/// What a branch changed since its last commit, which the next records: its changes, and the
/// extents written since, which are open, with where their data goes, until it seals them.
pub(crate) struct Uncommitted<F> {
    open: HashMap<ExtentId, Open<F>>,
    /// The extents opened so far, which numbers each opening: an extent freed while open, and
    /// written again, is opened anew, and the writes to it before are another opening's.
    openings: u64,
    changes: Vec<Change>,
    /// fxdax: the bytes the open extents' writes wrote over, in segments (TASK-79.2).
    overwritten: u64,
    /// fxdax: cleaning's copies, which the next commit makes their extents' places (TASK-79.2).
    copies: Vec<Copied>,
}

/// fxdax: a copy of cleaning's: of `extent`, sealed, as `entry` was, in `piece`, which the segment
/// holds as extents' data until the next commit makes it the extent's place, or lets it go.
pub(crate) struct Copied {
    pub extent: ExtentId,
    pub entry: Extent,
    pub piece: Piece,
}

/// An extent open: where its data goes, and its opening's number, which the writes to it carry;
/// fxdax: and the bytes its writes wrote over, in segments, which its pieces hold until it is
/// sealed, or freed.
struct Open<F> {
    placement: Placement<F>,
    opening: u64,
    overwritten: u64,
}

impl<F> Default for Uncommitted<F> {
    fn default() -> Self {
        let (open, changes, copies) = (HashMap::new(), Vec::new(), Vec::new());
        Self { open, openings: 0, changes, overwritten: 0, copies }
    }
}

impl<F> Uncommitted<F> {
    /// Where `extent`'s data goes, if it is open.
    pub fn placement(&self, extent: ExtentId) -> Option<&Placement<F>> {
        self.open.get(&extent).map(|open| &open.placement)
    }

    /// Where `extent`'s data goes, and its opening's number, if it is open.
    pub fn opening(&self, extent: ExtentId) -> Option<(Placement<F>, u64)> {
        self.open.get(&extent).map(|open| (open.placement.clone(), open.opening))
    }

    /// Whether `extent` is open.
    pub fn is_open(&self, extent: ExtentId) -> bool {
        self.open.contains_key(&extent)
    }

    /// Whether `extent` is open still in its opening `opening`: neither freed since, nor sealed.
    pub fn is_open_in(&self, extent: ExtentId, opening: u64) -> bool {
        self.open.get(&extent).is_some_and(|open| open.opening == opening)
    }

    /// Whether an extent is open.
    pub fn has_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// Opens `extent`, whose data goes as `placement` says: returns the opening's number.
    pub fn open(&mut self, extent: ExtentId, placement: Placement<F>) -> u64 {
        self.openings += 1;
        let opening = self.openings;
        self.open.insert(extent, Open { placement, opening, overwritten: 0 });
        opening
    }

    /// fxdax: a write to `extent`, open, wrote over `bytes` of what it held, in segments
    /// (TASK-79.2).
    pub fn wrote_over(&mut self, extent: ExtentId, bytes: u64) {
        if let Some(open) = self.open.get_mut(&extent) {
            open.overwritten += bytes;
            self.overwritten += bytes;
        }
    }

    /// fxdax: the bytes the open extents' writes wrote over, in segments: no extent holds them,
    /// though the extents' pieces are in them until their seals let them go (TASK-79.2).
    pub fn overwritten(&self) -> u64 {
        self.overwritten
    }

    /// `extent`, freed, is open no more: where its data went, if it was.
    pub fn remove(&mut self, extent: ExtentId) -> Option<Placement<F>> {
        let open = self.open.remove(&extent)?;
        self.overwritten -= open.overwritten;
        Some(open.placement)
    }

    /// The extents open, which the next commit seals.
    pub fn open_extents(&self) -> Vec<ExtentId> {
        self.open.keys().copied().collect()
    }

    pub fn push(&mut self, change: Change) {
        self.changes.push(change);
    }

    /// fxdax: cleaning copied an extent, for the next commit to make the copy its place.
    pub fn copied(&mut self, copied: Copied) {
        self.copies.push(copied);
    }

    /// fxdax: cleaning's copies that wait for the next commit.
    pub fn copies(&self) -> &[Copied] {
        &self.copies
    }

    /// fxdax: cleaning's copies, for the commit, which makes them their extents' places.
    pub fn take_copies(&mut self) -> Vec<Copied> {
        std::mem::take(&mut self.copies)
    }

    /// The changes since the last commit, for the commit, which seals the extents open.
    pub fn take(&mut self) -> Vec<Change> {
        self.open.clear();
        self.overwritten = 0;
        std::mem::take(&mut self.changes)
    }
}

/// The gate writes pass ([`Store::begin_write`](crate::Store::begin_write)): held while a commit,
/// or a closing between commits ([`Store::close_now`](crate::Store::close_now)), waits for the
/// writes in progress, which new ones wait for; and the writes in progress, to segments or
/// extents' own files: the extents a commit seals hold what they write.
#[derive(Default)]
pub(crate) struct Gate {
    held: bool,
    writing: u64,
}

impl Gate {
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Holds new writes back.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// Lets new writes pass.
    pub fn release(&mut self) {
        self.held = false;
    }

    /// A write passes, and is in progress.
    pub fn pass(&mut self) {
        self.writing += 1;
    }

    /// A write in progress is done, finished or abandoned: returns whether it was the last.
    pub fn done(&mut self) -> bool {
        self.writing -= 1;
        self.writing == 0
    }

    /// Whether writes are in progress.
    pub fn is_writing(&self) -> bool {
        self.writing > 0
    }
}

/// The files of a branch's extents stored on their own, sealed: those open, at most
/// `Options::open_files` but for those reads use, the least lately read closing as more open; the
/// bytes each takes; those written since the last commit, which its flush syncs, with their
/// directory.
pub(crate) struct OwnFiles<F> {
    open: Files<ExtentId, F>,
    sizes: HashMap<ExtentId, u64>,
    unsynced: HashMap<ExtentId, Arc<F>>,
    /// Their directory, the branch's, and whether its entries changed since the last commit.
    dir: String,
    written: bool,
}

impl<F: File> OwnFiles<F> {
    /// `branch`'s, whose files take `sizes`, of which `limit` stay open.
    pub fn new(branch: u64, sizes: HashMap<ExtentId, u64>, limit: usize) -> Self {
        let dir = branch_dir(EXTENTS, branch);
        Self { open: Files::new(limit), sizes, unsynced: HashMap::new(), dir, written: false }
    }

    /// `extent`'s file, opened in `dir` if it isn't.
    pub fn file<D: Dir<File = F>>(
        &mut self,
        dir: &D,
        extent: ExtentId,
    ) -> Result<Arc<Framed<F>>, Error> {
        if let Some(file) = self.open.get(extent) {
            return Ok(file);
        }
        let path = extent_path(extent);
        let file = Arc::new(Framed::open(dir.open(&path, true)?, &path)?);
        Ok(self.open.insert(extent, file))
    }

    /// `extent`'s file is `framed`, just written: its commit's flush syncs it, and its directory.
    pub fn sealed(&mut self, extent: ExtentId, framed: Arc<Framed<F>>) {
        self.sizes.insert(extent, framed.size());
        self.unsynced.insert(extent, framed.file.clone());
        self.open.insert(extent, framed);
        self.written = true;
    }

    /// `extent`'s file goes: the extent was freed.
    pub fn forget(&mut self, extent: ExtentId) {
        self.open.remove(extent);
        self.sizes.remove(&extent);
        self.unsynced.remove(&extent);
    }

    /// Adds to `syncs` what the next commit's flush syncs of them: the files written since the
    /// last commit, and their directory.
    pub fn take_syncs(&mut self, syncs: &mut Syncs<F>) {
        for file in std::mem::take(&mut self.unsynced).into_values() {
            syncs.file(file);
        }
        if std::mem::take(&mut self.written) {
            syncs.dir(self.dir.clone());
        }
    }

    /// The bytes they take.
    pub fn bytes(&self) -> u64 {
        self.sizes.values().sum()
    }
}

/// What a branch's next commit makes unreferenced: files of the branch's alone, which it removes
/// once it is durable; and the files of extents' data it lets go, no extent of the branch being in
/// them, which go once no branch holds them, as the lineage's ledger says
/// ([`Lineage::release`](crate::Lineage::release)).
#[derive(Default)]
pub(crate) struct Unreferenced {
    /// Raw files whose extents were sealed from them, or freed, and index files a checkpoint
    /// replaced: no other branch holds them, so they wait for the branch's commits alone, in its
    /// queue of them, and are forgotten with a failed store's, as its next opening removes what
    /// nothing refers to.
    pub doomed: Vec<String>,
    pub released: Vec<FileKey>,
}

impl Unreferenced {
    /// Those of the commit: from now on, the next's.
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}
