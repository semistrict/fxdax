// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An open branch's state, as its store holds it ([`State`]), which its lock guards: its extents,
//! epoch and root, and an owner of each part of the rest, which keeps that part's invariants: the
//! room its extents take ([`Space`]), what it changed since its last commit ([`Uncommitted`]), the
//! gate writes pass ([`Gate`]), its segments ([`Segments`]), its extents' own files
//! ([`OwnFiles`]), what its next commit makes unreferenced ([`Unreferenced`]); fxdax: its
//! write-ahead log's segments not yet acknowledged in the bucket ([`Queue`], TASK-165.9), the
//! frees it keeps ([`KeptExtents`]), and what views refuse ([`Sealed`]). fxdax: the state locked
//! ([`Locked`]), which the builds of the tests audit as it lets it go ([`State::audit`],
//! TASK-160.16).

use crate::dir::{Dir, File};
use crate::durability::Syncs;
use crate::framed::{Files, Framed};
use crate::kept::{Kept, KeptExtents};
use crate::layout::{
    Change, Data, EXTENTS, Extent, Extents, FileKey, Piece, SegmentId, branch_dir, extent_path,
};
#[cfg(any(test, feature = "audit"))]
use crate::segments::Recount;
use crate::segments::{Segments, Unframed};
#[cfg(any(test, feature = "audit"))]
use crate::store::covered;
use crate::store::{Info, NoSpace, TooSmall};
use crate::sync::{Condvar, MutexGuard, rules};
use crate::views::{Keeping, NotHeld, Sealed};
use crate::wal::Queue;
use anyhow::{Error, bail};
use extent_frame::segment::Location;
use extent_frame::spent::Spent;
#[cfg(any(test, feature = "audit"))]
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent};

/// An open branch's state. See the module's docs.
pub(crate) struct State<F> {
    /// Its extents.
    pub extents: Extents,
    /// The room they take.
    pub space: Space,
    /// The last epoch committed.
    pub epoch: u64,
    /// The root as of the last commit.
    pub root: Vec<u8>,
    pub next_sequence: u64,
    /// fxdax: the sequence numbers of its own extents it opens no more (TASK-79.6).
    pub spent: Spent,
    /// What it changed since the last commit, which the next records.
    pub uncommitted: Uncommitted<F>,
    /// The gate writes pass.
    pub gate: Gate,
    /// The segments it tracks, and the one taking writes.
    pub segments: Segments<F>,
    /// The files of its extents stored on their own.
    pub own_files: OwnFiles<F>,
    /// The files the next commit makes unreferenced.
    pub unreferenced: Unreferenced,
    /// fxdax: its write-ahead log's segments not yet acknowledged in the bucket, which the
    /// uploader takes (TASK-165.9).
    pub wal: Queue,
    /// fxdax: whether it holds a point, which nothing writes (doc-7, Points).
    pub point: bool,
    /// fxdax: the extents freed whose data stays.
    pub kept: KeptExtents,
    /// fxdax: the extents views refuse, sealed after their commits.
    pub sealed: Sealed,
    /// Whether a commit failed, after which the store's state may differ from what its files say,
    /// so it does nothing more. fxdax: and why, the first failure's error, which what it refuses
    /// says (TASK-165.9: a framing's failure, which no flush waits for, failed a later call).
    pub failed: Option<String>,
    /// Whether it was closed ([`Store::close`](crate::Store::close)): it reads what it holds, and
    /// changes nothing.
    pub closed: bool,
    /// fxdax: the segments closed for want of room since the last commit, which the next frames,
    /// with the rule that framings end themselves off alone (TASK-150.2's shape, TASK-153.8).
    pub deferred: Vec<Unframed<F>>,
    /// fxdax: the framings to run, one at a time, in the order their segments closed
    /// (TASK-165.9).
    pub framing: Framing<F>,
    /// fxdax: the branch as the replay of its footers has it, from its opening on, in the builds
    /// of the tests ([`Replay`], TASK-165.9).
    #[cfg(any(test, feature = "audit"))]
    pub replay: Replay,
}

