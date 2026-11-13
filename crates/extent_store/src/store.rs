// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store. See the crate's docs.

use crate::dir::{Dir, File, not_found, read_file};
use crate::durability::{Batch, Durability, FileSyncs, Syncs};
use crate::framed::{Aligned, Framed, Packed, Part, Source, Unpacked, read_aligned};
use crate::kept::{Kept, KeptExtents};
use crate::layout::{
    Change, DESCRIPTION, Data, Description, EXTENTS, Extent, Extents, FileKey, KINDS, LOGS,
    OPEN_EXTENTS, OPEN_SEGMENTS, Piece, Place, SEGMENTS, SegmentId, StoreFile, Volume, add_piece,
    branch_dir, create_branch_dirs, encode_salted, extent_path, files_of, list_numbers, log_path,
    open_extent_path, open_segment_path, segment_path, spare_path,
};
use crate::seals;
use crate::segments::{self, Behind, Preparation, Preparing, Segments, Unframed, footer_last};
use crate::shared::{Shared, SharedLock};
use crate::state::{
    Framing, Gate, Life, Locked, OwnFiles, Placement, Readable, Reader, Space, Standing, State,
    Uncommitted, Unreferenced, Writable,
};
use crate::sync::{Condvar, Mutex, MutexGuard, rules, spawn_or_return, thread};
use crate::timings::{self, Flush, Waits, flush};
use crate::uploads::Closed;
use crate::views::{Limits, Sealed, View, Views};
use crate::wal::{
    BaseFile, LOG_HEADER, LogRecord, Queue, Queued, Recovered, Runs, SegmentLog, log_salt,
};
use crate::written;
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Branch, Chain, Child, Contents, Footer, Location, epoch_of};
use extent_remote::cache::Cache;
use extent_remote::recovery::{Places, Seal};
use extent_remote::trace;
use extent_remote::wal::{Fenced, Rebased, Wal};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_device::{ExtentId, OtherBranch, SealedExtent};

/// The error of a write the store has no room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the extent store is full")]
pub struct NoSpace;

/// fxdax: the context of a fork's failure once its commit was made ([`Store::commit_fork`]): the
/// commit stands, the segment it ends naming the fork in its footer, which what keeps the branch's
/// segments elsewhere, as an uploader does, may have taken, and whose fork it makes there, so that
/// the child is to be deleted there as a fork made is (TASK-79.82).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the fork to branch {child} failed once its commit was made")]
pub struct ForkCommitted {
    /// The fork's child.
    pub child: Branch,
}

/// fxdax: the error of a fork whose child's ID is the branch's, or that of a branch whose extents
/// the branch holds: its child's extents' IDs would be theirs (TASK-364).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the fork's child {0} has the ID of its parent's or of a branch it inherits from")]
pub struct Inherited(pub Branch);

/// The error of a capacity smaller than what the store holds: its extents' bytes, and those that
/// writes in progress may add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the extent store holds {held} bytes, more than {capacity}")]
pub struct TooSmall {
    pub capacity: u64,
    pub held: u64,
}

/// fxdax: the error of a write, or an opening, of an extent of the store's branch that the branch
/// sealed and freed (TASK-79.6): an extent's ID is its branch's to write once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} was written before: an extent's ID is written once")]
pub struct Spent(pub ExtentId);

/// fxdax: the error of a read of an extent the store holds whose data is in the bucket, at
/// `location` ([`Store::adopt`]): what the store's user reads it from.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{extent:?}'s data is in the bucket, at {location:?}")]
pub struct RemoteData {
    pub extent: ExtentId,
    pub location: Location,
}

/// The error of I/O at an offset, or of a length, not a multiple of the block size, or empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unaligned I/O: {len} bytes at {offset}")]
pub struct Unaligned {
    pub offset: u64,
    pub len: u64,
}

/// The error of I/O whose end is past the most bytes an extent holds (`Options::max_extent`), or
/// past the largest offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{len} bytes at {offset}: past the most an extent holds")]
pub struct TooLong {
    pub offset: u64,
    pub len: u64,
}

/// How a store lays its extents out.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The most bytes an extent holds, doc-4's `max_extent`: no write or read ends past it. A
    /// sealed extent is read, framed and uploaded whole, so that this bounds what each takes of
    /// memory, whatever its writer asks (TASK-79.2).
    pub max_extent: u64,
    /// The size past which a segment takes no more extents: the next commit closes it, and starts
    /// another. fxdax: between commits it takes writes up to `max_extent` past it, then they go to
    /// another, so that no segment holds more than both together (TASK-79.2).
    pub segment_size: u64,
    /// The age past which a segment holding extents' data takes no more: the next commit closes it.
    pub segment_age: Duration,
    /// The size from which an extent is stored in a file of its own: one sealed at that length or
    /// more.
    pub large_extent: u64,
    /// How many segments' files the store keeps open between reads.
    pub open_files: usize,
    /// fxdax: how long a view ([`Store::view`]) may lag behind the commits after its own before
    /// it is cut off, if it may not lag for ever.
    pub view_lag: Option<Duration>,
    /// fxdax: how many bytes of frees views may keep, past which the oldest is cut off, if any
    /// limit.
    pub view_kept_bytes: Option<u64>,
    /// fxdax: the most bytes of runs a flush's commits seal for their records to hold them, and
    /// the flush to sync its logs alone (`wal::LogRecord`, TASK-301): what a guest's journal, or
    /// a database's transaction, commits at a time, not a bulk load's.
    pub held_most: u64,
}

impl Options {
    /// fxdax: the most bytes of data a segment holds: `segment_size`, and a write more, of
    /// `max_extent` at most, before writes go to another. A closed segment is read, framed and
    /// uploaded whole, so that this bounds what each takes of memory, however much is written
    /// between commits (TASK-79.2).
    pub fn segment_limit(&self) -> u64 {
        self.segment_size.saturating_add(self.max_extent)
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_extent: 64 << 20,
            segment_size: 32 << 20,
            segment_age: Duration::from_secs(60),
            large_extent: 4 << 20,
            open_files: 64,
            view_lag: None,
            view_kept_bytes: None,
            held_most: 256 << 10,
        }
    }
}

/// What a store holds, as doc-4's `INFO` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// The most bytes the store's extents may hold.
    pub capacity: u64,
    /// The bytes its extents hold: each as long as its furthest write.
    pub used: u64,
    /// The bytes it can still take.
    pub available: u64,
    /// One more than the highest sequence number of the store's branch's extents, freed or not
    /// since it opened; and, as it opened, of those it held. fxdax: those freed before, the
    /// writer's floor is past, each opening's writer minting past its own (doc-7, Fencing).
    pub next_sequence: u64,
    /// fxdax: of the bytes its extents hold, those of the extents its branch wrote, rather than
    /// inherited from the branches it was forked from, which may share them still (TASK-49).
    pub own: u64,
    /// fxdax: those of the extents it inherited from the branches it was forked from.
    pub inherited: u64,
}

/// The space a store's files take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// The segments, and how many there are.
    pub segments: u64,
    pub segment_bytes: u64,
    /// The bytes of extents' data in segments.
    pub live_bytes: u64,
    /// The bytes of the extents stored on their own.
    pub own_bytes: u64,
    /// The bytes the branch's segments' files and the extents' own files take: closed segments and
    /// sealed extents compressed (`extent_frame`).
    pub disk_bytes: u64,
    /// fxdax: the bytes of extents freed whose data stays, for the uploader or views.
    pub kept_bytes: u64,
    /// fxdax: those of them views keep, which count as used ([`Info::used`]).
    pub kept_for_views: u64,
}

/// fxdax: a store's directory, taken and claimed for its opening, until it closes.
type Opened<D> = Option<(Claim<D>, Arc<Taken<D>>)>;

/// An extent store, open. See the crate's docs.
pub struct Store<D: Dir> {
    /// Its directory, which its framings share.
    dir: Arc<D>,
    block_size: u32,
    branch: Branch,
    /// fxdax: the IDs of the branches whose extents it held as it opened, its own among them: no
    /// fork's child takes one, as an extent's ID names its branch by its ID alone, and branches
    /// are unique by theirs only within their cells (TASK-364).
    lineage: BTreeSet<u64>,
    /// fxdax: the volume of its branch's cell it is, if its description names one (TASK-364).
    volume: Option<Volume>,
    /// fxdax: the URL of the bucket its branch is kept in, if its description names one.
    bucket: Option<String>,
    /// Its directory, taken for this process ([`Dir::take`]), and claimed for its opening
    /// ([`Dir::claim`]): closing it lets go of both, but of the taking what holds it still
    /// ([`Store::hold_dir`]).
    opened: SharedLock<Mutex<Opened<D>>>,
    /// The parts of its files read lately, unpacked.
    unpacked: Unpacked,
    options: Options,
    /// fxdax: shared with the framings of its segments, each of which ends itself in it, on a
    /// thread of its own (TASK-153.8).
    state: Arc<SharedLock<Mutex<State<D::File>>>>,
    /// Held by a commit.
    committing: SharedLock<Mutex<()>>,
    /// When its commits are durable: those ordered that no flush has taken yet, oldest first, for
    /// [`Store::flush`] to make durable, and the last that is.
    durability: SharedLock<Mutex<Durability<Pending<D::File>>>>,
    /// Held by a flush: commits become durable in order. fxdax: and the logs flushes append
    /// records to, of the open segment, and of those closed since whose commits wait (TASK-165.9).
    flushing: SharedLock<Mutex<Logs<D::File>>>,
    /// fxdax: the base of its write-ahead log, written as segments are acknowledged, before their
    /// files leave (TASK-165.9).
    base: SharedLock<Mutex<BaseFile>>,
    /// fxdax: the host's cache of the bucket's objects, if it has one: what the bucket acknowledged
    /// of the store's goes into it (TASK-165.8).
    cache: Option<Arc<Cache>>,
    /// Notified when the open segment has no write in progress, and when it closed: what
    /// [`Store::close_now`] and the writes it holds up wait for; fxdax: and as each framing of a
    /// segment ends, which flushes, the uploader, forks and the branch's closing wait for.
    idle: Arc<Condvar>,
    /// fxdax: its views, which followers read it through ([`Store::view`]).
    views: Arc<Views>,
}

/// A store's directory, taken for this process ([`Dir::take`]): let go of once dropped.
struct Taken<D: Dir>(Arc<D>);

impl<D: Dir> Drop for Taken<D> {
    fn drop(&mut self) {
        self.0.let_go();
    }
}

/// fxdax: a store's directory, claimed for its opening in this process ([`Dir::claim`]): let go of
/// once dropped.
struct Claim<D: Dir>(Arc<D>);

impl<D: Dir> Drop for Claim<D> {
    fn drop(&mut self) {
        self.0.unclaim();
    }
}

/// fxdax: a hold of a store's directory, taken for this process until it is dropped
/// ([`Store::hold_dir`]).
pub struct DirHold<D: Dir> {
    _taken: Arc<Taken<D>>,
}

/// What a commit ordered but not yet durable leaves for the flush that makes it durable.
struct Pending<F> {
    /// The files whose data it refers to, and the directories whose entries changed, to sync.
    syncs: Syncs<F>,
    /// fxdax: its contents, and the segment whose log its record is to be appended to, the one it
    /// was made in, open, or closed since (TASK-165.9): recorded by its flush, with the runs it
    /// sealed checksummed ([`LogRecord`], TASK-301). fxdax: one for each segment it spans, in
    /// their order: what it goes on with past each filled since the last commit, in that one's
    /// log, then the rest, in the open segment's, where it ends (TASK-92.41).
    records: Vec<(SegmentId, Contents)>,
    /// The files of the branch's alone it made unreferenced ([`Unreferenced::doomed`]): they go
    /// once it is durable.
    doomed: Vec<String>,
    /// fxdax: the files of the segments acknowledged in the bucket it let go of, with their keys
    /// there if they are the objects there: they leave for the host's cache with its flush
    /// (TASK-165.8).
    leaving: Vec<(String, Option<String>)>,
    /// fxdax: the segments opened whose raw files' and logs' entries its syncs make durable
    /// (`Segments::take_syncs`, TASK-369).
    entries: BTreeSet<SegmentId>,
}

/// fxdax: the logs a flush appends commits' records to, by their segments ([`Store::flush`]).
/// fxdax: what flushes keep between them: the logs they append commits' records to, by their
/// segments, and the raw files a flush holding its commits' runs left unsynced, which the next
/// flush syncing data syncs too (TASK-301): a commit that seals nothing, as one setting the
/// capacity, takes a raw file's sync, which a later commit's data written to it needs.
struct Logs<F> {
    by_segment: BTreeMap<SegmentId, SegmentLog<F>>,
    unsynced: Syncs<F>,
}

/// A write in progress (see [`Store::begin_write`]): its data goes to `file()` at `offset()`.
/// Dropping it before it is finished abandons it.
pub struct Write<'a, D: Dir> {
    store: &'a Store<D>,
    extent: ExtentId,
    /// The extent's opening it writes to ([`Uncommitted::open`]): one its free ends drops it.
    opening: u64,
    extent_offset: u64,
    len: u64,
    file: Arc<D::File>,
    offset: u64,
    /// The segment written to, if it isn't the extent's own file.
    segment: Option<SegmentId>,
    /// The bytes it reserved, which it may add to the store.
    grows: u64,
    finished: bool,
}

impl<D: Dir> Write<'_, D> {
    /// The file the data goes to.
    pub fn file(&self) -> &Arc<D::File> {
        &self.file
    }

    /// Where in the file the data goes.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Records the write, whose data is in the file: reads see it, and the next commit includes
    /// it.
    pub fn finish(mut self) {
        self.finished = true;
        // fxdax: counted (TASK-310).
        written::wrote(written::Kind::Raw, self.len as usize);
        // fxdax: what was written to the open segment, its writing back begun once a MiB or more
        // waits, and waited for once more than WRITTEN_BEHIND_MOST is begun: holding no lock
        // (TASK-221). Failing, it is the flush's sync that fails.
        let Some(Behind { raw, begin: (offset, len), wait }) = self.store.finish_write(&self)
        else {
            return;
        };
        if let Err(err) = raw.write_behind(offset, len) {
            log::warn!("extent store: writing back {len} bytes at {offset} of a segment: {err}");
        }
        if let Some((offset, len)) = wait
            && let Err(err) = raw.write_back(offset, len)
        {
            log::warn!("extent store: waiting for {len} bytes at {offset} of a segment: {err}");
        }
    }
}

impl<D: Dir> Drop for Write<'_, D> {
    fn drop(&mut self) {
        if !self.finished {
            self.store.abandon_write(self);
        }
    }
}

/// Where `entry`'s data is.
fn place_of(entry: &Extent) -> Place {
    match &entry.data {
        Data::Pieces(pieces) => match pieces[..] {
            [piece] if piece.extent_offset == 0 && piece.len == entry.len => Place::Run {
                branch: piece.segment.branch,
                number: piece.segment.number,
                offset: piece.offset,
            },
            _ => Place::Pieces,
        },
        Data::Own => Place::Own,
        Data::Remote(location) => Place::Remote(location.clone()),
    }
}

/// fxdax: ends the framing of segment `number`, `framed` saying how ([`Store::frame_behind`]), in
/// `state`: the segment is read from its framed file from now on, durable, its raw file going once
/// the commits its footer names are durable; or its framing failed, writing the framed file or
/// making it durable, and the store with it, as it is ahead of its files.
fn framing_ended<F: File>(
    state: &mut State<F>,
    number: SegmentId,
    framed: Result<Arc<Framed<F>>, &Error>,
) {
    match framed {
        Ok(framed) => state.segments.framed(number, framed),
        Err(error) => {
            log::error!("extent store: framing segment {number:?}: {error:#}");
            let error = anyhow!("framing segment {number:?}: {error:#}");
            state.fail(&error);
            state.segments.framing_failed(number, format!("{error:#}"));
        }
    }
}

/// fxdax: what a store's segments are framed with, behind what closed them
/// ([`Store::frame_behind`]): its directory, its state, which each framing ends itself in, and
/// what is notified as one does. A framer frames the segments closed, one after another, in the
/// order they closed, each written as it goes (`Unframed::frame`), and hands it to be made durable;
/// another thread makes those framed meanwhile durable together (`segments::make_durable`), and
/// ends their framings: so that no framing waits for another's syncs, which on a disk VMs share
/// took seconds each (TASK-310). Each on a thread of its own, started as there is work for it, and
/// gone once there is none.
struct Framer<D: Dir> {
    dir: Arc<D>,
    branch: u64,
    state: Arc<SharedLock<Mutex<State<D::File>>>>,
    idle: Arc<Condvar>,
}

impl<D: Dir> Clone for Framer<D> {
    fn clone(&self) -> Self {
        let (dir, state, idle) = (self.dir.clone(), self.state.clone(), self.idle.clone());
        Self { dir, branch: self.branch, state, idle }
    }
}

