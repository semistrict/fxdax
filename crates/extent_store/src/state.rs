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
use crate::layout::{
    Change, Data, EXTENTS, Extent, FileKey, Piece, SegmentId, branch_dir, extent_path,
};
use crate::segments::{Closing, Segments};
use crate::store::{Info, NoSpace, TooSmall};
use crate::sync::rules;
use crate::uploads::Uploads;
use crate::views::{Keeping, NotHeld, Sealed};
use anyhow::{Error, bail};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// fxdax: the segments closed since the last commit, being framed behind, which the next
    /// commit's flush waits for: those closed for want of room, full between commits (TASK-79.2),
    /// and one a commit that failed closed. The branch's closing waits for them, if no commit
    /// comes after.
    pub closings: Vec<Closing<F>>,
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

    /// fxdax: the bytes of the host's disk the branch's files take that no extent holds
    /// (TASK-79.2): what of its segments no extent is in takes of their files
    /// ([`Segments::dead_on_disk`]), and what writes since the last commit wrote over, counted as
    /// many as it is, the most it takes.
    pub fn garbage(&self) -> u64 {
        self.segments.dead_on_disk() + self.uncommitted.overwritten()
    }

    /// fxdax: whether what the branch's files take of the host's disk that no extent holds is past
    /// the capacity's worth: cleaning is due then for whatever it takes back (TASK-79.2). Counted
    /// as the bytes of data it is, a guest's zeros, freed, which framing compresses to next to none
    /// of the disk, were past it pass after pass, and made cleaning due for no disk it took back.
    pub fn garbage_past_capacity(&self) -> bool {
        self.garbage() > self.space.capacity()
    }

    /// Fails with [`NoSpace`] unless a write may add `grows` bytes, and write over `overwrites`;
    /// fxdax: and add `adds` bytes to what the branch's files take of the host's disk, as
    /// [`Space::check_files`] says (TASK-79.89).
    pub fn check_room(&self, grows: u64, overwrites: u64, adds: u64) -> Result<(), NoSpace> {
        let wasted = self.uncommitted.wasted() + overwrites;
        self.space.check(grows, self.kept.for_views(), wasted)?;
        self.space.check_files(self.file_bytes() + adds, || self.taken_back())
    }

    /// fxdax: the bytes of the host's disk the branch's files take that it alone holds, but what
    /// its uploader keeps (TASK-79.89): its segments', but those another branch may hold extents
    /// in, which are that branch's to let go of ([`Segments::file_bytes`]); its extents' own files;
    /// and the raw files of those open, written in place.
    pub fn file_bytes(&self) -> u64 {
        let own_files = self.own_files.bytes() - self.own_files.for_uploader();
        self.segments.file_bytes() + own_files + self.uncommitted.raw_bytes()
    }

    /// fxdax: `entry`'s data, of an extent freed, is for the uploader to keep, if `kept`, or no
    /// more: what it takes of the host's disk counts not towards what the branch's files take
    /// meanwhile, as the upload lag bound bounds it (TASK-79.89).
    pub fn keep_for_uploader(&mut self, extent: ExtentId, entry: &Extent, kept: bool) {
        match &entry.data {
            Data::Pieces(pieces) => {
                for piece in pieces {
                    self.segments.keep_for_uploader(piece, kept);
                }
            }
            Data::Own => self.own_files.keep_for_uploader(extent, kept),
            Data::Remote => {}
        }
    }

    /// fxdax: the uploader needs the frees of the commits up to `epoch` no more: what their data
    /// takes of the host's disk is the branch's again, until trims let it go (TASK-79.89).
    pub fn release_kept(&mut self, epoch: u64) {
        for extent in self.kept.release(epoch) {
            let entry = self.kept.get(&extent).expect("an extent released is kept").entry.clone();
            self.keep_for_uploader(extent, &entry, false);
        }
    }

    /// fxdax: the bytes of the host's disk the next commit takes back of the branch's files as
    /// cleaning's copies land, which wait for it ([`Copied::lands`]): those of the segments they
    /// leave no extent in, which it lets go of (TASK-79.89). Found going through the copies, at
    /// most what a pass of cleaning copies, one extent each.
    pub fn taken_back(&self) -> u64 {
        let open = self.segments.open_segment();
        let mut moved = BTreeMap::new();
        for copied in self.uncommitted.copies() {
            if !copied.lands(open, self.extents.get(&copied.extent)) {
                continue;
            }
            let Data::Pieces(pieces) = &copied.entry.data else {
                unreachable!("copies are of segments'")
            };
            for piece in pieces {
                *moved.entry(piece.segment).or_default() += piece.len;
            }
        }
        let uploads = &self.uploads;
        self.segments.emptied_by(&moved, |segment| uploader_keeps(uploads, open.branch, segment))
    }

    /// What the store holds, as doc-4's `INFO` reports it.
    pub fn info(&self) -> Info {
        self.space.info(self.next_sequence, self.kept.for_views(), self.uncommitted.wasted())
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

/// fxdax: how many times its capacity a branch's files may take of the host's disk before writes
/// are refused ([`Space`], TASK-79.89).
pub(crate) const FILES_PER_CAPACITY: u64 = 3;

/// The room a branch's extents take: the most they may hold, as its record says
/// ([`Store::set_capacity`](crate::Store::set_capacity)), the bytes they hold, and those writes
/// in progress may add. A write takes room only if what they hold and may add, with what views
/// keep of the frees, which counts as used, fits. fxdax: and with what writes since the last
/// commit wasted past the capacity's worth, which counts as used too (TASK-79.2): what they wrote
/// over, in segments, and what frees of extents open let go of, and writes dropped, of what they
/// wrote. That is its writer's own, which no commit held, and which its next commit takes back:
/// so that a writer that never commits takes twice the capacity of the host's disk at most,
/// rewriting, or writing extents and freeing them. Past it, the writes a commit comes with are
/// refused too (doc-4's `WRITE_COMMIT`'s, and those of the extents a DAX guest maps writably,
/// which its commits write first), so that a writer past it may be unable to commit: a guest's
/// filesystem comes nowhere near it, Fxfs freeing an extent only once a flush has committed it,
/// and writing over little between commits. What the commits leave in the branch's segments
/// that no extent is in, freed, takes none of the room, however much: cleaning takes it back,
/// behind the commits that come with the writer's writes, which a writer refused room for it
/// could neither make nor know to. Counted as used past the capacity's worth, it found a guest's
/// store full with a tenth of it used (`ENOSPC`), which the guest's filesystem did not survive.
///
/// fxdax: what the branch's own files take of the host's disk is bounded apart from the room
/// (TASK-79.89): a write is refused once they ([`State::file_bytes`]), with what it adds to them,
/// would take more than [`FILES_PER_CAPACITY`] times the capacity, counted as they take the disk,
/// framed and compressed, less what the next commit takes back of them as cleaning's copies land
/// ([`State::taken_back`]), a capacity's worth at most. Bounded by cleaning's pace alone across
/// commits, they grew without bound under a writer freeing faster than cleaning took back. What
/// an uploader keeps counts not towards it, as the disk it takes: the segments it has yet to
/// upload, and of extents freed, what their data takes of the segments' files and their own, until
/// it releases the commits that freed them. The upload lag bound bounds that, having commits wait
/// for the uploader rather than fail: counted, it found an honest guest's writes refused as its
/// uploader lagged, its extents holding a third of its capacity. An honest writer's own files come
/// nowhere near the bound: its extents hold its capacity at most, and what no extent is in, about
/// as much again, cleaning being due once it takes the capacity's worth
/// ([`State::garbage_past_capacity`]). What the next commit takes back counts as taken back for
/// the writes it comes with, which the bound would refuse otherwise, and so the commit: the
/// copies, which wait for it, would never land, and the store would stay at the bound for good. A
/// capacity's worth is the most those writes need, as the room bounds them; and what bounds the
/// files should the copies not land, a segment closed between commits taking them, which no footer
/// names as their extents' places. So the branch's files take three to four times the capacity at
/// most, as each write begins, and what the upload lag bound lets its uploader keep. Commits, and
/// cleaning's copies, are never refused: the files take more by what cleaning copies, until the
/// commits after take it back, by what a commit writes again sealing the extents written in
/// pieces, and by the frames of the segments it closes, which say what they hold.
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
    /// of the frees, and writes since the last commit, this one's included, wasting `wasted`
    /// bytes.
    pub fn check(&self, grows: u64, kept_for_views: u64, wasted: u64) -> Result<(), NoSpace> {
        let used = self.stored + kept_for_views + self.past_slack(wasted);
        if used + self.reserved + grows > self.capacity {
            return Err(NoSpace);
        }
        Ok(())
    }

    /// fxdax: fails with [`NoSpace`] if the branch's files, taking `file_bytes` of the host's disk
    /// with a write's, less what the next commit takes back of them, `taken_back()`, a capacity's
    /// worth at most, would take more than [`FILES_PER_CAPACITY`] times the capacity (TASK-79.89).
    /// What it takes back is found only if the files take more without it, as it goes through
    /// cleaning's copies.
    pub fn check_files(
        &self,
        file_bytes: u64,
        taken_back: impl FnOnce() -> u64,
    ) -> Result<(), NoSpace> {
        let bound = self.capacity.saturating_mul(FILES_PER_CAPACITY);
        if file_bytes <= bound || file_bytes - taken_back().min(self.capacity) <= bound {
            return Ok(());
        }
        Err(NoSpace)
    }

    /// fxdax: of `wasted`, what is past the capacity's worth, which counts as used (TASK-79.2).
    fn past_slack(&self, wasted: u64) -> u64 {
        wasted.saturating_sub(self.capacity)
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
    /// and writes since the last commit wasting `wasted` bytes, what is past the capacity's worth
    /// of which room is not available either.
    pub fn info(&self, next_sequence: u64, kept_for_views: u64, wasted: u64) -> Info {
        let used = self.stored + kept_for_views;
        let unavailable = used + self.past_slack(wasted) + self.reserved;
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
    /// fxdax: the bytes writes since the last commit wrote that no extent holds any more, which
    /// no commit held: of the extents freed while open, in segments, what they wrote over too, or
    /// in raw files of their own; and of the writes dropped, in segments (TASK-79.2).
    let_go: u64,
    /// fxdax: cleaning's copies, which the next commit makes their extents' places (TASK-79.2).
    copies: Vec<Copied>,
    /// fxdax: the bytes the open extents' raw files take (TASK-79.89).
    raw: u64,
}

/// fxdax: a copy of cleaning's: of `extent`, sealed, as `entry` was, in `piece`, which the segment
/// holds as extents' data until the next commit makes it the extent's place, or lets it go.
pub(crate) struct Copied {
    pub extent: ExtentId,
    pub entry: Extent,
    pub piece: Piece,
}

impl Copied {
    /// Whether the next commit makes it its extent's place, the extent being `current`, if held,
    /// and `open` the open segment: if it is in the open segment, whose footer names it then, and
    /// the extent is as it was copied. What the others copied is dead space.
    pub fn lands(&self, open: SegmentId, current: Option<&Extent>) -> bool {
        let placed = self.piece.segment == open || !rules().moves_into_the_open_segment;
        placed && current == Some(&self.entry)
    }
}

/// An extent open: where its data goes, and its opening's number, which the writes to it carry;
/// fxdax: and the bytes its writes wrote over, in segments, which its pieces hold until it is
/// sealed, or freed; and those its raw file takes, if it has one (TASK-79.89).
struct Open<F> {
    placement: Placement<F>,
    opening: u64,
    overwritten: u64,
    raw: u64,
}

impl<F> Default for Uncommitted<F> {
    fn default() -> Self {
        let (open, changes, copies) = (HashMap::new(), Vec::new(), Vec::new());
        Self { open, openings: 0, changes, overwritten: 0, let_go: 0, copies, raw: 0 }
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
        self.open.insert(extent, Open { placement, opening, overwritten: 0, raw: 0 });
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

    /// fxdax: `extent`, open, has its raw file take `end` bytes, if it took fewer: a write to it
    /// that ends there began (TASK-79.89).
    pub fn raw_to(&mut self, extent: ExtentId, end: u64) {
        if let Some(open) = self.open.get_mut(&extent)
            && end > open.raw
        {
            self.raw += end - open.raw;
            open.raw = end;
        }
    }

    /// fxdax: the bytes `extent`'s raw file takes, if it is open, in one (TASK-79.89).
    pub fn raw_of(&self, extent: ExtentId) -> u64 {
        self.open.get(&extent).map_or(0, |open| open.raw)
    }

    /// fxdax: the bytes the open extents' raw files take, which the next commit frames in files of
    /// their own, and whose flush removes them (TASK-79.89).
    pub fn raw_bytes(&self) -> u64 {
        self.raw
    }

    /// fxdax: `bytes` that writes since the last commit wrote are no extent's any more: of an
    /// extent freed while open, or of a write dropped (TASK-79.2).
    pub fn let_go(&mut self, bytes: u64) {
        self.let_go += bytes;
    }

    /// fxdax: the bytes writes since the last commit wasted: what the open extents' writes wrote
    /// over, and what those freed, and writes dropped, let go of (TASK-79.2). The next commit takes
    /// them back: of segments set aside, which it removes, or as dead space, which cleaning takes
    /// back behind the commits; and of raw files, which its flush removes.
    pub fn wasted(&self) -> u64 {
        self.overwritten + self.let_go
    }

    /// `extent`, freed, is open no more: where its data went, if it was. fxdax: what its writes
    /// wrote over is let go of with the rest of what they wrote, which its free lets go of; and its
    /// raw file, if it has one, which the next commit's flush removes.
    pub fn remove(&mut self, extent: ExtentId) -> Option<Placement<F>> {
        let open = self.open.remove(&extent)?;
        self.overwritten -= open.overwritten;
        self.raw -= open.raw;
        Some(open.placement)
    }

    /// The extents open, which the next commit seals, in the order they were opened. fxdax: an
    /// order the branch's writes decide, not the map's, which differs from one run to the next,
    /// so that where a commit seals each, and what its record says, do not (TASK-79.79).
    pub fn open_extents(&self) -> Vec<ExtentId> {
        let mut open: Vec<_> =
            self.open.iter().map(|(extent, open)| (open.opening, *extent)).collect();
        open.sort_unstable();
        open.into_iter().map(|(_, extent)| extent).collect()
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
        self.let_go = 0;
        self.raw = 0;
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
    /// fxdax: what they take in all, kept as `sizes` changes, so that a write may count it as it
    /// begins, not going through every file (TASK-79.89).
    bytes: u64,
    /// fxdax: those of extents freed that an uploader keeps, until it releases the commits that
    /// freed them, and what they take in all (TASK-79.89).
    for_uploader: HashSet<ExtentId>,
    for_uploader_bytes: u64,
    /// fxdax: in order, which their flush syncs them in, rather than the order of a map that
    /// differs from one run to the next (TASK-79.79).
    unsynced: BTreeMap<ExtentId, Arc<F>>,
    /// Their directory, the branch's, and whether its entries changed since the last commit.
    dir: String,
    written: bool,
}

impl<F: File> OwnFiles<F> {
    /// `branch`'s, whose files take `sizes`, of which `limit` stay open.
    pub fn new(branch: u64, sizes: HashMap<ExtentId, u64>, limit: usize) -> Self {
        let (dir, bytes) = (branch_dir(EXTENTS, branch), sizes.values().sum());
        let (open, unsynced) = (Files::new(limit), BTreeMap::new());
        let (for_uploader, for_uploader_bytes) = (HashSet::new(), 0);
        Self { open, sizes, bytes, for_uploader, for_uploader_bytes, unsynced, dir, written: false }
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
        let replaced = self.sizes.insert(extent, framed.size());
        self.bytes = self.bytes - replaced.unwrap_or(0) + framed.size();
        self.unsynced.insert(extent, framed.file.clone());
        self.open.insert(extent, framed);
        self.written = true;
    }

    /// `extent`'s file goes: the extent was freed.
    pub fn forget(&mut self, extent: ExtentId) {
        self.open.remove(extent);
        let size = self.sizes.remove(&extent).unwrap_or(0);
        self.bytes -= size;
        if self.for_uploader.remove(&extent) {
            self.for_uploader_bytes -= size;
        }
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
        self.bytes
    }

    /// fxdax: `extent`'s file, of an extent freed, is for the uploader to keep, if `kept`, or no
    /// more (TASK-79.89).
    pub fn keep_for_uploader(&mut self, extent: ExtentId, kept: bool) {
        let Some(&size) = self.sizes.get(&extent) else { return };
        let changed =
            if kept { self.for_uploader.insert(extent) } else { self.for_uploader.remove(&extent) };
        if changed {
            self.for_uploader_bytes =
                if kept { self.for_uploader_bytes + size } else { self.for_uploader_bytes - size };
        }
    }

    /// fxdax: the bytes of those an uploader keeps (TASK-79.89).
    pub fn for_uploader(&self) -> u64 {
        self.for_uploader_bytes
    }
}

/// fxdax: whether `branch`'s uploader, if `uploads` say it has one, keeps `segment`, the branch's
/// own, which it has yet to upload: the branch lets go of it no sooner (TASK-85).
pub(crate) fn uploader_keeps(uploads: &Option<Uploads>, branch: u64, segment: SegmentId) -> bool {
    segment.branch == branch
        && uploads.as_ref().is_some_and(|uploads| uploads.holds(segment.number))
}

/// What a branch's next commit makes unreferenced: files of the branch's alone, which it removes
/// once it is durable; and the files of extents' data it lets go, no extent of the branch being in
/// them, which go once no branch holds them, as the lineage's ledger says
/// ([`Lineage::release`](crate::Lineage::release)).
#[derive(Default)]
pub(crate) struct Unreferenced {
    /// Raw files whose extents were sealed from them, or freed, and index files a checkpoint
    /// replaced; fxdax: and the raw files of segments closed between commits, framed: no other
    /// branch holds them, so they wait for the branch's commits alone, in its queue of them, and
    /// are forgotten with a failed store's, as its next opening removes what nothing refers to.
    pub doomed: Vec<String>,
    pub released: Vec<FileKey>,
}

impl Unreferenced {
    /// Those of the commit: from now on, the next's.
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}