/// fxdax: a branch as the one replay of its segments' footers has it, the bucket's and its
/// write-ahead log's from its store's opening on, its extents by their lengths: which the state,
/// as of each commit that closes a segment, must hold, the audit asserts (TASK-165.9's AC #5).
#[cfg(any(test, feature = "audit"))]
pub(crate) type Replay = extent_remote::State<Lengths>;

/// fxdax: the lengths of a branch's extents, as a replay of its footers places them ([`Replay`]).
#[cfg(any(test, feature = "audit"))]
#[derive(Default)]
pub(crate) struct Lengths(pub BTreeMap<ExtentId, u64>);

#[cfg(any(test, feature = "audit"))]
impl extent_remote::recovery::Places for Lengths {
    fn seal(&mut self, extent: ExtentId, seal: extent_remote::recovery::Seal) {
        let (extent_remote::recovery::Seal::Run(at) | extent_remote::recovery::Seal::Own(at)) =
            seal;
        self.0.insert(extent, at.len);
    }

    fn free(&mut self, extent: ExtentId) {
        self.0.remove(&extent);
    }
}

/// fxdax: the segments closed whose framings are yet to run, in the order they closed, and whether
/// a framer runs them, one after another (`wal`'s docs, TASK-165.9).
pub(crate) struct Framing<F> {
    pub queue: std::collections::VecDeque<Unframed<F>>,
    pub running: bool,
}

impl<F> Default for Framing<F> {
    fn default() -> Self {
        Self { queue: std::collections::VecDeque::new(), running: false }
    }
}