impl<D: Dir> Framer<D> {
    fn lock(&self) -> Locked<'_, D::File> {
        Locked::new(self.state.with(Mutex::lock))
    }

    /// The framer's thread: frames the segments queued, one after another, until none is.
    fn frame_queued(&self) {
        let mut state = self.lock();
        while let Some(unframed) = state.framing.queue.pop_front() {
            // fxdax: none once the store failed, a segment before it among what failed, saying so
            // (TASK-165.9).
            let whole = state.standing.whole();
            let number = unframed.number();
            drop(state);
            let framed = whole.and_then(|()| unframed.frame(&*self.dir));
            state = self.lock();
            if self.wrote(&mut state, number, framed) {
                // The host refused a thread to make it durable: on this one, as it would.
                drop(state);
                self.make_written_durable();
                state = self.lock();
            }
        }
        state.framing.running = false;
    }

    /// Segment `number`'s framing wrote its framed file, `framed`, in `state`: to be made durable
    /// with those framed meanwhile, on a thread that this starts if none runs. Or its writing
    /// failed, which ends its framing. Returns whether the host refused the thread, which the
    /// caller is to be then.
    fn wrote(
        &self,
        state: &mut State<D::File>,
        number: SegmentId,
        framed: Result<Arc<Framed<D::File>>, Error>,
    ) -> bool {
        let framed = match framed {
            Ok(framed) => framed,
            Err(error) => {
                framing_ended(state, number, Err(&error));
                self.idle.notify_all();
                return false;
            }
        };
        state.segments.written(number, framed.clone());
        state.framing.wrote(number, framed);
        // fxdax: the uploader, which takes it now (TASK-384).
        self.idle.notify_all();
        if std::mem::replace(&mut state.framing.syncing, true) {
            return false;
        }
        let syncer = self.clone();
        spawn_or_return("store durable", (), move |()| syncer.make_written_durable()).is_some()
    }

    /// The thread making the segments framed durable: those written since it last did, together,
    /// until none is left, each of whose framings it ends.
    fn make_written_durable(&self) {
        let mut state = self.lock();
        while !state.framing.written.is_empty() {
            let (written, since) = state.framing.take_written();
            let entries =
                state.segments.unsynced_entries(written.iter().map(|(number, _)| *number));
            let whole = state.standing.whole();
            drop(state);
            let durable = whole.and_then(|()| self.make_durable(&written, &entries, since));
            state = self.lock();
            Self::made_durable(&mut state, written, &entries, &durable);
            self.idle.notify_all();
        }
        state.framing.syncing = false;
        self.idle.notify_all();
    }

    /// On the caller's thread, holding `state`, as before TASK-41, if the directory's I/O is its
    /// callers' alone, or the host refuses a thread to frame on: frames the segments queued, then
    /// makes them durable together, unless a thread does already, which takes them. No framer runs
    /// meanwhile.
    fn frame_here(&self, state: &mut State<D::File>) {
        while let Some(unframed) = state.framing.queue.pop_front() {
            let number = unframed.number();
            match state.standing.whole().and_then(|()| unframed.frame(&*self.dir)) {
                Ok(framed) => {
                    state.segments.written(number, framed.clone());
                    state.framing.wrote(number, framed);
                }
                Err(error) => framing_ended(state, number, Err(&error)),
            }
        }
        state.framing.running = false;
        if !state.framing.syncing && !state.framing.written.is_empty() {
            let (written, since) = state.framing.take_written();
            let entries =
                state.segments.unsynced_entries(written.iter().map(|(number, _)| *number));
            let whole = state.standing.whole();
            let durable = whole.and_then(|()| self.make_durable(&written, &entries, since));
            Self::made_durable(state, written, &entries, &durable);
        }
        self.idle.notify_all();
    }

    /// Makes `written`, segments framed, the first of them written at `since`, durable together;
    /// and the entries of `entries`, segments opened, too.
    fn make_durable(
        &self,
        written: &[(SegmentId, Arc<Framed<D::File>>)],
        entries: &BTreeSet<SegmentId>,
        since: Instant,
    ) -> Result<(), Error> {
        let entries = !entries.is_empty();
        segments::make_durable(&*self.dir, self.branch, (written, entries), since)
    }

    /// `written`, segments framed, were made durable together, and the entries of `entries`, or
    /// not, as `durable` says: their framings end.
    fn made_durable(
        state: &mut State<D::File>,
        written: Vec<(SegmentId, Arc<Framed<D::File>>)>,
        entries: &BTreeSet<SegmentId>,
        durable: &Result<(), Error>,
    ) {
        if durable.is_ok() {
            state.segments.entries_synced(entries);
        }
        for (number, framed) in written {
            framing_ended(state, number, durable.as_ref().map(|()| framed));
        }
    }
}

/// fxdax: makes the entries of `branch`'s raw files and logs durable in `dir`, those of the next
/// segment's, placed ahead of it, among them, off its commits (TASK-384): returns whether it did,
/// a flush syncing them otherwise, as it syncs those of a segment opened unplaced.
fn sync_entries<D: Dir>(dir: &D, branch: u64) -> bool {
    let mut syncs = Syncs::default();
    for kind in [OPEN_SEGMENTS, LOGS] {
        syncs.dir(branch_dir(kind, branch));
    }
    match syncs.sync_dirs(dir).and_then(|()| dir.barrier().map_err(Error::from)) {
        Ok(()) => true,
        Err(err) => {
            log::warn!("extent store: making the next segment's entries durable: {err:#}");
            false
        }
    }
}

/// fxdax: removes the spare log `spare` names, if any, one the branch keeps no more
/// (`Segments::prepared`, TASK-333).
fn remove_spare<D: Dir>(dir: &D, spare: Option<SegmentId>) {
    let Some(spare) = spare else { return };
    let path = spare_path(spare);
    if let Err(err) = dir.remove(&path)
        && !not_found(&err)
    {
        log::warn!("extent store: failed to remove {path}: {err}");
    }
}

/// fxdax: the file a store's directory holds until its maker made it whole, which opening it
/// refuses ([`Unmade`]): written before its description, removed once whole ([`Store::made`]), as
/// an import fills it, so that a maker dying part way leaves no store that opens (TASK-196).
pub(crate) const UNMADE: &str = "unmade";

/// fxdax: the error of opening a store whose maker never made it whole ([`UNMADE`], TASK-196).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the store was never made whole: its maker stopped part way")]
pub struct Unmade;

/// fxdax: what the bucket holds of the branch a store opens (doc-14, Opening a store;
/// TASK-165.9): its state, as recovery found it once its writer held its lease and fenced it,
/// where the writer found its end ([`Fenced`]), which the store's write-ahead log is put after,
/// and its writer's chain there, which stamps the store's footers from then on; or, for a point,
/// its state alone, which nothing writes. And the host's cache of the bucket's objects, if it has
/// one, which what the bucket acknowledged of the store's goes into.
pub struct Opening {
    pub state: extent_remote::State,
    pub fenced: Option<Fenced>,
    pub cache: Option<Arc<Cache>>,
}

impl Opening {
    /// The opening of `branch`, which the bucket holds nothing of: no commit of it there, and a
    /// writer's chain from its first segment, of lease epoch 0. Its store's write-ahead log
    /// follows only if it never had anything acknowledged ([`extent_remote::Error::Diverged`]):
    /// a store of the store's own tests, of a bucket it never reaches.
    pub fn new(branch: Branch) -> Self {
        let chain = Chain::new(branch, 0, 0, None, 0);
        let fenced = Fenced { recovered: None, last_commit: None, chain };
        Self { state: extent_remote::State::new(branch), fenced: Some(fenced), cache: None }
    }
}

/// fxdax: a store's directory, taken for this process and claimed for its opening, its write-ahead
/// log recovered from it ([`Store::recover`]), to be opened as what the bucket holds of its branch
/// says ([`Recovering::open`]): its branch's writer takes the lease meanwhile, and recovers it.
pub struct Recovering<D: Dir> {
    dir: Arc<D>,
    taken: Arc<Taken<D>>,
    claim: Claim<D>,
    file: StoreFile,
    wal: Recovered,
}

impl<D: Dir> Recovering<D> {
    /// The branch.
    pub fn branch(&self) -> Branch {
        self.file.branch
    }

    /// The URL of the bucket its branch is kept in, if its description names one.
    pub fn bucket(&self) -> Option<&str> {
        self.file.bucket.as_deref()
    }

    /// The fencing token of its placement, if it was placed (TASK-39.7).
    pub fn token(&self) -> Option<u64> {
        self.file.token
    }

    /// fxdax: the volume of its branch's cell it is, if its description names one (TASK-364).
    pub fn volume(&self) -> Option<&Volume> {
        self.file.volume.as_ref()
    }

    /// The branch's commit its write-ahead log goes on from: the last the bucket acknowledged of
    /// its commits, if it acknowledged any.
    pub fn base(&self) -> Option<u64> {
        self.wal.base
    }

    /// fxdax: whether `commit`, a branch's last, is its write-ahead log's base or one of its
    /// commits: a branch whose last commit is never moved on from the log, which then diverged
    /// not ([`extent_remote::Error::Diverged`], TASK-345).
    pub fn holds_commit(&self, commit: Option<u64>) -> bool {
        let wal = self.wal();
        let mut commits = wal.segments().flat_map(|(footer, _)| &footer.contents.commits);
        commit == wal.base() || commit.is_some_and(|commit| commits.any(|&own| own == commit))
    }

    /// The write-ahead log, as its directory holds it: stamped as it was.
    fn wal(&self) -> Wal<u64> {
        Wal::of(self.wal.base, self.wal.segments.iter().cloned())
    }

    /// Whether its write-ahead log holds nothing.
    pub fn is_empty(&self) -> bool {
        self.wal.segments.is_empty()
    }

    /// Whether its write-ahead log follows the branch whose end a writer found as `fenced` says:
    /// unless the branch moved on from it ([`extent_remote::Error::Diverged`]). What a store asks
    /// of the branch as recovery found it before taking its lease, so that a store plainly stale
    /// fences no writer.
    pub fn follows(&self, fenced: &Fenced) -> bool {
        self.wal().rebase(fenced).is_ok()
    }

    /// Drops its write-ahead log, the commits it holds never acknowledged lost: what `fxdaxctl
    /// take` does of a store whose log diverged (doc-14, Opening a store). Its base goes with it:
    /// an empty log follows any branch. Durably, before the store opens past it: a crash after
    /// brings none of it back, whose commits the branch moved on from, to upload after the store's
    /// own since (TASK-295).
    pub fn drop_wal(&mut self) -> Result<(), Error> {
        let dir = &*self.dir;
        let branch = self.file.branch;
        for (footer, number) in std::mem::take(&mut self.wal.segments) {
            let segment = SegmentId { branch: branch.id, number };
            self.remove_files([segment_path(segment)], &footer);
        }
        for kind in [SEGMENTS, EXTENTS] {
            dir.sync_dir(&branch_dir(kind, branch.id))?;
        }
        self.wal.base_file.write(dir, None)?;
        dir.barrier()?;
        self.wal.base = None;
        Ok(())
    }