/// Whom a read is for: the store's own, of what it holds; of what it holds or keeps, freed, for
/// views or for the bucket ([`Store::read_kept`](crate::Store::read_kept)); or a view's, as of the commit of `epoch`, of what that commit and those since `since`
/// held, which the store holds or keeps, and of what it pins, if `pinned`: decided in the critical
/// section that locates it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reader {
    Store,
    Kept,
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
        if let Err(error) = &changed {
            self.fail(error);
        }
        changed
    }

    /// fxdax: fails the store, for `error`, unless it failed before ([`State::failed`]).
    pub fn fail(&mut self, error: &Error) {
        self.failed.get_or_insert_with(|| format!("{error:#}"));
    }

    /// fxdax: fails, saying why, if the store failed ([`State::failed`]).
    pub fn still_whole(&self) -> Result<(), Error> {
        match &self.failed {
            Some(cause) => Err(anyhow::anyhow!("the extent store failed: {cause}")),
            None => Ok(()),
        }
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
            Reader::Kept => true,
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
                self.unreferenced.doomed.extend(FileKey::Own(extent).paths());
            }
            Data::Remote(_) => {}
        }
    }

    /// fxdax: the extents `moved` says the collector moved, from where to where, held or kept, named
    /// where they went, if they are named on their way there ([`Store::moved`](crate::Store::moved)).
    pub fn moved(&mut self, moved: &[(ExtentId, Location, Location)]) {
        for (extent, _, to) in moved {
            if let Some(entry) = self.extents.get_mut(extent) {
                entry.data.follow(to);
            }
            self.kept.moved(*extent, to);
        }
    }

    /// fxdax: segment `segment`, closed, is acknowledged in the bucket, at `key`, and the extents
    /// stored on their own its footer names are at `own` there (TASK-165.8): its extents sealed in
    /// it, and those stored on their own, held or kept, are the bucket's from now on, read there,
    /// and their files leave the store, for the host's cache if `cached`, its framed file being
    /// the object at `key`. Returns those that leave now, by their paths, with their keys in the
    /// bucket, if into the cache: the extents' own files, those no extent holds nor keeps any more
    /// too, which the segment kept for the bucket (TASK-165.9); the segment's leaves once no
    /// extent's data is in it here, which writes since the last commit may be
    /// ([`Reaped::Acknowledged`](crate::segments::Reaped)).
    pub fn acknowledge(
        &mut self,
        segment: SegmentId,
        (key, cached): (&str, bool),
        own: &[(ExtentId, Location)],
    ) -> Vec<(String, Option<String>)> {
        let in_bucket = |len, piece: &Piece| {
            Data::Remote(Location { key: key.to_owned(), offset: piece.offset, len })
        };
        let cached_key = |key: &str| cached.then(|| key.to_owned());
        for extent in self.segments.acknowledge(segment, cached_key(key).as_deref()) {
            let entry =
                self.extents.get_mut(&extent).expect("an extent sealed in a segment is held");
            let Data::Pieces(pieces) = &entry.data else {
                unreachable!("{extent:?}, sealed, is not")
            };
            let [piece] = pieces[..] else { unreachable!("{extent:?}, sealed, is not in one run") };
            let held = std::mem::replace(&mut entry.data, in_bucket(entry.len, &piece));
            self.segments.displace(extent, &held);
            self.segments.let_go(&piece);
        }
        let mut leaving = Vec::new();
        for (extent, location) in own {
            if let Some(entry) = self.extents.get_mut(extent)
                && entry.data == Data::Own
            {
                entry.data = Data::Remote(location.clone());
                self.own_files.forget(*extent);
                leaving.push((extent_path(*extent), cached_key(&location.key)));
            } else if self.kept.get(extent).is_none_or(|kept| kept.entry.data != Data::Own) {
                // Freed, and kept no more, or since an opening: the file the bucket needed alone.
                leaving.push((extent_path(*extent), None));
            }
        }
        let own: HashMap<ExtentId, &Location> =
            own.iter().map(|(extent, location)| (*extent, location)).collect();
        // What it keeps of extents freed is read there too.
        let mut kept = Vec::new();
        for (extent, held) in self.kept.entries_mut() {
            let placed = match &held.entry.data {
                Data::Pieces(pieces) => match pieces[..] {
                    [piece] if piece.segment == segment => Some(in_bucket(held.entry.len, &piece)),
                    _ => None,
                },
                Data::Own => own.get(extent).map(|location| Data::Remote((*location).clone())),
                Data::Remote(_) => None,
            };
            if let Some(placed) = placed {
                kept.push((*extent, std::mem::replace(&mut held.entry.data, placed)));
            }
        }
        for (extent, data) in kept {
            match data {
                Data::Pieces(pieces) => self.segments.let_go(&pieces[0]),
                Data::Own => {
                    self.own_files.forget(extent);
                    leaving.push((extent_path(extent), cached_key(&own[&extent].key)));
                }
                Data::Remote(_) => unreachable!("kept, placed in the bucket again"),
            }
        }
        leaving
    }

    /// Fails with [`NoSpace`] unless a write may add `grows` bytes, and write over `overwrites`.
    pub fn check_room(&self, grows: u64, overwrites: u64) -> anyhow::Result<()> {
        let wasted = self.uncommitted.wasted() + overwrites;
        self.space.check(grows, self.kept.for_views(), wasted)
    }

    /// fxdax: the bucket needs the frees of the commits up to `epoch` no more, acknowledged there:
    /// trims let them go once views need them no more either.
    pub fn release_kept(&mut self, epoch: u64) {
        self.kept.release(epoch);
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

/// fxdax: the state, locked, which the builds of the tests audit as they let it go, waiting or
/// done ([`State::audit`], TASK-160.16): so that whatever changed it, its counts are found right
/// before anything else reads them. Unless the store failed, which changes nothing more, its
/// state other than its files say; or the thread is panicking, which another panic would abort.
pub(crate) struct Locked<'a, F: File> {
    /// None only as it waits ([`Locked::wait`]).
    guard: Option<MutexGuard<'a, State<F>>>,
}

impl<'a, F: File> Locked<'a, F> {
    pub fn new(guard: MutexGuard<'a, State<F>>) -> Self {
        Self { guard: Some(guard) }
    }

    /// Waits for `condvar`, letting the state go meanwhile, and returns it, locked again.
    pub fn wait(mut self, condvar: &Condvar) -> Self {
        let guard = audited(self.guard.take().expect("the state is locked"));
        Self::new(condvar.wait(guard).unwrap())
    }
}

impl<F: File> Deref for Locked<'_, F> {
    type Target = State<F>;

    fn deref(&self) -> &State<F> {
        self.guard.as_ref().expect("the state is locked")
    }
}

impl<F: File> DerefMut for Locked<'_, F> {
    fn deref_mut(&mut self) -> &mut State<F> {
        self.guard.as_mut().expect("the state is locked")
    }
}

impl<F: File> Drop for Locked<'_, F> {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            drop(audited(guard));
        }
    }
}

/// fxdax: the most extents a branch may hold for the audit to recount its state ([`Locked`]): it
/// goes through them all each time the state is let go, which took a test of fxdax's, writing a
/// base of 32,768 extents one at a time and forking it a hundred times, past an hour; the stores
/// of the store's own tests hold hundreds at most (TASK-160.16).
#[cfg(any(test, feature = "audit"))]
const AUDITED_EXTENTS: usize = 4096;

/// fxdax: `guard`'s state, audited, in the builds of the tests ([`Locked`]), if its branch holds
/// [`AUDITED_EXTENTS`] at most. An audit that fails panics once it has let the state go,
/// unpoisoned, saying what it found after [`AUDIT_FAILED`]: the store's drop, closing its branch,
/// locks it again, which a lock poisoned would panic in, aborting as the audit's panic unwinds.
#[cfg(any(test, feature = "audit"))]
fn audited<F: File>(guard: MutexGuard<'_, State<F>>) -> MutexGuard<'_, State<F>> {
    if guard.failed.is_some() || std::thread::panicking() || guard.extents.len() > AUDITED_EXTENTS {
        return guard;
    }
    let audit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| guard.audit()));
    match audit {
        Ok(()) => guard,
        Err(panic) => {
            drop(guard);
            let found = (panic.downcast_ref::<String>().map(String::as_str))
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("a panic");
            panic!("{AUDIT_FAILED}: {found}")
        }
    }
}

/// fxdax: what a failed audit's panic begins with, whatever it found (TASK-160.16).
#[cfg(any(test, feature = "audit"))]
pub const AUDIT_FAILED: &str = "the extent store's audit found its counts drifted";