    /// Removes the files at `paths`, and those of the extents stored on their own `footer` names.
    fn remove_files(&self, paths: impl IntoIterator<Item = String>, footer: &Footer) {
        let own = footer.own().map(|(extent, _)| extent_path(extent));
        for path in paths.into_iter().chain(own) {
            if let Err(err) = self.dir.remove(&path)
                && !not_found(&err)
            {
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
    }

    /// Opens the store as `opening` says the bucket holds its branch (doc-14, Opening a store):
    /// its write-ahead log put after the branch's end, by its commits, those of its segments that
    /// landed acknowledged, their files into the host's cache, and the rest stamped anew on the
    /// writer's chain ([`Wal::rebase`]), its base written first; or refused, if the branch moved
    /// on from it ([`extent_remote::Error::Diverged`]). The branch's state is the bucket's, its
    /// extents there, with the log's segments' footers applied over it, their extents here; and
    /// another segment opens, past them. A point's store holds its state alone, and no log.
    pub fn open(self, options: Options, opening: Opening) -> Result<Store<D>, Error> {
        let Self { dir, taken, claim, file, wal: recovered } = self;
        let StoreFile { block_size, branch, bucket, capacity, volume, .. } = file;
        let Opening { state, fenced, cache } = opening;
        ensure!(state.branch == branch, "branch {branch} opened as {}", state.branch);
        // fxdax: what the bucket acknowledged goes into the host's cache by a rename, which can't
        // cross filesystems: a store elsewhere than the cache is refused, not left to read every
        // segment back from the bucket (TASK-359).
        if let (Some(cache), Some(here)) = (&cache, dir.host_path("")) {
            use std::os::unix::fs::MetadataExt as _;
            let device = |path: &std::path::Path| {
                std::fs::metadata(path)
                    .map(|metadata| metadata.dev())
                    .with_context(|| format!("{}", path.display()))
            };
            ensure!(
                device(&here)? == device(cache.dir())?,
                "the store at {} is on another filesystem than the host's cache at {}: the \
                 store's segments go into the cache by renaming them",
                here.display(),
                cache.dir().display()
            );
        }
        // Stamped as the directory holds them: those the rebase stamps anew upload so.
        let stamped: BTreeMap<u64, Footer> =
            recovered.segments.iter().map(|(footer, number)| (*number, footer.clone())).collect();
        let mut wal = Wal::of(recovered.base, recovered.segments);
        let mut base_file = recovered.base_file;
        let (Rebased { landed, ended }, chain) = match &fenced {
            Some(fenced) => {
                let rebased = wal.rebase(fenced)?;
                base_file.write(&*dir, wal.base())?;
                // Its footers from now on follow those stamped anew.
                let mut chain = fenced.chain;
                for (footer, _) in wal.segments() {
                    chain = chain.admit(footer).expect("a log rebased follows the writer's chain");
                }
                (rebased, chain)
            }
            None => {
                ensure!(wal.is_empty(), "the store of point {branch} holds commits of its own");
                (Rebased { landed: Vec::new(), ended: None }, Chain::new(branch, 0, 0, None, 0))
            }
        };
        // What landed is the bucket's: into the host's cache, as an acknowledgement's files go.
        // fxdax: and the files of the extents stored on their own of the commits a segment ending
        // the branch ends, which goes on, stamped again with what it goes on with alone; the
        // extents sealed in it, those commits', the bucket's state holds there (TASK-345).
        let mut leaving = Vec::new();
        for (footer, number) in landed {
            let segment = SegmentId { branch: branch.id, number };
            let key = extent_remote::layout::segment(branch, footer.seq);
            leaving.push((segment_path(segment), Some(key)));
            leaving.extend(footer.own().map(|(extent, at)| (extent_path(extent), Some(at.key))));
        }
        if let Some(footer) = &ended {
            leaving.extend(footer.own().map(|(extent, at)| (extent_path(extent), Some(at.key))));
        }
        leave(&*dir, cache.as_deref(), leaving);

        // The branch: the bucket's state, the log's footers applied over it.
        let mut replay = state.placed(|extents| Local {
            extents: (extents.into_iter())
                .map(|(extent, at)| (extent, Extent { len: at.len, data: Data::Remote(at) }))
                .collect(),
            segments: HashMap::new(),
        });
        let mut named_own = BTreeSet::new();
        for (footer, number) in wal.segments() {
            let key = extent_remote::layout::segment(branch, footer.seq);
            replay.extents.segments.insert(key, SegmentId { branch: branch.id, number: *number });
            replay.apply_contents(footer.seq, footer.epoch, &footer.contents);
            named_own.extend(footer.own().map(|(extent, _)| extent));
        }
        // fxdax: a log ends with no commit unended (`wal::recover`, TASK-92.41).
        ensure!(replay.begun.is_none(), "branch {branch}'s log ends inside a commit");
        let head = wal.head().or(replay.last_commit);
        let epoch = head.map_or(0, epoch_of);
        let restate = replay.capacity.is_none() && fenced.is_some();
        let capacity = replay.capacity.unwrap_or(capacity);
        let extent_remote::State { extents: Local { extents, .. }, root, spent, .. } = replay;
        // Past every extent of its own the branch holds: fxfs looks for what a crash left below
        // it (`Allocator::open`). Not the footers' next sequence, which their writers stamped
        // past their own floors (`Chain::close`), the next floor being the uploader's to say.
        let own = extents.keys().filter(|extent| extent.branch == branch.id);
        let next_sequence = own.map(|extent| extent.sequence + 1).max().unwrap_or(0);
        let root = root.unwrap_or_default();
        remove_unheld(&*dir, branch.id, &extents, &named_own)?;

        // The segments it holds extents' data in, with the bytes of it each holds, and its own
        // extents' files. They must be there.
        let mut tracked: BTreeMap<SegmentId, u64> = BTreeMap::new();
        for (_, number) in wal.segments() {
            tracked.insert(SegmentId { branch: branch.id, number: *number }, 0);
        }
        let (mut stored, mut own_stored) = (0, 0);
        let mut own_sizes = HashMap::new();
        let mut lineage = BTreeSet::from([branch.id]);
        for (extent, entry) in &extents {
            lineage.insert(extent.branch);
            stored += entry.len;
            if extent.branch == branch.id {
                own_stored += entry.len;
            }
            match &entry.data {
                Data::Pieces(pieces) => {
                    for piece in pieces {
                        *tracked.entry(piece.segment).or_default() += piece.len;
                    }
                }
                Data::Own => {
                    let path = extent_path(*extent);
                    let file = dir.open(&path, true);
                    let file = file.with_context(|| format!("extent {extent:?}'s file"))?;
                    own_sizes.insert(*extent, file.size()?);
                }
                Data::Remote(_) => {}
            }
        }
        let opened = (epoch, recovered.next, chain);
        let mut segments = Segments::open(&*dir, (branch.id, tracked), opened, options.open_files)?;
        // fxdax: the sealed extents in each, read in the bucket once it is acknowledged there
        // (TASK-165.8).
        for (extent, entry) in &extents {
            segments.place(*extent, &entry.data);
        }
        let mut queued = Vec::new();
        for (footer, number) in wal.segments() {
            let restamped = stamped.get(number) != Some(footer);
            let own: u64 = footer.contents.own.iter().map(|(_, len)| len).sum();
            let bytes = segments.len(SegmentId { branch: branch.id, number: *number }) + own;
            queued.push(Queued::new(*number, footer.clone(), restamped, bytes));
        }
        let mut uncommitted = Uncommitted::default();
        // fxdax: a branch saying no capacity has the store's, which its next commit sets in it
        // (TASK-165.3).
        if restate {
            uncommitted.resize(capacity);
        }
        let open = segments.open_segment();
        let log = segments.open_log();
        let log = BTreeMap::from([(open, SegmentLog::new(log.file, log.salt))]);
        #[cfg(any(test, feature = "audit"))]
        let replay = {
            let lengths = extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
            let mut replay = crate::state::Replay::of(branch, crate::state::Lengths(lengths));
            (replay.root, replay.spent) = (Some(root.clone()), spent.clone());
            replay.capacity = (!restate).then_some(capacity);
            replay
        };
        let views = Views::new(epoch, &root);
        let store = Store {
            dir,
            block_size,
            branch,
            lineage,
            bucket,
            volume,
            opened: SharedLock::new("store opening", Mutex::new(Some((claim, taken)))),
            unpacked: Unpacked::default(),
            options,
            state: Arc::new(SharedLock::new(
                "store state",
                Mutex::new(State {
                    extents,
                    space: Space::new(capacity, stored, own_stored),
                    epoch,
                    root,
                    next_sequence,
                    spent,
                    uncommitted,
                    gate: Gate::default(),
                    segments,
                    own_files: OwnFiles::new(branch.id, own_sizes, options.open_files),
                    unreferenced: Unreferenced::default(),
                    wal: Queue::new(wal.base(), queued),
                    point: fenced.is_none(),
                    kept: KeptExtents::default(),
                    sealed: Sealed::default(),
                    standing: Standing::Open,
                    deferred: Vec::new(),
                    framing: Framing::default(),
                    #[cfg(any(test, feature = "audit"))]
                    replay,
                }),
            )),
            committing: SharedLock::new("store commit", Mutex::new(())),
            durability: SharedLock::new("store durability", Mutex::new(Durability::new(epoch))),
            flushing: SharedLock::new(
                "store flush",
                Mutex::new(Logs { by_segment: log, unsynced: Syncs::default() }),
            ),
            base: SharedLock::new("store log base", Mutex::new(base_file)),
            cache,
            idle: Arc::new(Condvar::new()),
            views,
        };
        Ok(store)
    }
}

/// fxdax: a branch's extents as a replay of the footers of its write-ahead log places them over
/// the bucket's ([`Recovering::open`]): sealed in one run of a segment of the log, which
/// `segments` holds by its key in the bucket, or in files of their own. By the run's key, not the
/// segment applied: a commit spanning segments seals its runs in those before (TASK-92.41).
struct Local {
    extents: Extents,
    segments: HashMap<String, SegmentId>,
}

impl Places for Local {
    fn seal(&mut self, extent: ExtentId, seal: Seal) {
        let entry = match seal {
            Seal::Run(Location { key, offset, len }) => {
                let segment = self.segments[&key];
                let piece = Piece { segment, offset, extent_offset: 0, len };
                Extent { len, data: Data::Pieces(vec![piece]) }
            }
            Seal::Own(location) => Extent { len: location.len, data: Data::Own },
        };
        self.extents.insert(extent, entry);
    }

    fn free(&mut self, extent: ExtentId) {
        self.extents.remove(&extent);
    }
}

/// fxdax: the files at the paths of `leaving` leave a store's directory `dir`, the objects in the
/// bucket at their keys, if given: into the host's cache, `cache`, if it takes them, the file
/// renamed; or removed (TASK-165.8). What fails to leave is logged, and removed, the bucket
/// holding it.
fn leave<D: Dir>(dir: &D, cache: Option<&Cache>, leaving: Vec<(String, Option<String>)>) {
    for (path, key) in leaving {
        let adopted = match (cache, key, dir.host_path(&path)) {
            (Some(cache), Some(key), Some(from)) => {
                cache.adopt(&key, &from).unwrap_or_else(|err| {
                    log::warn!("extent store: {path} into the cache, as {key}: {err}");
                    false
                })
            }
            _ => false,
        };
        if !adopted
            && let Err(err) = dir.remove(&path)
            && !not_found(&err)
        {
            log::warn!("extent store: failed to remove {path}: {err}");
        }
    }
}

impl<D: Dir> Store<D> {
    /// fxdax: makes a new store's directory in `dir`, which holds no store, as `description`
    /// describes, its description naming `bucket`, the URL of the bucket its branch is kept in,
    /// if given ([`Store::bucket`]), and `volume`, the volume of its branch's cell it is, if given
    /// ([`Store::volume`], TASK-364); unmade until its maker says it is whole ([`Store::made`]).
    /// The description is made last: until it is durable, the directory is no store either. Its
    /// branch is the bucket's, which opening it takes ([`Store::recover`]).
    pub fn make(
        dir: &D,
        description: Description,
        bucket: Option<&str>,
        volume: Option<Volume>,
    ) -> Result<(), Error> {
        let Description { block_size, capacity, branch } = description;
        ensure!(
            block_size.is_power_of_two() && block_size as usize <= crate::framed::ALIGNMENT,
            "bad block size {block_size}"
        );
        ensure!(capacity >= block_size as u64, "the store is smaller than a block");
        dir.take().context("taking the store's directory")?;
        let made = || -> Result<(), Error> {
            for path in KINDS {
                dir.create_dir(path).with_context(|| format!("creating {path}"))?;
            }
            create_branch_dirs(dir, branch.id)?;
            dir.create(UNMADE, false)?.sync()?;
            let file = dir.create(DESCRIPTION, false)?;
            let bucket = bucket.map(str::to_owned);
            let described = StoreFile { block_size, branch, bucket, capacity, token: None, volume };
            let text = described.text();
            file.write_at(text.as_bytes(), 0)?;
            file.sync()?;
            dir.sync_dir("")?;
            dir.barrier()?;
            Ok(())
        };
        let made = made();
        dir.let_go();
        made
    }

    /// fxdax: the store is whole, as its maker made it: the file saying it is not goes, durably
    /// ([`UNMADE`], TASK-196). Its next openings open it.
    pub fn made(&self) -> Result<(), Error> {
        match self.dir.remove(UNMADE) {
            Err(err) if !not_found(&err) => return Err(err.into()),
            _ => {}
        }
        self.dir.sync_dir("")?;
        Ok(self.dir.barrier()?)
    }

    /// fxdax: makes a new store in `dir` as [`Store::make`] does, whole, kept nowhere, and opens
    /// it: one whose branch the bucket holds nothing of ([`Opening::new`]), as the store's own
    /// tests' are.
    pub fn create(dir: D, description: Description, options: Options) -> Result<Self, Error> {
        Self::make(&dir, description, None, None)?;
        let store = Self::recover_unmade(dir)?.open(options, Opening::new(description.branch))?;
        store.made()?;
        Ok(store)
    }

    /// fxdax: opens the store in `dir`, whose branch the bucket holds nothing of
    /// ([`Opening::new`]), as the store's own tests' is: refused if its write-ahead log had
    /// anything acknowledged.
    pub fn open(dir: D, options: Options) -> Result<Self, Error> {
        let recovering = Self::recover(dir)?;
        let branch = recovering.branch();
        recovering.open(options, Opening::new(branch))
    }

    /// Takes the store in `dir` for this process ([`Dir::take`]), and claims it for an opening
    /// ([`Dir::claim`]), and recovers its write-ahead log ([`crate::wal`]), to be opened as the
    /// bucket holds its branch ([`Recovering::open`]). Fails while another process has it open, or
    /// this one; and with [`Unmade`] if its maker never made it whole (TASK-196).
    pub fn recover(dir: D) -> Result<Recovering<D>, Error> {
        match dir.open(UNMADE, false) {
            Ok(_) => return Err(Unmade.into()),
            Err(err) if not_found(&err) => {}
            Err(err) => return Err(err.into()),
        }
        Self::recover_unmade(dir)
    }

    /// Recovers the store in `dir` as [`Store::recover`] does, unmade or not: its maker's
    /// opening.
    pub fn recover_unmade(dir: D) -> Result<Recovering<D>, Error> {
        dir.take().context("taking the store's directory")?;
        let dir = Arc::new(dir);
        // Dropped, as an opening that fails drops them, they let the directory go.
        let taken = Arc::new(Taken(dir.clone()));
        dir.claim().context("taking the store's directory")?;
        let claim = Claim(dir.clone());
        let file = {
            let file = dir.open(DESCRIPTION, false).context("opening the store's description")?;
            let text = String::from_utf8(read_file(&file)?).context("reading the description")?;
            StoreFile::parse(&text)?
        };
        let wal = crate::wal::recover(&*dir, file.branch)?;
        Ok(Recovering { dir, taken, claim, file, wal })
    }

    /// fxdax: the sequence numbers of the branch's own extents it freed once sealed, which it
    /// writes no more ([`Spent`], TASK-79.6).
    pub fn spent(&self) -> Result<extent_frame::spent::Spent, Error> {
        Ok(self.state()?.spent.clone())
    }

    /// Where `extent`'s data is, if the branch holds it.
    pub fn place(&self, extent: ExtentId) -> Result<Option<Place>, Error> {
        Ok(self.state()?.extents.get(&extent).map(place_of))
    }

    /// The extents the branch holds, with their lengths and where they are, in order.
    pub fn places(&self) -> Result<Vec<(ExtentId, u64, Place)>, Error> {
        let state = self.state()?;
        let mut extents: Vec<_> = state
            .extents
            .iter()
            .map(|(extent, entry)| (*extent, entry.len, place_of(entry)))
            .collect();
        extents.sort_by_key(|(extent, ..)| *extent);
        Ok(extents)
    }

    /// fxdax: the URL of the bucket its branch is kept in, as its description names it: every
    /// store in a host's directory names one ([`crate::create_host`]); none, one of memory's
    /// (TASK-165.6).
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    /// fxdax: the volume of its branch's cell it is, as its description names it (TASK-364).
    pub fn volume(&self) -> Option<&Volume> {
        self.volume.as_ref()
    }

    /// fxdax: holds the store's directory taken for this process until what it returns is dropped,
    /// as the store open does: for what outlives the store's closing and still writes in the
    /// directory, as its uploader does as it stops (TASK-129). Fails once it is closed.
    pub fn hold_dir(&self) -> Result<DirHold<D>, Error> {
        let opened = self.opened.with(Mutex::lock);
        let (_, taken) =
            opened.as_ref().ok_or_else(|| anyhow!("branch {} is closed", self.branch))?;
        Ok(DirHold { _taken: taken.clone() })
    }

    /// fxdax: commits, durably, a commit whose segment's footer names `child`, a fork of the
    /// branch, which its owner and whether it is a point say (doc-7, Forks): the fork is made where
    /// the branch's segments go, as they go, and nowhere here (TASK-165.7). Its segment closes at
    /// the commit. Returns the commit's epoch. One that fails once its commit was made fails with
    /// [`ForkCommitted`]: the segment names the fork, as a host crashing then would leave it.
    pub fn commit_fork(&self, child: Child) -> Result<u64, Error> {
        if self.lineage.contains(&child.branch.id) {
            bail!(Inherited(child.branch));
        }
        let committing = self.committing.with(Mutex::lock);
        self.writable()?.segments.close_for_fork(child);
        self.commit_locked(&committing, None)?;
        let epoch = self.lock().epoch;
        let made = || -> Result<u64, Error> {
            failpoint!("fork::committed");
            self.flush()?;
            failpoint!("fork::flushed");
            Ok(epoch)
        };
        made().context(ForkCommitted { child: child.branch })
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.dir
    }

    /// The store, as its user sees it.
    pub fn description(&self) -> Description {
        Description {
            block_size: self.block_size,
            capacity: self.lock().space.capacity(),
            branch: self.branch,
        }
    }

    /// The state, locked, failed or not: audited as it is let go, in the builds of the tests
    /// ([`Locked`], TASK-160.16).
    fn lock(&self) -> Locked<'_, D::File> {
        Locked::new(self.state.with(Mutex::lock))
    }

    /// The state, to read: unless the store failed ([`Readable`], TASK-160.19).
    fn state(&self) -> Result<Readable<'_, D::File>, Error> {
        Readable::check(self.lock(), self.branch)
    }