/// fxdax: `guard`, audited not, in the other builds.
#[cfg(not(any(test, feature = "audit")))]
fn audited<F: File>(guard: MutexGuard<'_, State<F>>) -> MutexGuard<'_, State<F>> {
    guard
}

/// fxdax: the audit of the state's counts, in the builds of the tests (TASK-160.16).
#[cfg(any(test, feature = "audit"))]
impl<F: File> State<F> {
    /// fxdax: the replay ([`Replay`]) applies `footer`, of a segment just closed; and, if the
    /// state holds nothing open nor changed since its last commit, which the footer names last,
    /// panics unless the state is what the replay says: its extents and their lengths, its root,
    /// what it spent and its capacity (TASK-165.9's AC #5).
    pub fn closed(&mut self, footer: &extent_frame::segment::Footer) {
        self.replay.apply_contents(footer.seq, footer.epoch, &footer.contents);
        if !self.uncommitted.is_empty() {
            return;
        }
        let lengths: BTreeMap<ExtentId, u64> =
            self.extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        assert_eq!(lengths, self.replay.extents.0, "the extents, as the footers say");
        let root = self.replay.root.as_deref().unwrap_or_default();
        assert_eq!(self.root, root, "the root, as the footers say");
        assert_eq!(self.spent, self.replay.spent, "the IDs spent, as the footers say");
        if let Some(capacity) = self.replay.capacity {
            assert_eq!(self.space.capacity(), capacity, "the capacity, as the footers say");
        }
    }

    /// Recounts each count the state keeps on as it changes from what it holds, and panics,
    /// saying which, unless they agree: the bytes its extents hold ([`Space`]); what writes since
    /// the last commit wrote over ([`Uncommitted`]); what each segment it tracks holds
    /// ([`Segments`]); what its extents' own files take ([`OwnFiles`]); and the frees it keeps
    /// ([`KeptExtents`]). Recounted from its extents, open and sealed, those it keeps, and the
    /// segments it tracks. Kept by hand, as each change adds to them or takes from them, a count
    /// drifting from what it counts showed only as a write refused room it had, or given room it
    /// had not (TASK-79.2, TASK-79.105). What writes in
    /// progress take, and what writes since the last commit let go of, which nothing it holds
    /// says, it checks only as far as what it holds bounds them.
    pub fn audit(&self) {
        let branch = self.segments.open_segment().branch;
        let held: Vec<_> = self.extents.iter().collect();
        self.space.audit(branch, &held, self.gate.is_writing());
        self.uncommitted.audit(&self.extents);
        self.kept.audit();
        let own =
            |extent: &ExtentId, entry: &Extent| extent.branch == branch && entry.data == Data::Own;
        let mut recount = Recount::default();
        let mut own_files = HashSet::new();
        for &(extent, entry) in &held {
            let sealed = !self.uncommitted.is_open(*extent);
            recount.holds(*extent, entry, sealed);
            if sealed && own(extent, entry) {
                own_files.insert(*extent);
            }
        }
        for (extent, kept) in self.kept.iter() {
            recount.keeps(&kept.entry);
            if own(extent, &kept.entry) {
                own_files.insert(*extent);
            }
        }
        self.segments.audit(&recount);
        self.own_files.audit(&own_files);
    }
}

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
/// that no extent is in, freed, takes none of the room, however much: the segments leave the
/// store once acknowledged in the bucket, whose collector takes it back (TASK-165.8). Counted as
/// used past the capacity's worth, it found a guest's store full with a tenth of it used
/// (`ENOSPC`), which the guest's filesystem did not survive.
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

    /// Fails with [`NoSpace`], saying the numbers (TASK-79.105), unless a write may add `grows`
    /// bytes, views keeping `kept_for_views` of the frees, and writes since the last commit, this
    /// one's included, wasting `wasted` bytes.
    pub fn check(&self, grows: u64, kept_for_views: u64, wasted: u64) -> anyhow::Result<()> {
        let used = self.stored + kept_for_views + self.past_slack(wasted);
        if used + self.reserved + grows > self.capacity {
            return Err(anyhow::Error::new(NoSpace).context(format!(
                "{used} bytes used, {} reserved and {grows} written past the capacity's {}",
                self.reserved, self.capacity
            )));
        }
        Ok(())
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

    /// fxdax: panics unless it counts what `held`, the extents of `branch`, hold, of them its own;
    /// and, unless `writing`, writes in progress reserving none (TASK-160.16).
    #[cfg(any(test, feature = "audit"))]
    fn audit(&self, branch: u64, held: &[(&ExtentId, &Extent)], writing: bool) {
        let stored: u64 = held.iter().map(|(_, entry)| entry.len).sum();
        let own: u64 = (held.iter())
            .filter(|(extent, _)| extent.branch == branch)
            .map(|(_, entry)| entry.len)
            .sum();
        assert_eq!((self.stored, self.own), (stored, own), "the bytes the extents hold, own");
        assert!(writing || self.reserved == 0, "{} bytes reserved, no write in progress", {
            self.reserved
        });
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
    /// fxdax: the capacity set since the last commit, which the next sets in the branch's state
    /// as object storage keeps it, its segment's footer saying so (TASK-165.3).
    capacity: Option<u64>,
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
        let (open, changes, overwritten, let_go) = (HashMap::new(), Vec::new(), 0, 0);
        Self { open, openings: 0, changes, overwritten, let_go, capacity: None }
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

    /// fxdax: whether nothing changed since the last commit: no extent open, no free.
    #[cfg(any(test, feature = "audit"))]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.changes.is_empty() && self.capacity.is_none()
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

    /// fxdax: `bytes` that writes since the last commit wrote are no extent's any more: of an
    /// extent freed while open, or of a write dropped (TASK-79.2).
    pub fn let_go(&mut self, bytes: u64) {
        self.let_go += bytes;
    }

    /// fxdax: the bytes writes since the last commit wasted: what the open extents' writes wrote
    /// over, and what those freed, and writes dropped, let go of (TASK-79.2). The next commit takes
    /// them back: of segments set aside, which it removes, or as dead space, which leaves with the
    /// segments once acknowledged in the bucket (TASK-165.8); and of raw files, which its flush
    /// removes.
    pub fn wasted(&self) -> u64 {
        self.overwritten + self.let_go
    }

    /// `extent`, freed, is open no more: where its data went, if it was. fxdax: what its writes
    /// wrote over is let go of with the rest of what they wrote, which its free lets go of; and its
    /// raw file, if it has one, which the next commit's flush removes.
    pub fn remove(&mut self, extent: ExtentId) -> Option<Placement<F>> {
        let open = self.open.remove(&extent)?;
        self.overwritten -= open.overwritten;
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

    /// fxdax: the branch's capacity is `capacity`, as of the next commit.
    pub fn resize(&mut self, capacity: u64) {
        self.capacity = Some(capacity);
    }

    /// fxdax: the capacity set since the last commit, if any, for the commit, which sets it.
    pub fn take_capacity(&mut self) -> Option<u64> {
        self.capacity.take()
    }

    /// The changes since the last commit, for the commit, which seals the extents open.
    pub fn take(&mut self) -> Vec<Change> {
        self.open.clear();
        self.overwritten = 0;
        self.let_go = 0;
        std::mem::take(&mut self.changes)
    }

    /// fxdax: panics unless what it counts of the open extents, which `extents` holds as the
    /// writes that finished left them, is theirs (TASK-160.16): what their writes wrote over, in
    /// segments, the bytes of their pieces past what the extent holds of them. What writes since
    /// the last commit let go of, which nothing it holds says, it can't recount.
    #[cfg(any(test, feature = "audit"))]
    fn audit(&self, extents: &Extents) {
        let overwritten: u64 = self.open.values().map(|open| open.overwritten).sum();
        assert_eq!(self.overwritten, overwritten, "written over");
        for (extent, open) in &self.open {
            assert!(open.opening <= self.openings, "{extent:?} opened past the openings");
            let data = extents.get(extent).map(|entry| (entry.len, &entry.data));
            match (&open.placement, data) {
                (Placement::Segment, Some((len, Data::Pieces(pieces)))) => {
                    let written: u64 = pieces.iter().map(|piece| piece.len).sum();
                    let held = covered(pieces, 0, len);
                    assert_eq!(open.overwritten + held, written, "{extent:?}'s writes, open");
                }
                (Placement::Own(_), Some((_, Data::Own)))
                | (Placement::Segment | Placement::Own(_), None) => {
                    assert_eq!(open.overwritten, 0, "{extent:?}, in a raw file, wrote over");
                }
                (_, data) => panic!("{extent:?}, open, holds {data:?} where it is not placed"),
            }
        }
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
        let (dir, open, unsynced) =
            (branch_dir(EXTENTS, branch), Files::new(limit), BTreeMap::new());
        Self { open, sizes, unsynced, dir, written: false }
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

    /// fxdax: panics unless they are the files of `sealed`, the branch's own extents sealed in
    /// files of their own, held or kept (TASK-160.16).
    #[cfg(any(test, feature = "audit"))]
    pub fn audit(&self, sealed: &HashSet<ExtentId>) {
        let files: HashSet<ExtentId> = self.sizes.keys().copied().collect();
        assert_eq!(&files, sealed, "the files of the extents sealed on their own");
    }
}

/// What a branch's next commit makes unreferenced: files of the branch's, which it removes once it
/// is durable.
#[derive(Default)]
pub(crate) struct Unreferenced {
    /// Raw files whose extents were sealed from them, or freed, and files of extents' data no
    /// extent is in any more: they wait for the branch's commits, in its queue of them, and are
    /// forgotten with a failed store's, as its next opening removes what nothing refers to.
    pub doomed: Vec<String>,
}

impl Unreferenced {
    /// Those of the commit: from now on, the next's.
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}