    /// The state, to change: unless the store failed, or the branch is closed, or a point
    /// ([`Writable`]), checked again by its waits, which let it go meanwhile (TASK-83).
    fn writable(&self) -> Result<Writable<'_, D::File>, Error> {
        Writable::check(self.lock(), self.branch)
    }

    /// Closes the branch, once the commits only ordered are durable, those a flush in flight is
    /// making durable among them: another store may open it, and this one reads what it holds, as
    /// caches of its extents may still, and changes nothing.
    pub fn close(&self) -> Result<(), Error> {
        // After a flush in flight: its commits are durable, and the files they made unreferenced
        // gone, before another store may open the branch.
        let mut flushing = rules().close_waits_for_flushes.then(|| self.flushing.with(Mutex::lock));
        let flushed = match &mut flushing {
            Some(_) if !self.flush_pending() => Ok(()),
            Some(flushing) => self.flush_locked(flushing, &mut Flush::default()),
            // With the rule off, as if the commits a flush in flight took were durable.
            None if !self.durability.with(Mutex::lock).queued() => Ok(()),
            None => self.flush(),
        };
        drop(flushing);
        let mut state = self.lock();
        if !state.standing.is_closed() {
            // fxdax: once no segment is being framed, whatever closed it, a commit no flush took
            // or one that failed among them, which another store, opening the branch, would frame
            // again meanwhile from its raw file (TASK-41.1, TASK-79.99): each framing ends itself,
            // and the segments say which are running (TASK-153.8). The opening keeps their framed
            // files, if whole, or frames their raw files anew.
            if rules().close_waits_for_framings {
                state = self.framed(state);
            }
            // fxdax: and for the next segment's log being prepared, which writes a spare's header
            // and room: a store opening the branch again meanwhile, taking the spare as a log, would
            // have its header written over, another segment's, its records then none of its own
            // (TASK-333, the Shuttle rounds closing after a failed framing).
            while state.segments.preparing() {
                state = state.wait(&self.idle);
            }
            state.standing.close();
            // Their raw files, of commits durable, go as a flush's would.
            let raw = match state.standing.failed() {
                false => state.segments.raw(self.durability.with(Mutex::lock).durable()),
                true => Vec::new(),
            };
            drop(state);
            self.remove_raw(raw);
            drop(self.opened.with(Mutex::lock).take());
        }
        flushed
    }

    /// Fails unless I/O of `len` bytes at `offset` in an extent is aligned to blocks, and ends
    /// within the most an extent holds.
    pub fn check_io(&self, offset: u64, len: u64) -> Result<(), Error> {
        let block_size = self.block_size as u64;
        if !offset.is_multiple_of(block_size) || !len.is_multiple_of(block_size) || len == 0 {
            bail!(Unaligned { offset, len });
        }
        if offset.checked_add(len).is_none_or(|end| end > self.options.max_extent) {
            bail!(TooLong { offset, len });
        }
        Ok(())
    }

    /// A segment's file, opened if it isn't: the open segment's raw, a closed one's framed.
    fn segment_file(
        &self,
        state: &mut State<D::File>,
        number: SegmentId,
    ) -> Result<Source<D::File>, Error> {
        state.segments.source(&*self.dir, number)
    }

    /// The file `extent`'s data is in, stored on its own: raw while it is open, framed once sealed.
    fn own_source(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
    ) -> Result<Source<D::File>, Error> {
        match state.uncommitted.placement(extent) {
            Some(Placement::Own(file)) => Ok(Source::Raw(file.clone())),
            _ => Ok(Source::Framed(state.own_files.file(&*self.dir, extent)?)),
        }
    }

    /// Whether a commit, or a closing between commits, waits for the writes in progress, holding
    /// new ones back ([`Store::begin_write`]).
    pub fn holding_writes(&self) -> bool {
        self.lock().gate.is_held()
    }

    /// Begins writing `len` bytes at `offset` in `extent`, which must be the store's branch's and
    /// open: not yet written, or written since the last commit. The caller writes the data to the
    /// returned write's file, and finishes it.
    pub fn begin_write(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Write<'_, D>, Error> {
        self.check_io(offset, len)?;
        if extent.branch != self.branch.id {
            bail!(OtherBranch(extent));
        }
        // Not while a commit, or a closing between commits, waits for the writes in progress.
        let state = self.writable()?.wait_while(&self.idle, |state| state.gate.is_held())?;
        self.begin_write_locked(state, extent, offset, len)
    }

    /// fxdax: begins a write as [`Store::begin_write`] does, or, while a commit, or a closing
    /// between commits, holds writes back, returns `None` at once rather than wait on the
    /// caller's thread (TASK-98): an executor's thread waiting there may be the one the writes in
    /// progress need to end, as futures of the same task, and the commit waits for them for
    /// ever. Such a caller waits elsewhere ([`Store::wait_until_writes_resume`]) and tries again.
    pub fn try_begin_write(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Option<Write<'_, D>>, Error> {
        self.check_io(offset, len)?;
        if extent.branch != self.branch.id {
            bail!(OtherBranch(extent));
        }
        let state = self.writable()?;
        if state.gate.is_held() {
            return Ok(None);
        }
        self.begin_write_locked(state, extent, offset, len).map(Some)
    }

    /// fxdax: waits, blocking the calling thread, until no commit, nor closing between commits,
    /// holds writes back ([`Store::try_begin_write`]).
    pub fn wait_until_writes_resume(&self) {
        let mut state = self.lock();
        while state.gate.is_held() {
            state = state.wait(&self.idle);
        }
    }

    /// fxdax: fails unless `extent`, the branch's, whose life in `state` is `life`, may be opened,
    /// as a write opens it: unless it is open, or never sealed by the branch, held or freed
    /// ([`Spent`], TASK-79.6). One kept, freed, is spent if it was the branch's own.
    fn check_opens(state: &State<D::File>, life: &Life<'_>, extent: ExtentId) -> Result<(), Error> {
        match life {
            Life::Open(_) | Life::Unknown => Ok(()),
            Life::Held(_) => bail!(SealedExtent(extent)),
            Life::Spent => bail!(Spent(extent)),
            Life::Kept(_) if state.spent.contains(extent.sequence) => bail!(Spent(extent)),
            Life::Kept(_) => Ok(()),
        }
    }

    /// Begins a write, as [`Store::begin_write`] says, with `state`, writable, held, and no commit
    /// holding writes back.
    fn begin_write_locked(
        &self,
        mut state: Writable<'_, D::File>,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Write<'_, D>, Error> {
        let end = offset + len;
        let (grows, to_segment, overwrites) = {
            let life = state.life(extent);
            Self::check_opens(&state, &life, extent)?;
            let current = life.held();
            let grows = end.saturating_sub(current.map_or(0, |entry| entry.len));
            let to_segment = match state.uncommitted.placement(extent) {
                Some(placement) => matches!(placement, Placement::Segment),
                None => end < self.options.large_extent,
            };
            // fxdax: what it writes over, in a segment, takes room too (TASK-79.2), as the writes
            // that finished left the extent: not its holes (TASK-160.16).
            let overwrites = match current.map(|entry| &entry.data) {
                Some(Data::Pieces(pieces)) if to_segment => covered(pieces, offset, end),
                _ => 0,
            };
            (grows, to_segment, overwrites)
        };
        state.check_room(grows, overwrites)?;
        if to_segment {
            self.make_room(&mut state, len)?;
        }
        let (placement, opening) = match state.uncommitted.opening(extent) {
            Some(opened) => opened,
            None => {
                let placement = if end >= self.options.large_extent {
                    // Raw until it is sealed, when it is framed in its own file.
                    Placement::Own(Arc::new(self.create_raw(extent)?))
                } else {
                    Placement::Segment
                };
                let opening = state.uncommitted.open(extent, placement.clone());
                (placement, opening)
            }
        };
        let (file, file_offset, segment) = match placement {
            Placement::Own(file) => (file, offset, None),
            Placement::Segment => {
                let (number, file, at) = state.segments.reserve(len);
                (file, at, Some(number))
            }
        };
        state.space.reserve(grows);
        state.gate.pass();
        Ok(Write {
            store: self,
            extent,
            opening,
            extent_offset: offset,
            len,
            file,
            offset: file_offset,
            segment,
            grows,
            finished: false,
        })
    }

    /// The raw file of `extent`, not open, made for its writes until it is sealed. fxdax: one left
    /// at its path holds nothing the extent does, as no open extent's writes go to it: a free's, of
    /// the extent while it was open, which the next commit's flush removes. It makes way, so that
    /// the extent may be written anew before that flush (TASK-79.5).
    fn create_raw(&self, extent: ExtentId) -> Result<D::File, Error> {
        let (dir, path) = (&*self.dir, open_extent_path(extent));
        let created = match dir.create(&path, true) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                dir.remove(&path).and_then(|()| dir.create(&path, true))
            }
            created => created,
        };
        created.with_context(|| format!("creating {path}"))
    }

    /// fxdax: makes room in the open segment for a write of `len` bytes, if it has none: writes
    /// go to the next (TASK-79.2). The open segment, full, takes no more, and waits for the next
    /// commit, which seals in place what is one run in it and closes it (`Segments::fill`,
    /// TASK-92.41): holding the state alone, so that the write waits neither for a commit in
    /// progress, nor for the writes in the segment, which go on, nor for a framing. Before, it was
    /// closed then, holding commits, or set aside, and the next commit copied out of it every
    /// extent written since.
    fn make_room(&self, state: &mut State<D::File>, len: u64) -> Result<(), Error> {
        if state.segments.fits(len, self.options.segment_limit()) {
            return Ok(());
        }
        state.segments.fill(&*self.dir, state.epoch)?;
        self.prepare_next(state);
        Ok(())
    }

    /// fxdax: what frames the store's segments, and prepares its next segment's log, off its
    /// commits ([`Framer`]).
    fn framer(&self) -> Framer<D> {
        let (dir, state, idle) = (self.dir.clone(), self.state.clone(), self.idle.clone());
        Framer { dir, branch: self.branch.id, state, idle }
    }

    /// fxdax: prepares the log of the segment after the open one, off the state lock, on a thread
    /// of its own, unless it is or is being (`Segments::to_prepare`, TASK-333): the segment's
    /// opening takes it, syncing nothing, where it wrote and synced its header holding the state
    /// lock, every write and read of the store, and its flushes, waiting 0.1-1.4 s behind the
    /// host's journal on a disk VMs share. fxdax: placed there under its name, beside the
    /// segment's raw file, holding the state, then their entries made durable, holding nothing
    /// (`Segments::prepared`, TASK-384): the commits made in the segment, opened, then wait for no
    /// sync of the directories of raw files and logs, which each segment's opening cost the next
    /// flush, a commit of the host's journal a durable commit waited for. Here, holding it, as
    /// before, if the directory does one thing at a time, or the host refuses the thread.
    fn prepare_next(&self, state: &mut State<D::File>) {
        let Some(preparation) = state.segments.to_prepare() else { return };
        let preparation = match self.dir.concurrent() {
            true => {
                let opener = self.framer();
                let prepare = move |preparation: Preparation| {
                    let prepared = preparation.prepare(&*opener.dir);
                    let preparing = opener.lock().segments.prepared(&*opener.dir, prepared);
                    if let Preparing::Placed(segment) = preparing {
                        let synced = sync_entries(&*opener.dir, segment.branch);
                        opener.lock().segments.placed_synced(segment, synced);
                    } else if let Preparing::Ended(stale) = preparing {
                        remove_spare(&*opener.dir, stale);
                    }
                    opener.idle.notify_all();
                };
                match spawn_or_return("store opener", preparation, prepare) {
                    None => return,
                    Some(preparation) => preparation,
                }
            }
            false => preparation,
        };
        let prepared = preparation.prepare(&*self.dir);
        match state.segments.prepared(&*self.dir, prepared) {
            Preparing::Placed(segment) => {
                let synced = sync_entries(&*self.dir, segment.branch);
                state.segments.placed_synced(segment, synced);
            }
            Preparing::Ended(stale) => remove_spare(&*self.dir, stale),
        }
    }

    /// fxdax: the number of the next segment, if its raw file and log are there, under their
    /// names, ahead of it, its preparation's ([`Store::prepare_next`], TASK-384): what reads the
    /// directory beside the store, as a test does, finds them holding nothing, as an opening
    /// would, which removes them.
    pub fn placed(&self) -> Option<u64> {
        self.lock().segments.placed().map(|segment| segment.number)
    }

    /// fxdax: waits for the preparation of the next segment's log to end, if one runs
    /// ([`Store::prepare_next`]): its raw file and log there, under their names, their entries
    /// durable, until the segment opens, as what counts the store's files counts them (TASK-333,
    /// TASK-384).
    pub fn prepared(&self) -> Result<(), Error> {
        let mut state = self.lock();
        while state.segments.preparing() {
            state = state.wait(&self.idle);
        }
        Ok(())
    }

    /// Records `write`, done; returns what of the open segment's raw file to write back, if
    /// anything ([`Segments::write_behind`]).
    fn finish_write(&self, write: &Write<'_, D>) -> Option<Behind<D::File>> {
        let mut state = self.lock();
        let state = &mut *state;
        // A write to an extent freed since it began is dropped, as though it finished before the
        // free: the extent, if written again since, is another opening's, whose data may go
        // elsewhere, and which its bytes are no part of (TASK-79.1). A commit waits for the writes
        // in progress before it seals what they write to: a free, not a seal, ended the opening.
        if rules().drop_writes_to_freed_extents
            && !state.uncommitted.is_open_in(write.extent, write.opening)
        {
            self.drop_write(state, write);
            return None;
        }
        state.space.unreserve(write.grows);
        let extent = write.extent;
        let end = write.extent_offset + write.len;
        let entry = state.extents.entry(extent).or_insert_with(|| Extent {
            len: 0,
            data: if write.segment.is_some() { Data::Pieces(Vec::new()) } else { Data::Own },
        });
        let grew = end.saturating_sub(entry.len);
        if grew > 0 {
            // A write's extent is its branch's.
            state.space.added(grew, true);
            entry.len = end;
        }
        match (write.segment, &mut entry.data) {
            (Some(number), Data::Pieces(pieces)) => {
                let piece = Piece {
                    segment: number,
                    offset: write.offset,
                    extent_offset: write.extent_offset,
                    len: write.len,
                };
                // fxdax: what the extent held of where it wrote: not its holes, which writes past
                // its end grew it by, and which it writes over nothing in (TASK-160.16).
                let over = covered(pieces, write.extent_offset, end);
                add_piece(pieces, piece);
                if state.segments.held(number, write.len) {
                    self.idle.notify_all();
                }
                state.uncommitted.wrote_over(extent, over);
                state.uncommitted.push(Change::Write { extent, piece });
            }
            // Its file is the branch's to claim once it is sealed, and framed.
            (None, Data::Own) => {
                state.uncommitted.push(Change::WriteOwn { extent, len: entry.len });
            }
            _ => unreachable!("an extent's data is in segments or its own file"),
        }
        state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        self.written(state);
        let number = write.segment?;
        state.segments.write_behind(number, write.offset + write.len)
    }

    fn abandon_write(&self, write: &Write<'_, D>) {
        self.drop_write(&mut self.lock(), write);
    }

    /// `write` is done, and holds nothing: abandoned, or to an extent freed since it began. What it
    /// wrote to a segment, if anything, is dead space; fxdax: which its writer wasted, until the
    /// next commit (TASK-79.2).
    fn drop_write(&self, state: &mut State<D::File>, write: &Write<'_, D>) {
        state.space.unreserve(write.grows);
        if let Some(number) = write.segment {
            state.uncommitted.let_go(write.len);
            if state.segments.dropped(number, write.len) {
                self.idle.notify_all();
            }
        }
        self.written(state);
    }

    /// A write in progress is done, finished or abandoned: a commit waiting for the last goes on.
    fn written(&self, state: &mut State<D::File>) {
        if state.gate.done() {
            self.idle.notify_all();
        }
    }

    /// Where the `len` bytes at `offset` in `extent` are: in files, or zeros, where nothing was
    /// written, which is anything past the extent's end.
    pub fn locate(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Vec<Part<D::File>>, Error> {
        self.locate_entry(extent, offset, len, Reader::Store)
    }

    /// Where the bytes are, as [`Store::locate`] says, of an extent `reader` may read.
    fn locate_entry(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        reader: Reader,
    ) -> Result<Vec<Part<D::File>>, Error> {
        self.check_io(offset, len)?;
        let mut state = self.state()?;
        let entry = state.entry_for(extent, reader)?;
        self.parts_locked(&mut state, extent, &entry, offset, len)
    }

    /// Where the `len` bytes at `offset` in `extent`, `entry` its entry, are, holding the state.
    fn parts_locked(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
        entry: &Extent,
        offset: u64,
        len: u64,
    ) -> Result<Vec<Part<D::File>>, Error> {
        let end = offset + len;
        let held_end = end.min(entry.len).max(offset);
        let unpacked = &self.unpacked;
        let mut parts = Vec::new();
        match &entry.data {
            Data::Own => {
                if held_end > offset {
                    let source = self.own_source(state, extent)?;
                    source.locate(offset, held_end - offset, unpacked, &mut parts)?;
                }
            }
            Data::Remote(location) => bail!(RemoteData { extent, location: location.clone() }),
            Data::Pieces(pieces) => {
                for (start, end, source) in paint(pieces, offset, held_end) {
                    match source {
                        None => parts.push(Part::Zeros(end - start)),
                        Some((number, at)) => {
                            let source = self.segment_file(state, number)?;
                            source.locate(at, end - start, unpacked, &mut parts)?;
                        }
                    }
                }
            }
        }
        if end > held_end {
            parts.push(Part::Zeros(end - held_end));
        }
        Ok(parts)
    }

    /// Reads `parts` into `buf`, as long as they are.
    fn read_parts(&self, parts: Vec<Part<D::File>>, buf: &mut [u8]) -> Result<(), Error> {
        let mut at = 0;
        for part in parts {
            let len = part.len() as usize;
            let out = &mut buf[at..at + len];
            match part {
                Part::Data { file, offset, len } => {
                    let mut data = Aligned::new(len as usize);
                    file.read_cached(data.as_mut_slice(), offset)?;
                    out.copy_from_slice(data.as_slice());
                }
                Part::Packed { part, skip, .. } => {
                    let bytes = self.read_packed(&part)?;
                    out.copy_from_slice(&bytes[skip as usize..skip as usize + len]);
                }
                Part::Unpacked { bytes, skip, .. } => {
                    out.copy_from_slice(&bytes[skip as usize..skip as usize + len]);
                }
                Part::Zeros(_) => out.fill(0),
            }
            at += len;
        }
        Ok(())
    }

    /// A part of a framed file, read, checked and unpacked.
    pub fn read_packed(&self, part: &Packed<D::File>) -> Result<Arc<Aligned>, Error> {
        let stored = read_aligned(&*part.file, part.stored.offset, part.stored.len as usize)?;
        self.unpack(part, stored)
    }

    /// A part of a framed file, unpacked from `stored`, its stored bytes and any after them, read
    /// from its file, checked against its CRC; reads of it find it unpacked for a while.
    pub fn unpack(&self, part: &Packed<D::File>, stored: Aligned) -> Result<Arc<Aligned>, Error> {
        part.unpack(stored, &self.unpacked)
    }

    /// Frees `extent`, which may be open, if the store holds it.
    pub fn free(&self, extent: ExtentId) -> Result<(), Error> {
        let mut state = self.writable()?;
        let state = &mut *state;
        let placement = state.uncommitted.remove(extent);
        if let Some(Placement::Own(_)) = placement {
            // Its raw file, which it is not sealed from, or a write abandoned created.
            state.unreferenced.doomed.push(open_extent_path(extent));
        }
        let Some(entry) = state.extents.remove(&extent) else { return Ok(()) };
        // fxdax: sealed, the branch's own is written no more (TASK-79.6).
        if placement.is_none() && extent.branch == self.branch.id {
            state.spent.insert(extent.sequence);
        }
        state.space.removed(entry.len, extent.branch == self.branch.id);
        state.uncommitted.push(Change::Free(extent));
        // A sealed one's data stays until the uploader has what it needs of it, and until the
        // commit that frees it decides whether views keep it. fxdax: an open one's goes at once:
        // no commit held it, so that no reader needs it, and what it took of segments is dead
        // space, counted so (TASK-79.2). Of a file of its own it has none, which its seal would
        // have made, but its raw file, which goes with the next commit: nothing else of it to let
        // go of, which would take the file the extent's next opening seals at that path
        // (TASK-79.1). What it took of either its writer wasted, until the next commit
        // (TASK-79.2).
        if placement.is_none() {
            state.segments.displace(extent, &entry.data);
            // fxdax: for the bucket, until the commit that frees it is acknowledged there, as the
            // segment holding its seal is by then (TASK-165.9).
            let uploader = rules().keep_frees;
            let freed = state.epoch + 1;
            state.kept.insert(extent, Kept { entry, freed, uploader });
            return Ok(());
        }
        match &entry.data {
            Data::Pieces(pieces) => {
                for piece in pieces {
                    state.segments.let_go(piece);
                    state.uncommitted.let_go(piece.len);
                }
            }
            Data::Own => state.uncommitted.let_go(entry.len),
            Data::Remote(_) => unreachable!("an open extent's data is in segments or a raw file"),
        }
        Ok(())
    }

    /// fxdax: reads `buf.len()` bytes at `offset` in `extent`, held, or freed and kept: for views,
    /// or for the bucket, until the commit that freed it is acknowledged there (TASK-165.9).
    pub fn read_kept(&self, extent: ExtentId, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.read_into_entry(extent, offset, buf, Reader::Kept)
    }

    /// Commits what was written and freed since the last commit, and `root`, if given, as the
    /// store's root, durably: a barrier, after which the store recovers at least this. It seals
    /// the extents written before it.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<(), Error> {
        self.commit_ordered(root)?;
        self.flush()
    }

    /// fxdax: commits as [`Store::commit`] does, but without waiting for durability: a barrier,
    /// which orders what came before it before what comes after, and seals the extents written
    /// before it. A crash keeps a prefix of the commits, which [`Store::flush`] makes durable in
    /// order: doc-4's `COMMIT` without `COMMIT_DURABLE`.
    pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<(), Error> {
        let committing = self.committing.with(Mutex::lock);
        self.commit_locked(&committing, root)
    }

    /// Commits, as [`Store::commit_ordered`] does, holding `_committing`.
    fn commit_locked(
        &self,
        _committing: &Shared<MutexGuard<'_, ()>>,
        root: Option<&[u8]>,
    ) -> Result<(), Error> {
        let mut state = self.writable()?;
        // It includes the writes in progress, which may be to extents it seals: what they write
        // must be in them, not after. New ones wait for it, checked again after (TASK-83).
        if rules().commit_waits_for_writes {
            state = state.hold_writes_while(&self.idle, |state| state.gate.is_writing())?;
        }
        // From its first seal on, the state runs ahead of the files: a commit failing part way
        // fails the store.
        let committed = state.failing(|state| self.commit_in(state, root));
        committed.context("committing the extent store")
    }

    /// The steps of the next commit, which sets `root`, if any, in their order: each takes what
    /// those before it leave.
    /// fxdax: its seal, and what it writes of it, each in a span of its own, `commit.seal` and
    /// `commit.write` (`extent_remote::trace`, TASK-258).
    fn commit_in(&self, state: &mut State<D::File>, root: Option<&[u8]>) -> Result<(), Error> {
        let epoch = state.epoch + 1;
        let seal = trace::Span::start("commit.seal");
        let sealing = seal.run(|| self.seal_open(state, &seal))?;
        drop(seal);
        let write = trace::Span::start("commit.write");
        write.set_count(trace::EPOCH, epoch);
        write.run(|| self.write_commit(state, epoch, root, &sealing))
    }

    /// Writes commit `epoch`, which sets `root`, if any, and seals `sealing`, sealed: publishes it
    /// to views, records it in the open segment, and queues its flush.
    fn write_commit(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        root: Option<&[u8]>,
        sealing: &[ExtentId],
    ) -> Result<(), Error> {
        self.publish(state, epoch, root, sealing);
        let (changes, capacity) = (state.uncommitted.take(), state.uncommitted.take_capacity());
        // The open segment's footer records them, before the segment closes; fxdax: and its log,
        // unless it closes with them (TASK-165.9).
        let changed = (root, capacity);
        let records =
            state.segments.record(epoch, &changes, changed, &state.extents, state.next_sequence)?;
        if let Some(root) = root {
            state.root = root.to_vec();
        }
        self.committed(state, epoch, records)
    }

    /// fxdax: what follows commit `epoch`, whose contents are `records`, by the segments it spans,
    /// in the state: the segments filled since the last commit closed and framed, which it sealed
    /// in, the open segment closed if it is due, the segments it let go of, and what its flush
    /// makes durable, queued.
    fn committed(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        records: Vec<(SegmentId, Contents)>,
    ) -> Result<(), Error> {
        // fxdax: with the rule off, the segments the last commit closed are framed only now, by a
        // commit that may never come (TASK-150.2).
        let mut closed = std::mem::take(&mut state.deferred);
        // fxdax: those filled since the last commit, in the order they filled, before the open
        // one, as the chain stamps them (TASK-92.41).
        for unframed in state.segments.close_full(epoch, state.next_sequence) {
            self.enqueue(state, &unframed);
            match rules().framings_end_themselves {
                true => closed.push(unframed),
                false => state.deferred.push(unframed),
            }
        }
        // fxdax: framed together, made durable together once framed (TASK-310), whether or not
        // the open segment's closing fails.
        let due = self.close_if_due(state, epoch).map(|due| closed.extend(due));
        self.frame_behind(state, closed);
        due?;
        // What its flush syncs, after the closing, whose directories its flush must sync: the
        // next segment's raw file's and log's entries durable before a commit made in it.
        let mut syncs = Syncs::default();
        state.own_files.take_syncs(&mut syncs);
        let entries = state.segments.take_syncs(&mut syncs);
        // The segments none of its extents' data is in any more, acknowledged in the bucket: their
        // files go into the host's cache once it is durable (TASK-165.8).
        let leaving = (state.segments.reap().into_iter())
            .map(|(number, key)| (segment_path(number), key))
            .collect();
        let Unreferenced { doomed } = state.unreferenced.take();
        state.epoch = epoch;
        // Queued while the state is locked, so that commits queue in their order.
        let pending = Pending { syncs, records, doomed, leaving, entries };
        self.durability.with(Mutex::lock).push(epoch, pending);
        Ok(())
    }

    /// Seals the extents open, as a commit does, each in one run, or a file of its own if large
    /// (doc-7, Segments), and returns them; fxdax: saying on `span` how many, and the bytes it
    /// copied.
    fn seal_open(
        &self,
        state: &mut State<D::File>,
        span: &trace::Span,
    ) -> Result<Vec<ExtentId>, Error> {
        let sealing = state.uncommitted.open_extents();
        // fxdax: what sealing copied, which the commit's flush syncs: said once past 8 MiB, which
        // the flush of extentd's turn writes at once (TASK-221).
        let (mut copies, mut copied, mut largest) = (0, 0, 0);
        // fxdax: why it copies what it copies, with FXDAX_SEAL_STATS set (TASK-92.41).
        let mut why = seals::enabled().then(seals::Sealing::default);
        for &extent in &sealing {
            if let Some(why) = &mut why
                && let Some((sealed, len, pieces)) = self.why_sealed(state, extent)
            {
                why.sealed(sealed, len, pieces);
            }
            let bytes = self.seal(state, extent)?;
            copies += u64::from(bytes > 0);
            copied += bytes;
            largest = largest.max(bytes);
        }
        if copied > 8 << 20 {
            log::warn!(
                "extent store: branch {}'s commit sealed {} extents, copying {copies} of them, \
                 {} KiB, the largest {} KiB",
                self.branch,
                sealing.len(),
                copied >> 10,
                largest >> 10
            );
        }
        if let Some(why) = why {
            why.log(self.branch.id, state.epoch);
        }
        span.set_count(trace::EXTENTS, sealing.len() as u64);
        span.set_count(trace::BYTES, copied);
        Ok(sealing)
    }

    /// fxdax: why sealing `extent` copies it, or not, its length and how many pieces it is in;
    /// none if sealing does nothing with it (TASK-92.41).
    fn why_sealed(
        &self,
        state: &State<D::File>,
        extent: ExtentId,
    ) -> Option<(seals::Why, u64, usize)> {
        let entry = state.extents.get(&extent)?;
        match (&entry.data, state.uncommitted.placement(extent)) {
            (Data::Own, Some(Placement::Own(_))) => Some((seals::Why::Own, entry.len, 1)),
            (Data::Pieces(pieces), _) => {
                let large = entry.len >= self.options.large_extent;
                let stage = |segment| state.segments.stage(segment);
                let why = seals::Why::of_pieces(pieces, entry.len, stage, large);
                Some((why, entry.len, pieces.len()))
            }
            _ => None,
        }
    }

    /// Publishes commit `epoch`, which sets `root`, if any, and seals `sealing`, to views, which
    /// open as of it from now on, deciding at once what of the frees they keep: those of views as
    /// of earlier commits (see `Views`).
    fn publish(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        root: Option<&[u8]>,
        sealing: &[ExtentId],
    ) {
        let published = root.unwrap_or(&state.root).to_vec();
        let limits =
            Limits { lag: self.options.view_lag, kept_bytes: self.options.view_kept_bytes };
        let kept = &mut state.kept;
        let keeping =
            self.views.publish(epoch, &published, limits, |keeping| kept.views_bytes(keeping));
        state.trim_kept(&keeping, epoch);
        // What it sealed, as views older than it, if any lives, must refuse; what all of them
        // were sealed by is theirs.
        state.sealed.published(keeping.oldest(), sealing, epoch);
    }

    /// Closes the open segment after commit `epoch`, once no write is in progress in it, if it is
    /// due: full, old, wanted closed, or at a fork; writes go to a new one. Returns it, to be
    /// framed off the commit ([`Store::frame_behind`]).
    fn close_if_due(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
    ) -> Result<Option<Unframed<D::File>>, Error> {
        if !state.segments.due(&self.options) {
            return Ok(None);
        }
        self.close_open_segment(state, epoch).map(Some)
    }

    /// Seals `extent`, which a commit seals, in one run in the open segment, or in a file of its
    /// own, framed, if it is large, copying its data there unless it is. fxdax: or where it is, if
    /// one run of a segment filled since the last commit, which the commit goes on with
    /// (`Segments::fill`, TASK-92.41); a copy past the open segment's room filling it, and going in
    /// the next. Returns the bytes it copied, if it copied them.
    fn seal(&self, state: &mut State<D::File>, extent: ExtentId) -> Result<u64, Error> {
        let Some(entry) = state.extents.get(&extent).cloned() else {
            // fxdax: opened by a write abandoned, none since: its raw file, if it has one, holds
            // nothing, and goes now, as the commit waited for the writes in progress; not with the
            // commit's flush, which the extent's next write, making another, may come before
            // (TASK-79.5). Opening the store removes it, failing here.
            if let Some(Placement::Own(_)) = state.uncommitted.placement(extent) {
                let path = open_extent_path(extent);
                if let Err(err) = self.dir.remove(&path) {
                    log::warn!("extent store: failed to remove {path}: {err}");
                }
            }
            return Ok(0);
        };
        let large = entry.len >= self.options.large_extent;
        match (&entry.data, state.uncommitted.placement(extent)) {
            (Data::Remote(_), _) => return Ok(0),
            // Written to a raw file of its own, which it is framed from, or in.
            (Data::Own, Some(Placement::Own(raw))) => {
                let raw = raw.clone();
                return self.seal_own(state, extent, raw, entry.len);
            }
            (Data::Pieces(pieces), _) if !large => {
                let one_run = matches!(pieces[..], [piece] if state.segments.seals_in(piece.segment)
                    && piece.extent_offset == 0
                    && piece.len == entry.len);
                if one_run {
                    state.segments.place(extent, &entry.data);
                    return Ok(0);
                }
            }
            _ => {}
        }
        let Data::Pieces(pieces) = &entry.data else {
            bail!("{extent:?}, open, is in neither segments nor a file of its own")
        };
        let mut data = Aligned::new(entry.len as usize);
        let parts = self.parts_locked(state, extent, &entry, 0, entry.len)?;
        self.read_parts(parts, data.as_mut_slice())?;
        // Its old places go.
        for piece in pieces {
            state.segments.let_go(piece);
        }
        let data_now = if large {
            let data = data.as_slice();
            self.write_own(state, extent, entry.len, |at, buf| {
                buf.copy_from_slice(&data[at as usize..at as usize + buf.len()]);
                Ok(())
            })?;
            state.uncommitted.push(Change::MoveOwn { extent });
            Data::Own
        } else {
            // fxdax: in the next, the open one full, which the commit goes on with past it.
            if !state.segments.fits(entry.len, self.options.segment_limit()) {
                state.segments.fill(&*self.dir, state.epoch)?;
                self.prepare_next(state);
            }
            let open = state.segments.open_segment();
            let (file, at) = state.segments.append(entry.len);
            file.write_at(data.as_slice(), at)?;
            written::wrote(written::Kind::Copied, data.as_slice().len());
            let pieces =
                vec![Piece { segment: open, offset: at, extent_offset: 0, len: entry.len }];
            state.uncommitted.push(Change::Move { extent, pieces: pieces.clone() });
            Data::Pieces(pieces)
        };
        state.segments.place(extent, &data_now);
        state.extents.get_mut(&extent).unwrap().data = data_now;
        Ok(entry.len)
    }

    /// fxdax: seals `extent`, open in `raw`, a raw file of its own, of its `len` bytes, in its own
    /// file, framed: in place, if none of its parts compresses, as an object of such parts holds
    /// them as they are, where they are (`extent_frame::Packer`), its raw file that file then, its
    /// tail appended, renamed; or else in a file of its own, as [`Store::write_own`] frames one,
    /// the parts before the first that compresses copied there. A large extent's data was written
    /// twice, in its raw file, which the host's disk wrote back as its guest wrote it, and in the
    /// framed file: as much again as its guest wrote, its framings and the disk's other writers
    /// waiting behind it (TASK-310). In place, its bytes don't change, which a read of the raw
    /// file in progress reads. Returns the bytes it copied: none in place.
    fn seal_own(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
        raw: Arc<D::File>,
        len: u64,
    ) -> Result<u64, Error> {
        let (raw_path, path) = (open_extent_path(extent), extent_path(extent));
        let mut packer = extent_frame::Packer::new();
        let mut data = Aligned::new(len.min(OWN_CHUNK) as usize);
        // The framed file, once a part compresses.
        let mut framed: Option<D::File> = None;
        for at in (0..len).step_by(OWN_CHUNK as usize) {
            let buf = &mut data.as_mut_slice()[..(len - at).min(OWN_CHUNK) as usize];
            raw.read_cached(buf, at)?;
            let end = packer.end();
            let packed = packer.push(buf);
            // As they are, where they are, as those before.
            if framed.is_none() && end == at && packed[..] == buf[..] {
                continue;
            }
            let file = match &mut framed {
                Some(file) => file,
                None => {
                    let file =
                        self.dir.create(&path, true).with_context(|| format!("creating {path}"))?;
                    copy_range(&*raw, &file, end)?;
                    framed.insert(file)
                }
            };
            file.write_at(&packed, end)?;
            written::wrote(written::Kind::Own, packed.len());
        }
        let end = packer.end();
        let tail = packer.finish(&[]);
        let parts = extent_frame::Parts::of_suffix(&tail)?;
        let (file, copied) = match framed {
            Some(file) => {
                file.write_at(&tail, end)?;
                state.unreferenced.doomed.push(raw_path);
                (Arc::new(file), len)
            }
            None => {
                raw.write_at(&tail, end)?;
                self.dir
                    .rename(&raw_path, &path)
                    .with_context(|| format!("renaming {raw_path}"))?;
                (raw, 0)
            }
        };
        written::wrote(written::Kind::Own, tail.len());
        state.own_files.sealed(extent, Arc::new(Framed::new(file, parts)));
        Ok(copied)
    }

    /// Writes `extent`'s own file, framed, of its `len` bytes, which `read` reads, by ranges, into
    /// buffers: so many parts at a time.
    fn write_own(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
        len: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
    ) -> Result<(), Error> {
        let path = extent_path(extent);
        let file = self.dir.create(&path, true).with_context(|| format!("creating {path}"))?;
        let mut packer = extent_frame::Packer::new();
        let mut data = Aligned::new(len.min(OWN_CHUNK) as usize);
        for at in (0..len).step_by(OWN_CHUNK as usize) {
            let buf = &mut data.as_mut_slice()[..(len - at).min(OWN_CHUNK) as usize];
            read(at, buf)?;
            let end = packer.end();
            let packed = packer.push(buf);
            file.write_at(&packed, end)?;
            written::wrote(written::Kind::Own, packed.len());
        }
        let end = packer.end();
        let tail = packer.finish(&[]);
        file.write_at(&tail, end)?;
        written::wrote(written::Kind::Own, tail.len());
        let parts = extent_frame::Parts::of_suffix(&tail)?;
        let framed = Arc::new(Framed::new(Arc::new(file), parts));
        state.own_files.sealed(extent, framed);
        Ok(())
    }

    /// Closes the open segment, after commit `epoch`: its footer, of what the commits since it
    /// opened changed, stamped by the chain of the uploader's writer, if any, after its data, which
    /// is to be framed in a file of its own, as object storage keeps segments; and opens another.
    fn close_open_segment(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
    ) -> Result<Unframed<D::File>, Error> {
        let unframed = state.segments.close(&*self.dir, epoch, state.next_sequence)?;
        self.enqueue(state, &unframed);
        self.prepare_next(state);
        Ok(unframed)
    }

    /// fxdax: `unframed`, a segment just closed, is a segment of the write-ahead log from now on,
    /// until acknowledged (TASK-165.9), holding the bytes of its data framed, and of the extents
    /// stored on their own its footer names.
    fn enqueue(&self, state: &mut State<D::File>, unframed: &Unframed<D::File>) {
        #[cfg(any(test, feature = "audit"))]
        state.closed(&unframed.footer);
        let own: u64 = unframed.footer.contents.own.iter().map(|(_, len)| len).sum();
        let bytes = unframed.len() + own;
        state.wal.push(unframed.number().number, unframed.footer.clone(), bytes);
    }

    /// Has the next commit close the open segment, whatever it holds: for its data to be uploaded
    /// with its footer.
    pub fn close_segment(&self) -> Result<(), Error> {
        self.writable()?.segments.want_close();
        Ok(())
    }

    /// fxdax: closes the open segment now, between commits, once no write is in progress in it,
    /// if commits made since it opened, or forks: for its uploader to upload what the commits made
    /// so far without waiting for the next. Writes wait for the closing alone: it is framed, and
    /// made durable, by a framing that ends itself, as a commit's closing is
    /// ([`Store::frame_behind`]), waited for holding no lock, so that the store serves its writes,
    /// reads and commits meanwhile, which read the segment from its raw file (TASK-41.1). Its
    /// framed file is durable when this returns; its raw file goes once the commits its footer
    /// names are durable, as a commit's closing's does: so that opening the branch reads no
    /// segment whose raw file is gone (TASK-92.16). Returns whether it closed.
    pub fn close_now(&self) -> Result<bool, Error> {
        self.close_now_unless(false)
    }

    /// fxdax: closes as [`Store::close_now`] does, unless the guest is filling the segment it would
    /// close (`Segments::filling`): its next commit seals in place what is one run in it, where
    /// closed now that commit copies all it wrote since out of it (TASK-92.41). What an uploader's
    /// interval does; a commit waiting for its upload closes it whatever it holds. Returns whether
    /// it closed.
    pub fn close_now_unless_filling(&self) -> Result<bool, Error> {
        self.close_now_unless(true)
    }

    /// fxdax: closes as [`Store::close_now`] does, `unless_filling` as
    /// [`Store::close_now_unless_filling`] does, for the uploader: until its framing wrote its
    /// framed file, which it uploads, not until the framing made it durable, which its raw file
    /// and log stand for meanwhile, until the commits its footer names are (TASK-384). A durable
    /// commit waited, one after another, for the segment's framed file synced, the directory of
    /// framed files after it, each a commit of the host's journal, before its upload began.
    /// Returns whether it closed.
    pub fn close_for_upload(&self, unless_filling: bool) -> Result<bool, Error> {
        let Some(number) = self.close_between_commits(unless_filling)? else { return Ok(false) };
        self.wait_for_writings([number]).context("closing the open segment")?;
        Ok(true)
    }

    /// fxdax: [`Store::close_now`], unless `filling` and the guest is filling the segment.
    fn close_now_unless(&self, filling: bool) -> Result<bool, Error> {
        let Some(number) = self.close_between_commits(filling)? else { return Ok(false) };
        let mut state = self.lock();
        while state.segments.is_framing(number) {
            state = state.wait(&self.idle);
        }
        match state.segments.framing_failure(number) {
            Some(error) => bail!("closing the open segment: framing it: {error}"),
            None => Ok(true),
        }
    }

    /// fxdax: closes the open segment, between commits, as [`Store::close_now`] does, framing it
    /// behind, and returns it: none if no commit came since it opened, nor fork, or the branch is
    /// closed. fxdax: or the first segment filled since the last commit, holding commits, which
    /// would wait for the next otherwise (`Segments::fill`, TASK-92.41).
    fn close_between_commits(&self, unless_filling: bool) -> Result<Option<SegmentId>, Error> {
        let _committing = self.committing.with(Mutex::lock);
        let state = self.state()?;
        // Closed, it changes nothing: its last segment closes when it is next opened.
        if state.standing.is_closed() || !state.segments.has_commits(state.epoch) {
            return Ok(None);
        }
        if unless_filling && state.segments.filling() {
            return Ok(None);
        }
        let mut state = state.hold_writes_while(&self.idle, |state| {
            rules().close_waits_for_writers && state.segments.closing_writers() > 0
        })?;
        // fxdax: and closed while it waited, as before (TASK-83).
        if state.standing.is_closed() {
            return Ok(None);
        }
        let epoch = state.epoch;
        let unframed = state.failing(|state| match state.segments.has_full() {
            true => {
                let unframed = state.segments.close_first_full(epoch, state.next_sequence);
                self.enqueue(state, &unframed);
                Ok(unframed)
            }
            false => self.close_open_segment(state, epoch),
        });
        let unframed = unframed.context("closing the open segment")?;
        let number = unframed.number();
        self.frame_behind(&mut state, [unframed]);
        Ok(Some(number))
    }

    /// fxdax: frames `closed`, segments just closed, which `state` holds, off what closed them:
    /// a commit, a write finding no room, a closing between commits or a fork's commit, alike. Its
    /// framing writes it ([`Unframed::frame`]) and makes it durable, with the segments framed
    /// meanwhile (`segments::make_durable`, TASK-310), then ends itself, holding the state: the
    /// segment read from its framed file from now on, its raw file going once the commits its
    /// footer names are durable ([`Segments::framed`]); or the store failed. Whatever comes next,
    /// or never does, only waits for that, as the flush of the commit that closed it, a fork, the
    /// branch's closing and [`Store::close_now`] do (TASK-153.8): it ends whether or not a commit
    /// comes after (TASK-150.2), and a closing waits for every one, being counted
    /// ([`Segments::framings`], TASK-79.99). On a thread of its own, which would otherwise hold up
    /// every write and commit behind it for as long as reading, compressing, writing and syncing a
    /// segment takes (TASK-41: 67 ms under pgbench, the store's commits' time); on the caller's,
    /// holding them up as before TASK-41, if the directory's I/O is its callers' alone
    /// ([`Dir::concurrent`], TASK-79.79), or if the host refuses a thread, rather than panic
    /// through the store's locks, which the caller holds (TASK-79.80).
    fn frame_behind(
        &self,
        state: &mut State<D::File>,
        closed: impl IntoIterator<Item = Unframed<D::File>>,
    ) {
        // fxdax: one framer at a time, in the order the segments closed (`wal`'s docs,
        // TASK-165.9); each segment made durable once framed, with those framed meanwhile, by
        // another thread ([`Framer`], TASK-310).
        for unframed in closed {
            state.segments.framing_begun(unframed.number());
            state.framing.queue.push_back(unframed);
        }
        if state.framing.queue.is_empty() || std::mem::replace(&mut state.framing.running, true) {
            return;
        }
        let framer = self.framer();
        if self.dir.concurrent() {
            let thread = framer.clone();
            if spawn_or_return("store framer", (), move |()| thread.frame_queued()).is_none() {
                return;
            }
        }
        framer.frame_here(state);
    }

    /// fxdax: removes the files at `paths`, as nothing refers to them: logging what it can't,
    /// which opening the store removes.
    fn remove(&self, paths: impl IntoIterator<Item = String>) {
        for path in paths {
            if let Err(err) = self.dir.remove(&path)
                && !not_found(&err)
            {
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
    }

    /// fxdax: the numbers of the branch's segments being framed behind what closed them
    /// ([`Store::frame_behind`]): their framed files, in the store's directory already, are whole
    /// only once their framings end, their raw files holding their data meanwhile. What reads the
    /// directory beside the store, as a test does, reads no other framed file (TASK-189).
    pub fn framing(&self) -> Vec<u64> {
        let state = self.lock();
        let framing = state.segments.framings().iter();
        let ours = framing.filter(|segment| segment.branch == self.branch.id);
        ours.map(|segment| segment.number).collect()
    }

    /// fxdax: the last commit whose segments the uploader may take: the last durable. A segment of
    /// commits a crash may lose here, acknowledged in the bucket, would leave the branch past the
    /// write-ahead log the store opens with, which its opening reads as diverged (`ESTALE`), the
    /// commits its own (TASK-165.9). The uploader flushes before it takes them, but for commits
    /// made between.
    fn uploadable(&self) -> u64 {
        self.durability.with(Mutex::lock).durable()
    }

    /// fxdax: waits, holding no lock, for the framings of segment `number` and those before it to
    /// end, their framed files durable, which the uploader waits for no more before it uploads
    /// them (TASK-384): fails if one failed.
    fn wait_framed_through(&self, number: u64) -> Result<(), Error> {
        let framing =
            |segment: &SegmentId| segment.branch == self.branch.id && segment.number <= number;
        let mut state = self.lock();
        while state.segments.framings().iter().any(framing) {
            state = state.wait(&self.idle);
        }
        let segment = SegmentId { branch: self.branch.id, number };
        match state.segments.framing_failure(segment) {
            Some(error) => bail!("framing segment {number:#x}: {error}"),
            None => Ok(()),
        }
    }

    /// fxdax: waits, holding no lock, for every framing running to end, its framed file durable,
    /// or failed, which it fails then: what reads the directory beside the store, as a test does,
    /// finds the segments framed then named their own, their raw files and logs going with the next
    /// flush, if their commits are durable. The uploader waits for no more than their framed files
    /// written (`Store::closed`, TASK-384).
    pub fn wait_framed(&self) -> Result<(), Error> {
        let framing = self.state()?.segments.framings().clone();
        let mut state = self.lock();
        // And the thread making them durable done with them.
        while framing.iter().any(|&number| state.segments.is_framing(number))
            || state.framing.syncing
        {
            state = state.wait(&self.idle);
        }
        match framing.iter().find_map(|&number| state.segments.framing_failure(number)) {
            Some(error) => bail!("framing a segment: {error}"),
            None => Ok(()),
        }
    }

    /// fxdax: `state`, once no segment is being framed ([`Store::frame_behind`]), which it let go
    /// of meanwhile.
    fn framed<'a>(&self, mut state: Locked<'a, D::File>) -> Locked<'a, D::File> {
        while !state.segments.framings().is_empty() {
            state = state.wait(&self.idle);
        }
        state
    }

    /// fxdax: the segments of the write-ahead log not yet taken, oldest first, `limit` of them at
    /// most, for the uploader, which has them until it says they are acknowledged
    /// ([`Store::acknowledged`]): the rest it takes later, their files not opened meanwhile. Only
    /// those whose commits are durable ([`Store::uploadable`]). Those being framed it waits for,
    /// holding no lock: each framing ends itself, whatever comes next, which may be nothing, as
    /// after a write that closed one for want of room (TASK-150.2, TASK-153.8). fxdax: until it
    /// wrote their framed files, not until it made them durable, which their raw files and logs
    /// stand for meanwhile, until the commits their footers name are (TASK-384).
    pub fn closed(&self, limit: usize) -> Result<Vec<Closed<D::File>>, Error> {
        let framing = self.state()?.segments.framings().clone();
        self.wait_for_writings(framing)?;
        let mut state = self.state()?;
        let state = &mut *state;
        // fxdax: and framed: its framed file is what is uploaded.
        let segments = &state.segments;
        let writing = |number| segments.is_writing(SegmentId { branch: self.branch.id, number });
        let taken: Vec<(u64, Footer, bool)> = (state.wal.take(self.uploadable(), writing, limit))
            .into_iter()
            .map(|queued| (queued.number, queued.footer.clone(), queued.restamped))
            .collect();
        let mut closed = Vec::with_capacity(taken.len());
        for (number, footer, restamped) in taken {
            let segment = SegmentId { branch: self.branch.id, number };
            let file = state.segments.framed_file(&*self.dir, segment)?.file.clone();
            let mut own = Vec::with_capacity(footer.contents.own.len());
            for (extent, location) in footer.own() {
                let file = state.own_files.file(&*self.dir, extent)?;
                own.push((location.key, file.file.clone()));
            }
            closed.push(Closed { number, footer, file, own, restamped });
        }
        Ok(closed)
    }

    /// fxdax: puts the write-ahead log after the branch's end as a writer taking it again found
    /// it, `fenced` (doc-7, Fencing; TASK-204): the segments of it that landed are acknowledged,
    /// the rest stamped anew in memory, for the writer to take again, and the open segment's footer
    /// goes on from them. As opening the store does, live: what the guest committed stays the
    /// store's. Fails `Diverged` if another writer's commits moved the branch past the log.
    pub fn rebase(&self, fenced: &Fenced) -> Result<(), Error> {
        let (landed, ended) = {
            let mut state = self.state()?;
            let (landed, chain, ended) = state.wal.rebase(fenced)?;
            state.segments.set_chain(chain);
            (landed, ended)
        };
        for number in landed {
            self.acknowledged(number)?;
        }
        if let Some((number, footer)) = ended {
            self.landed_ending(number, &footer)?;
        }
        Ok(())
    }

    /// fxdax: the commits segment `number`, the first of the write-ahead log, ends landed in the
    /// bucket, as `footer`, its footer as it landed, names them, though it goes on with the next
    /// commit, stamped again with that alone (`extent_remote::Wal::rebase`, TASK-345): as
    /// [`Store::acknowledged`] does of a segment's commits, the log's base, the last of them, is
    /// durable first; then their extents are read in the bucket, in the object that landed, their
    /// own files leaving for the host's cache, and their frees are the bucket's. The segment stays
    /// the log's first, its file with it. Its extents read where it was stamped again instead,
    /// once acknowledged, would be read in an object whose data the branch holds no more of but
    /// what it goes on with, which the collector may compact away.
    fn landed_ending(&self, number: u64, footer: &Footer) -> Result<(), Error> {
        let mut base = self.base.with(Mutex::lock);
        let last = footer.contents.commits.last().copied();
        if let Err(err) = base.write(&*self.dir, last) {
            let err = err.context("recording an acknowledgement");
            self.lock().fail(&err);
            return Err(err);
        }
        let leaving = {
            let mut state = self.state()?;
            let state = &mut *state;
            state.wal.landed_ending(last);
            state.release_kept(last.map_or(0, epoch_of));
            state.trim_kept(&self.views.keeping(), state.epoch);
            let key = extent_remote::layout::segment(self.branch, footer.seq);
            let extents: Vec<ExtentId> = footer.contents.extents.iter().map(|run| run.0).collect();
            let own: Vec<_> = footer.own().collect();
            let segment = SegmentId { branch: self.branch.id, number };
            state.landed_ending(segment, &key, &extents, &own)
        };
        leave(&*self.dir, self.cache.as_deref(), leaving);
        Ok(())
    }

    /// fxdax: the extents `moved` says the collector moved, from where to where, as the uploader's
    /// writer names them where they went before its checkpoint lands (`Writer::following`): those
    /// the store reads in the bucket, held or kept, are named there too, if it names them on their
    /// way there, so that it names none further back than the writer's newest checkpoint, from
    /// which the collector keeps the moves (TASK-195). Before, it named each where it was sealed,
    /// whose object the collector deleted, and the moves after, once compacted again.
    pub fn moved(&self, moved: &[(ExtentId, Location, Location)]) -> Result<(), Error> {
        self.state()?.moved(moved);
        Ok(())
    }

    /// fxdax: segment `number`, which [`Store::closed`] gave, the first of the write-ahead log, is
    /// acknowledged in the bucket: the extents sealed in it, and those stored on their own its
    /// footer names, are read there from now on ([`RemoteData`]), and their files leave the store
    /// for the host's cache, or go: the extents' own files, and the segment's, once no extent's
    /// data is in it here (TASK-165.8). The frees of its commits are the bucket's to keep. The
    /// log's base, its last commit from now on, is durable first, so that the store opened again
    /// puts what is left of the log after the branch (TASK-165.9).
    ///
    /// fxdax: once its framing ended, its framed file durable, which its upload waited for no
    /// more, but for its framed file written (TASK-384): its upload and the syncs making it
    /// durable here go on at once, the bucket's round trip mostly the longer; and once it is
    /// acknowledged, its files leave as before, its framed file named its own, into the host's
    /// cache, where reads of its extents in the bucket find it, and its raw file and log go.
    pub fn acknowledged(&self, number: u64) -> Result<(), Error> {
        self.wait_framed_through(number)?;
        let mut base = self.base.with(Mutex::lock);
        let after = {
            let state = self.state()?;
            match state.wal.first() {
                Some(queued) if queued.number == number => {
                    queued.footer.contents.commits.last().copied().or(state.wal.base())
                }
                _ => bail!("segment {number:#x} is not the first of the write-ahead log"),
            }
        };
        if let Err(err) = base.write(&*self.dir, after) {
            let err = err.context("recording an acknowledgement");
            self.lock().fail(&err);
            return Err(err);
        }
        let (leaving, raw) = {
            let mut state = self.state()?;
            let state = &mut *state;
            // fxdax: those whose acknowledgements are whole, of a commit spanning segments all
            // once its last is acknowledged (`Queue::acknowledge`, TASK-338).
            let whole = state.wal.acknowledge(number).expect("the first, as found");
            for queued in &whole {
                state.release_kept(footer_last(&queued.footer));
            }
            state.trim_kept(&self.views.keeping(), state.epoch);
            let mut leaving = Vec::new();
            for queued in whole {
                let footer = &queued.footer;
                let key = extent_remote::layout::segment(self.branch, footer.seq);
                let own: Vec<_> = footer.own().collect();
                let segment = SegmentId { branch: self.branch.id, number: queued.number };
                leaving.extend(state.acknowledge(segment, (&key, !queued.restamped), &own));
            }
            // Those no extent's data is in here any more leave now.
            let reaped = state.segments.reap().into_iter();
            leaving.extend(reaped.map(|(number, key)| (segment_path(number), key)));
            // fxdax: and the raw files and logs of the segments framed whose commits are durable,
            // as those acknowledged are, which no flush may come to remove (TASK-165.9).
            let raw = state.segments.raw(self.durability.with(Mutex::lock).durable());
            (leaving, raw)
        };
        self.remove_raw(raw);
        leave(&*self.dir, self.cache.as_deref(), leaving);
        Ok(())
    }

    /// fxdax: makes the commits ordered so far durable, in order, as few syncs as they need
    /// together: their data, then their records in the logs of the segments they were made in,
    /// then the files they made unreferenced go; and the raw files and logs of the segments framed
    /// whose commits are durable. It waits for no framing (TASK-165.9).
    ///
    /// fxdax: in a span of its own, `store.flush` (`extent_remote::trace`, TASK-258): a durable
    /// commit's, or of the commits ordered before, which a daemon's turn makes durable.
    pub fn flush(&self) -> Result<(), Error> {
        let span = trace::Span::start("store.flush");
        span.set_branch(self.branch);
        span.run(|| {
            // fxdax: timed, phase by phase (TASK-165.9).
            let asked = Instant::now();
            // fxdax: and its waits to run (TASK-336).
            let waits = Waits::start();
            let mut logs = self.flushing.with(Mutex::lock);
            let mut timed = Flush::default();
            timed.phases.took(flush::WAIT, asked);
            let flushed = self.flush_locked(&mut logs, &mut timed);
            timed.phases.runnable_us = waits.us();
            timings::flushed(self.branch.id, &timed);
            span.set_count(trace::COMMITS, timed.commits as u64);
            flushed
        })
    }

    /// [`Store::flush`], holding `flushing`, and so `logs`, those of the open segment and of the
    /// segments closed since that the last flush left; timing it in `timed`.
    fn flush_locked(&self, logs: &mut Logs<D::File>, timed: &mut Flush) -> Result<(), Error> {
        // A store that failed makes nothing more durable.
        drop(self.state()?);
        let batch = self.durability.with(Mutex::lock).take();
        let appended = batch.is_some();
        let durable = match batch {
            None => self.durability.with(Mutex::lock).durable(),
            Some(Batch { last, mut commits }) => {
                if let Err(error) = self.make_durable(logs, &mut commits, timed) {
                    let error = error.context("committing the extent store");
                    self.lock().fail(&error);
                    return Err(error);
                }
                self.durability.with(Mutex::lock).finish(last);
                // fxdax: the entries of the segments opened its syncs made durable: no making of
                // them durable syncs them again (TASK-369).
                let entries =
                    commits.iter().flat_map(|pending| &pending.entries).copied().collect();
                self.lock().segments.entries_synced(&entries);
                // The files the commits let go of go, once they are durable; fxdax: those of the
                // bucket's, into the host's cache (TASK-165.8).
                let started = Instant::now();
                let mut leaving = Vec::new();
                for pending in commits {
                    self.remove(pending.doomed);
                    leaving.extend(pending.leaving);
                }
                leave(&*self.dir, self.cache.as_deref(), leaving);
                timed.phases.took(flush::GONE, started);
                last
            }
        };
        // fxdax: and the raw files and logs of the segments framed, whose footers name commits
        // durable now (TASK-153.8).
        let started = Instant::now();
        let raw = self.state()?.segments.raw(durable);
        self.remove_raw(raw);
        timed.phases.took(flush::RAW, started);
        // fxdax: and the next segment's log prepared, ahead of its opening (TASK-333): by a flush
        // that made commits durable, a store that writes, not as a store opens. A store never
        // written to, as a fork's that only reads, takes none: every fork a daemon served took
        // the next segment's log, and a file held open (serves_hundreds_of_forks_of_a_base).
        if appended {
            self.prepare_next(&mut *self.state()?);
        }
        Ok(())
    }

    /// fxdax: removes the raw files and logs of `raw`, segments framed, whose framed files, and
    /// their entries, an opening reads their commits from now: their framings made them durable
    /// before they ended (`segments::make_durable`), so that it syncs nothing, a flush's syncs
    /// waiting behind the host's disk enough (TASK-310). A log the branch wants as a spare is
    /// renamed one, rather than removed ([`spare_path`], TASK-301): the log of a segment to come,
    /// written in place, its blocks there.
    fn remove_raw(&self, raw: Vec<SegmentId>) {
        self.remove(raw.iter().map(|segment| open_segment_path(*segment)));
        for segment in raw {
            let (log, spare) = (log_path(segment), spare_path(segment));
            // A log without a header of its own holds records no salt tells from a later one's.
            if self.lock().segments.wants_spare()
                && let Ok(file) = self.dir.open(&log, false)
                && let mut header = [0; LOG_HEADER]
                && file.read_cached(&mut header, 0).is_ok()
                && log_salt(&header, segment).is_some()
            {
                match self.dir.rename(&log, &spare) {
                    Ok(()) => {
                        self.lock().segments.spare(segment);
                        continue;
                    }
                    Err(err) => log::warn!("extent store: keeping {log} as a spare: {err}"),
                }
            }
            self.remove([log]);
        }
    }

    /// Makes `commits`, a flush's, durable, in order: their data, and their records in the logs
    /// of the segments they were made in, open, or closed since; but for the files they made
    /// unreferenced. fxdax: it waits for no framing, those of segments the uploader closes between
    /// commits among them, which held up the turns a daemon makes its guests' commits durable in
    /// (TASK-165.9): an opening closes a segment whose framed file isn't whole again from its log.
    /// fxdax: in one sync's time, not two (TASK-301, [`LogRecord`]): commits that sealed little
    /// (`Options::held_most`) hold their runs in their records, and only the logs are synced, as a
    /// database's write-ahead log is; others' records check their runs, synced at once with them.
    /// Data first, then records, if the commits stored extents on their own, which no record
    /// checks. At once, the records go first in a directory that does one thing at a time, as crash
    /// tests' do: a crash between keeps records whose data it lost.
    fn make_durable(
        &self,
        logs: &mut Logs<D::File>,
        commits: &mut [Pending<D::File>],
        timed: &mut Flush,
    ) -> Result<(), Error> {
        let started = Instant::now();
        timed.commits = commits.len();
        let mut syncs = Syncs::default();
        for pending in commits.iter_mut() {
            syncs.extend(std::mem::take(&mut pending.syncs));
        }
        if syncs.holds_own() {
            syncs.extend(std::mem::take(&mut logs.unsynced));
            timed.synced = syncs.sync_files(&*self.dir)?;
            let synced = timed.phases.took(flush::DATA, started);
            syncs.sync_dirs(&*self.dir)?;
            let synced = timed.phases.took(flush::DIRS, synced);
            let records = self.records(commits, Runs::Synced)?;
            self.append_records(logs, &records)?;
            self.dir.barrier()?;
            timed.phases.took(flush::LOGS, synced);
        } else {
            let records = commits.iter().flat_map(|pending| &pending.records);
            let sealed: u64 = records.map(|(_, contents)| LogRecord::run_bytes(contents)).sum();
            let hold = sealed <= self.options.held_most;
            // The raw files it leaves unsynced, the next flush syncing data syncs.
            match hold {
                true => logs.unsynced.carry(syncs.take_cached()),
                false => syncs.extend(std::mem::take(&mut logs.unsynced)),
            }
            let runs =
                if hold { Runs::Held(Default::default()) } else { Runs::Checked(Vec::new()) };
            let records =
                self.records(commits, runs).context("reading back the runs commits sealed")?;
            let recorded = timed.phases.took(flush::DATA, started);
            // What is synced beside the records: the directories alone, if they hold the runs.
            let sync_data = || -> Result<FileSyncs, Error> {
                let synced =
                    if hold { FileSyncs::default() } else { syncs.sync_files(&*self.dir)? };
                syncs.sync_dirs(&*self.dir)?;
                Ok(synced)
            };
            // Nothing beside the records, as a commit's when the segment holds the runs: no thread.
            timed.synced = match syncs.holds_any(!hold) {
                true => self.at_once(|| self.append_records(logs, &records), sync_data)?,
                false => self.append_records(logs, &records).map(|()| FileSyncs::default())?,
            };
            self.dir.barrier()?;
            timed.phases.took(flush::LOGS, recorded);
        }
        // fxdax: the logs of segments whose logs went: no commit made in them is still to come.
        // Those of segments closed since stay, with where their records end, which their files'
        // sizes don't say of spares (TASK-301): a flush may yet append commits made in them before
        // they closed, as one taking those ordered as the uploader closed the segment does.
        let state = self.lock();
        logs.by_segment.retain(|segment, _| state.segments.has_log(*segment));
        Ok(())
    }

    /// fxdax: runs `first`, and `then`, at once, `then` on a thread of its own, one of the helpers
    /// every flush of the process shares (`durability::HELPERS`); or in turn, on this thread, if
    /// none is free, the directory does one thing at a time, or the host refuses the thread. Not
    /// at once under Shuttle, which fails an execution that ends with a scoped thread not run, as
    /// a flush on a thread a test leaves behind would.
    fn at_once<T: Send>(
        &self,
        first: impl FnOnce() -> Result<(), Error>,
        then: impl FnOnce() -> Result<T, Error> + Send + Copy,
    ) -> Result<T, Error> {
        let helper = crate::durability::Helpers::take(1);
        if !self.dir.concurrent() || cfg!(feature = "shuttle") || helper.count() == 0 {
            first()?;
            return then();
        }
        thread::scope(|scope| {
            let other = thread::Builder::new().spawn_scoped(scope, then);
            let done = first();
            let other = match other {
                Ok(other) => other.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                Err(error) => {
                    log::warn!("extent store: a flush's syncs in turn: {error}");
                    then()
                }
            };
            done.and(other)
        })
    }

    /// fxdax: the records of `commits`, a flush's, by the segments they were made in, in order:
    /// each the commit's contents, and what `runs` says of the runs it sealed ([`LogRecord`]);
    /// fxdax: of a commit spanning segments, one in each, in their order (TASK-92.41).
    fn records(
        &self,
        commits: &[Pending<D::File>],
        runs: Runs,
    ) -> Result<Vec<(SegmentId, LogRecord)>, Error> {
        let mut raw: Option<(SegmentId, Arc<D::File>)> = None;
        let mut records = Vec::with_capacity(commits.len());
        let spanned = commits.iter().flat_map(|pending| &pending.records);
        for (segment, contents) in spanned {
            let record = match runs {
                Runs::Synced => LogRecord { contents: contents.clone(), runs: Runs::Synced },
                Runs::Checked(_) | Runs::Held(_) => {
                    let file = match &raw {
                        Some((at, file)) if at == segment => file.clone(),
                        _ => {
                            let file = self.lock().segments.raw_file(&*self.dir, *segment)?;
                            raw = Some((*segment, file.clone()));
                            file
                        }
                    };
                    LogRecord::of(contents.clone(), &*file, matches!(runs, Runs::Held(_)))?
                }
            };
            records.push((*segment, record));
        }
        Ok(records)
    }

    /// fxdax: appends `records` to the logs of their segments, each segment's at once, and syncs
    /// them.
    fn append_records(
        &self,
        logs: &mut Logs<D::File>,
        records: &[(SegmentId, LogRecord)],
    ) -> Result<(), Error> {
        let mut at = 0;
        while at < records.len() {
            let segment = records[at].0;
            let run = records[at..].iter().take_while(|(of, _)| *of == segment).count();
            let log = match logs.by_segment.entry(segment) {
                std::collections::btree_map::Entry::Occupied(log) => log.into_mut(),
                // Its first records: a flush keeps a segment's log while it has one.
                std::collections::btree_map::Entry::Vacant(vacant) => {
                    let log = self.lock().segments.log_file(&*self.dir, segment)?;
                    vacant.insert(SegmentLog::new(log.file, log.salt))
                }
            };
            // Salted as its log's (`wal::LOG_MAGIC`).
            let salt = log.salt();
            let encoded: Vec<Vec<u8>> = records[at..at + run]
                .iter()
                .map(|(_, record)| encode_salted(record, salt))
                .collect();
            log.append(encoded.iter().map(Vec::as_slice)).context("appending commits' records")?;
            at += run;
        }
        Ok(())
    }

    /// fxdax: waits, holding no lock, for the framings of `segments` to write their framed files,
    /// or end, each of which ends itself ([`Store::frame_behind`]): fails if one failed. Not for
    /// them to make the framed files durable, which the uploader waits for no more (TASK-384).
    fn wait_for_writings(
        &self,
        segments: impl IntoIterator<Item = SegmentId>,
    ) -> Result<(), Error> {
        let segments: Vec<SegmentId> = segments.into_iter().collect();
        let mut state = self.lock();
        while segments.iter().any(|&number| state.segments.is_writing(number)) {
            state = state.wait(&self.idle);
        }
        match segments.iter().find_map(|&number| state.segments.framing_failure(number)) {
            Some(error) => bail!("framing a segment: {error}"),
            None => Ok(()),
        }
    }

    /// fxdax: whether commits ordered are not durable yet, which [`Store::flush`] makes durable:
    /// waiting for a flush to take them, or taken by one in flight, which a flush waits for. Those
    /// taken counted not, a daemon's turn finding none took commits for durable that weren't,
    /// a flush of the uploader's in flight (TASK-165.9).
    pub fn flush_pending(&self) -> bool {
        self.durability.with(Mutex::lock).pending()
    }

    /// Reads `buf.len()` bytes at `offset` in `extent`, as `locate` finds them.
    fn read_into(&self, extent: ExtentId, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.read_into_entry(extent, offset, buf, Reader::Store)
    }

    fn read_into_entry(
        &self,
        extent: ExtentId,
        offset: u64,
        buf: &mut [u8],
        reader: Reader,
    ) -> Result<(), Error> {
        let parts = self.locate_entry(extent, offset, buf.len() as u64, reader)?;
        self.read_parts(parts, buf)
    }

    /// Reads `buf.len()` bytes at `offset` in `extent`. Bytes never written read as zeros.
    pub fn read(&self, extent: ExtentId, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.read_into(extent, offset, buf)
    }

    /// Writes `data` at `offset` in `extent` (see `begin_write`).
    pub fn write(&self, extent: ExtentId, offset: u64, data: &[u8]) -> Result<(), Error> {
        let fill = |at: u64, buf: &mut [u8]| buf.copy_from_slice(&data[at as usize..][..buf.len()]);
        self.write_with(extent, offset, data.len() as u64, data.len(), fill)
    }

    /// fxdax: writes `len` bytes at `offset` in `extent`, as [`Store::write`] does, one write, but
    /// `piece` bytes at a time at most, each of which `fill(at, buf)` fills with the bytes at `at`
    /// of them: so that what a write holds of memory is a piece, whatever its length, which a VM
    /// process names (TASK-149.17).
    pub fn write_with(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        piece: usize,
        mut fill: impl FnMut(u64, &mut [u8]),
    ) -> Result<(), Error> {
        let write = self.begin_write(extent, offset, len)?;
        let mut aligned = Aligned::new((len as usize).min(piece.max(1)));
        let mut at = 0;
        while at < len {
            let buf = &mut aligned.as_mut_slice()[..(len - at).min(piece as u64) as usize];
            fill(at, buf);
            write.file().write_at(buf, write.offset() + at)?;
            at += buf.len() as u64;
        }
        write.finish();
        Ok(())
    }

    /// Sets the most bytes the branch's extents may hold, durably: fxdax: a commit of its own,
    /// which seals nothing, and takes none of what changed since the last commit, the next's, so
    /// that the extents a guest has open stay open (TASK-165.9); ordered with the others, as its
    /// segment's footer says (TASK-165.3). It may grow it, or shrink it no lower than what the
    /// extents hold, with what writes in progress may add ([`TooSmall`]).
    pub fn set_capacity(&self, capacity: u64) -> Result<(), Error> {
        let committing = self.committing.with(Mutex::lock);
        {
            let mut state = self.writable()?;
            let block_size = self.block_size as u64;
            ensure!(capacity >= block_size, "a capacity of {capacity} is less than a block");
            state.space.check_capacity(capacity)?;
            let committed = state.failing(|state| {
                state.space.set_capacity(capacity);
                state.uncommitted.take_capacity();
                let epoch = state.epoch + 1;
                self.publish(state, epoch, None, &[]);
                let (changed, next_sequence) = ((None, Some(capacity)), state.next_sequence);
                let records =
                    state.segments.record(epoch, &[], changed, &state.extents, next_sequence)?;
                self.committed(state, epoch, records)
            });
            committed.context("committing the capacity")?;
        }
        drop(committing);
        self.flush()
    }

    /// fxdax: the bytes of its write-ahead log not yet acknowledged in the bucket, those the open
    /// segment's commits sealed included: what the uploader's lag bounds (TASK-165.9).
    pub fn wal_bytes(&self) -> Result<u64, Error> {
        let state = self.state()?;
        Ok(state.wal.bytes() + state.segments.open_bytes())
    }

    /// The root as of the last commit: empty if none set one.
    pub fn root(&self) -> Result<Vec<u8>, Error> {
        Ok(self.state()?.root.clone())
    }

    pub fn info(&self) -> Result<Info, Error> {
        Ok(self.state()?.info())
    }

    pub fn usage(&self) -> Result<Usage, Error> {
        let state = self.state()?;
        let segments = state.segments.usage();
        Ok(Usage {
            own_bytes: (state.extents.iter().into_iter())
                .filter(|(_, entry)| entry.data == Data::Own)
                .map(|(_, entry)| entry.len)
                .sum(),
            disk_bytes: segments.disk_bytes + state.own_files.bytes(),
            kept_bytes: state.kept.bytes(),
            kept_for_views: state.kept.for_views(),
            ..segments
        })
    }

    /// fxdax: the bytes of frees views keep, which count as used ([`Usage::kept_for_views`]):
    /// found as [`Store::info`] finds what it says, at once, without [`Store::usage`]'s walk of
    /// every extent the branch holds, its base's with them, which holds the state meanwhile
    /// (TASK-92.17).
    pub fn kept_for_views(&self) -> Result<u64, Error> {
        Ok(self.state()?.kept.for_views())
    }

    /// fxdax: a view of the branch as of its latest commit, which a follower reads through
    /// ([`Store::read_view`]) while the writer goes on. It waits for no commit.
    pub fn view(&self) -> View {
        self.views.open()
    }

    /// fxdax: waits at most `timeout` for a commit after the commit of `epoch`, returning the
    /// latest commit's epoch, if one came: what tells the branch's followers.
    pub fn newer(&self, epoch: u64, timeout: Duration) -> Option<u64> {
        self.views.newer(epoch, timeout)
    }

    /// fxdax: what the branch's live views are: how many, how far behind (TASK-42.7).
    pub fn view_stats(&self) -> crate::ViewStats {
        self.views.stats()
    }

    /// fxdax: cuts the branch's views off: their reads, advances and pins fail with
    /// [`CutOff`](crate::CutOff), and the commits after keep nothing for them.
    pub fn cut_views(&self) {
        self.views.cut_all()
    }

    /// fxdax: reads `buf.len()` bytes at `offset` in `extent`, which `view`, of this branch, holds:
    /// held still, or freed since and kept. Fails with [`CutOff`](crate::CutOff) if the view was
    /// cut off, and with `NoExtent` if its commit doesn't hold `extent`: one open, or sealed by a
    /// later commit. So a follower's journal replay, which reads the journal on until it finds
    /// nothing, ends at the view's commit.
    pub fn read_view(
        &self,
        view: &View,
        extent: ExtentId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<(), Error> {
        self.views.check(view)?;
        self.read_into_entry(extent, offset, buf, self.view_reader(view, extent))
    }

    /// fxdax: where the `len` bytes at `offset` in `extent`, which `view` holds, are, as
    /// [`Store::locate`] says.
    pub fn locate_view(
        &self,
        view: &View,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Vec<Part<D::File>>, Error> {
        self.views.check(view)?;
        self.locate_entry(extent, offset, len, self.view_reader(view, extent))
    }

    /// fxdax: `extent`'s length, as of `view`'s commit, or as `view` pins it. Fails as
    /// [`Store::read_view`] does.
    pub fn view_extent(&self, view: &View, extent: ExtentId) -> Result<u64, Error> {
        self.views.check(view)?;
        let state = self.state()?;
        Ok(state.entry_for(extent, self.view_reader(view, extent))?.len)
    }

    /// `view`'s reads of `extent`.
    fn view_reader(&self, view: &View, extent: ExtentId) -> Reader {
        Reader::View { epoch: view.epoch(), since: view.since(), pinned: view.pins(extent) }
    }

    /// `extent`'s length, and whether it is open (written since the last commit), if the store
    /// holds it.
    pub fn extent(&self, extent: ExtentId) -> Result<Option<(u64, bool)>, Error> {
        let state = self.state()?;
        let life = state.life(extent);
        let open = matches!(life, Life::Open(_));
        Ok(life.held().map(|entry| (entry.len, open)))
    }

    /// fxdax: whether an extent is open: written since the last commit, which seals it.
    pub fn has_open(&self) -> Result<bool, Error> {
        Ok(self.state()?.uncommitted.has_open())
    }

    /// The extents the store holds, with their lengths, in order.
    pub fn extents(&self) -> Result<Vec<(ExtentId, u64)>, Error> {
        let state = self.state()?;
        let mut extents: Vec<_> =
            state.extents.iter().into_iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        extents.sort();
        Ok(extents)
    }

    /// The last epoch committed.
    pub fn epoch(&self) -> Result<u64, Error> {
        Ok(self.state()?.epoch)
    }
}

/// The bytes of an extent's data framed at once in its own file ([`Store::write_own`],
/// [`Store::seal_own`]).
const OWN_CHUNK: u64 = 16 * extent_frame::PART;

/// fxdax: copies the first `len` bytes of `from`, a raw file, into `to`, where they are, so many
/// at a time: the parts of an extent framed in place before the first that compressed
/// ([`Store::seal_own`], TASK-310).
fn copy_range<F: File>(from: &F, to: &F, len: u64) -> Result<(), Error> {
    let mut data = Aligned::new(len.min(OWN_CHUNK) as usize);
    for at in (0..len).step_by(OWN_CHUNK as usize) {
        let buf = &mut data.as_mut_slice()[..(len - at).min(OWN_CHUNK) as usize];
        from.read_cached(buf, at)?;
        to.write_at(buf, at)?;
        written::wrote(written::Kind::Own, buf.len());
    }
    Ok(())
}

/// Removes the files of extents stored on their own of `branch`'s in `dir` that neither `extents`
/// holds nor a footer of its write-ahead log names, `named`: what commits that weren't durable
/// wrote, or that were to go once the commits that let them go were; and the raw files of extents
/// open, which no commit made durable refers to, and which are framed once sealed. Its write-ahead
/// log's recovery removed what of segments it holds not ([`crate::wal::recover`]).
fn remove_unheld<D: Dir>(
    dir: &D,
    branch: u64,
    extents: &Extents,
    named: &BTreeSet<ExtentId>,
) -> Result<(), Error> {
    let held = files_of(extents);
    for (kind, own) in [(EXTENTS, true), (OPEN_EXTENTS, false)] {
        for number in list_numbers(dir, &branch_dir(kind, branch))? {
            let extent = ExtentId::new(branch, number);
            if own && (held.contains(&FileKey::Own(extent)) || named.contains(&extent)) {
                continue;
            }
            let path = format!("{}/{:016x}", branch_dir(kind, branch), number);
            dir.remove(&path).with_context(|| format!("removing {path}"))?;
        }
    }
    dir.barrier()?;
    Ok(())
}

// fxdax: commits only ordered are made durable before the store goes, which would otherwise
// lose them though nothing crashed.
impl<D: Dir> Drop for Store<D> {
    fn drop(&mut self) {
        if let Err(err) = self.close() {
            log::error!("extent store: making the last commits durable: {err:#}");
        }
    }
}

/// fxdax: the bytes of `start..end` of an extent that `pieces` hold: what a write there writes
/// over, in segments (TASK-79.2, TASK-160.16).
pub(crate) fn covered(pieces: &[Piece], start: u64, end: u64) -> u64 {
    let spans = paint(pieces, start, end).into_iter();
    spans.filter(|(_, _, source)| source.is_some()).map(|(start, end, _)| end - start).sum()
}

/// A part of an extent: its start and end, and the segment and offset of its data, or `None` where
/// nothing was written.
type Span = (u64, u64, Option<(SegmentId, u64)>);

/// The parts of `start..end` of an extent that `pieces` hold, later pieces over earlier ones.
fn paint(pieces: &[Piece], start: u64, end: u64) -> Vec<Span> {
    let mut parts = if start < end { vec![(start, end, None)] } else { Vec::new() };
    for piece in pieces {
        let (from, to) =
            (piece.extent_offset.max(start), (piece.extent_offset + piece.len).min(end));
        if from >= to {
            continue;
        }
        let mut painted = Vec::with_capacity(parts.len() + 2);
        for (a, b, source) in parts {
            if b <= from || a >= to {
                painted.push((a, b, source));
                continue;
            }
            if a < from {
                painted.push((a, from, source));
            }
            let (s, e) = (a.max(from), b.min(to));
            painted.push((s, e, Some((piece.segment, piece.offset + (s - piece.extent_offset)))));
            if b > to {
                painted.push((to, b, source.map(|(number, at)| (number, at + (to - a)))));
            }
        }
        parts = painted;
    }
    // Neighbours whose data is contiguous are one part.
    let mut merged: Vec<Span> = Vec::with_capacity(parts.len());
    for (start, end, source) in parts {
        if let Some((last_start, last_end, last_source)) = merged.last_mut() {
            let contiguous = match (*last_source, source) {
                (None, None) => true,
                (Some((number, at)), Some(next)) => {
                    next == (number, at + (*last_end - *last_start))
                }
                _ => false,
            };
            if contiguous {
                *last_end = end;
                continue;
            }
        }
        merged.push((start, end, source));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store's own tests under Shuttle (the `shuttle` feature), which only they can run:
    /// `cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::`.
    #[cfg(feature = "shuttle")]
    mod shuttle_tests {
        use super::*;

        /// fxdax: opening, advancing, pinning and dropping a view take none of the locks a commit
        /// holds, so a follower never waits for the writer: under Shuttle, whose waits that can't
        /// end are deadlocks it reports, they complete while this thread holds both.
        #[test]
        fn views_never_wait_for_the_writer() {
            let mut config = shuttle::Config::new();
            config.stack_size = 4 << 20;
            let scheduler = shuttle::scheduler::RandomScheduler::new(1);
            shuttle::Runner::new(scheduler, config).run(|| {
                let description = Description {
                    block_size: 4096,
                    capacity: 1 << 20,
                    branch: extent_remote::layout::test_branch(1),
                };
                let store =
                    Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
                let _committing = store.committing.with(Mutex::lock);
                let _state = store.state.with(Mutex::lock);
                let mut view = store.view();
                view.advance().unwrap();
                view.pin(ExtentId::new(1, 1)).unwrap();
                view.unpin(ExtentId::new(1, 1));
                drop(view);
            });
        }

        /// fxdax: a segment a commit closes while the host refuses threads is framed on the
        /// committing thread, rather than panic, as `thread::spawn` does, through the locks the
        /// commit holds, which every later call would find poisoned: the commit goes on, and its
        /// flush, which syncs on its own thread too, and the store's next commits; and the branch,
        /// opened again, holds what they committed (TASK-79.80). Under Shuttle, the only build in
        /// which a test can have the host refuse threads (`refuse_threads`), on random schedules:
        /// Shuttle's PCT scheduler fails a test that exercises no concurrency, as this one, its
        /// segment framed on the committing thread, does not.
        #[test]
        fn a_host_refusing_threads_frames_on_the_committing_thread() {
            let mut config = shuttle::Config::new();
            config.stack_size = 4 << 20;
            let scheduler = shuttle::scheduler::RandomScheduler::new(10);
            shuttle::Runner::new(scheduler, config).run(|| {
                let dir = crate::MemDir::new();
                let description = Description {
                    block_size: 4096,
                    capacity: 1 << 20,
                    branch: extent_remote::layout::test_branch(1),
                };
                let store = Store::create(dir.clone(), description, Options::default()).unwrap();
                let extent = |sequence| ExtentId::new(1, sequence);
                store.write(extent(0), 0, &[1; 4096]).unwrap();
                store.close_segment().unwrap();
                crate::sync::refuse_threads(true);
                let committed = store.commit(None);
                crate::sync::refuse_threads(false);
                committed.unwrap();
                let framed = dir.list(&branch_dir(crate::layout::SEGMENTS, 1)).unwrap();
                assert_eq!(framed, ["0000000000000000"]);
                store.write(extent(1), 0, &[2; 4096]).unwrap();
                store.commit(None).unwrap();
                drop(store);

                let store = Store::open(dir, Options::default()).unwrap();
                let mut data = [0; 4096];
                for (sequence, byte) in [(0, 1), (1, 2)] {
                    store.read(extent(sequence), 0, &mut data).unwrap();
                    assert_eq!(data, [byte; 4096], "extent {sequence}");
                }
            });
        }
    }

    /// fxdax: what commits sealed is recorded for the views older than them alone: commits no
    /// view follows record none, and a view's records go once it does.
    #[test]
    fn sealed_records_are_for_views() {
        let description = Description {
            block_size: 4096,
            capacity: 1 << 20,
            branch: extent_remote::layout::test_branch(1),
        };
        let store = Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
        // Whether extent `sequence` is recorded as sealed after commit `epoch`.
        let sealed_after = |sequence, epoch| {
            store.state.with(Mutex::lock).sealed.after(ExtentId::new(1, sequence), epoch)
        };
        let commit = |sequence| {
            store.write(ExtentId::new(1, sequence), 0, &[1; 4096]).unwrap();
            store.commit(None).unwrap();
        };
        commit(1);
        commit(2);
        assert!(!sealed_after(2, 1));
        let view = store.view();
        commit(3);
        assert!(sealed_after(3, view.epoch()));
        drop(view);
        commit(4);
        assert!(!sealed_after(3, 2));
        assert!(!sealed_after(4, 3));
    }

    /// fxdax: a count drifted from what it counts, as a change that missed it leaves it, is found
    /// as the state is next let go, by any call: here the bytes its extents hold (TASK-160.16).
    #[test]
    #[should_panic(expected = "the bytes the extents hold")]
    fn audits_find_counts_drifted() {
        let description = Description {
            block_size: 4096,
            capacity: 1 << 20,
            branch: extent_remote::layout::test_branch(1),
        };
        let store = Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
        store.write(ExtentId::new(1, 1), 0, &[1; 4096]).unwrap();
        store.state.with(Mutex::lock).space.added(4096, true);
        store.epoch().unwrap();
    }

    #[test]
    fn painting() {
        let piece = |segment, offset, extent_offset, len| Piece {
            segment: SegmentId { branch: 1, number: segment },
            offset,
            extent_offset,
            len,
        };
        let at = |segment, offset| Some((SegmentId { branch: 1, number: segment }, offset));
        // Nothing written.
        assert_eq!(paint(&[], 0, 8), vec![(0, 8, None)]);
        // A piece in the middle, and one over its end.
        let pieces = [piece(1, 100, 2, 4), piece(2, 200, 5, 2)];
        assert_eq!(
            paint(&pieces, 0, 8),
            vec![(0, 2, None), (2, 5, at(1, 100)), (5, 7, at(2, 200)), (7, 8, None)]
        );
        // A later piece within an earlier one splits it.
        let pieces = [piece(1, 100, 0, 8), piece(2, 200, 3, 2)];
        assert_eq!(
            paint(&pieces, 1, 7),
            vec![(1, 3, at(1, 101)), (3, 5, at(2, 200)), (5, 7, at(1, 105))]
        );
    }
}
