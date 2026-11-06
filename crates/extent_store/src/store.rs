// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store. See the crate's docs.

use crate::dir::{Dir, File, not_found, read_file};
use crate::durability::{Batch, Durability, Syncs};
use crate::framed::{Aligned, Framed, Packed, Part, Source, Unpacked, read_aligned};
use crate::index::{
    Appended, IndexLog, IndexTail, IndexWrite, Recovered, rebuild, recover, write_checkpoint,
};
use crate::kept::{Kept, KeptExtents};
use crate::layout::{
    Change, Checkpoint, DESCRIPTION, Data, Delta, Description, EXTENTS, Extent, FileKey, INDEX,
    OPEN_EXTENTS, OPEN_SEGMENTS, Piece, Place, Record, SEGMENTS, SegmentId, StoreFile, add_piece,
    branch_dir, create_branch_dirs, encode, extent_path, files_of, list_numbers, open_extent_path,
    segment_path,
};
use crate::segments::{Segments, Unframed, close_left_open};
use crate::state::{
    Gate, Locked, OwnFiles, Placement, Reader, Space, State, Uncommitted, Unreferenced,
    uploader_keeps,
};
use crate::sync::{Condvar, Mutex, MutexGuard, rules, spawn_or_return};
use crate::uploads::{Closed, Snapshot, Uploads};
use crate::views::{Limits, Sealed, View, Views};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Chain, Child, Location};
use extent_remote::cache::Cache;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;
use std::time::Duration;
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
#[error("the fork to branch {child:#x} failed once its commit was made")]
pub struct ForkCommitted {
    /// The fork's child.
    pub child: u64,
}

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
    /// The bytes of deltas an index file holds after which a commit starts another, with a
    /// checkpoint.
    pub checkpoint_after: u64,
    /// How many segments' files the store keeps open between reads.
    pub open_files: usize,
    /// fxdax: how long a view ([`Store::view`]) may lag behind the commits after its own before
    /// it is cut off, if it may not lag for ever.
    pub view_lag: Option<Duration>,
    /// fxdax: how many bytes of frees views may keep, past which the oldest is cut off, if any
    /// limit.
    pub view_kept_bytes: Option<u64>,
}

impl Options {
    /// fxdax: the most bytes of data a segment holds: `segment_size`, and a write more, of
    /// `max_extent` at most, before writes go to another. A closed segment is read, framed and
    /// uploaded whole, so that this bounds what each takes of memory, however much is written
    /// between commits (TASK-79.2).
    pub(crate) fn segment_limit(&self) -> u64 {
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
            checkpoint_after: 16 << 20,
            open_files: 64,
            view_lag: None,
            view_kept_bytes: None,
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
    /// One more than the highest sequence number of the store's branch's extents, freed or not.
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

/// An extent store, open. See the crate's docs.
pub struct Store<D: Dir> {
    /// Its directory, which its framings share.
    dir: Arc<D>,
    block_size: u32,
    branch: u64,
    /// fxdax: the URL of the bucket its branch is kept in, if its description names one.
    bucket: Option<String>,
    /// Its directory, taken for this process ([`Dir::take`]), and claimed for its opening
    /// ([`Dir::claim`]): closing it lets go of both, but of the taking what holds it still
    /// ([`Store::hold_dir`]).
    opened: Mutex<Option<(Claim<D>, Arc<Taken<D>>)>>,
    /// The parts of its files read lately, unpacked.
    unpacked: Unpacked,
    options: Options,
    /// fxdax: shared with the framings of its segments, each of which ends itself in it, on a
    /// thread of its own (TASK-153.8).
    state: Arc<Mutex<State<D::File>>>,
    /// Held by a commit.
    committing: Mutex<()>,
    /// When its commits are durable: those ordered that no flush has taken yet, oldest first, for
    /// [`Store::flush`] to make durable, and the last that is.
    durability: Mutex<Durability<Pending<D::File>>>,
    /// Held by a flush: commits become durable in order. fxdax: and where their records go in the
    /// index, which each flush writes after the last's, a checkpoint made and encoded only then,
    /// holding no other lock (TASK-92.56).
    flushing: Mutex<IndexTail<D::File>>,
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
    /// Its record in the index.
    write: IndexWrite,
    /// The files of the branch's alone it made unreferenced ([`Unreferenced::doomed`]): they go
    /// once it is durable.
    doomed: Vec<String>,
    /// fxdax: the files of the segments acknowledged in the bucket it let go of, with their keys
    /// there: they leave for the host's cache once it is durable (TASK-165.8).
    leaving: Vec<(String, String)>,
    /// fxdax: the segment it closed, if any, being framed: it is durable once that is, which its
    /// flush waits for, its data being in it (TASK-153.8).
    closed: Option<SegmentId>,
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
        self.store.finish_write(&self);
    }
}

impl<D: Dir> Drop for Write<'_, D> {
    fn drop(&mut self) {
        if !self.finished {
            self.store.abandon_write(self);
        }
    }
}

/// A branch's state, rebuilt from its newest checkpoint and the footers of its closed segments
/// after it ([`Store::rebuild`]): as of its last commit a closed segment holds, if one does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebuilt {
    pub epoch: u64,
    pub root: Vec<u8>,
    pub next_sequence: u64,
    /// The extents, with their lengths and where they are, in order.
    pub extents: Vec<(ExtentId, u64, Place)>,
    /// fxdax: the sequence numbers of its own extents it freed once sealed, which it writes no
    /// more ([`Store::spent`], TASK-160.15).
    pub spent: extent_frame::spent::Spent,
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
/// `state`: the segment is read from its framed file from now on, its raw file going once the
/// commits its footer names are durable; or its framing failed, and the store with it, as it is
/// ahead of its files.
fn framing_ended<F: File>(
    state: &mut State<F>,
    number: SegmentId,
    framed: Result<Arc<Framed<F>>, Error>,
) {
    match framed {
        Ok(framed) => state.segments.framed(number, framed),
        Err(error) => {
            log::error!("extent store: framing segment {number:?}: {error:#}");
            state.failed = true;
            state.segments.framing_failed(number, format!("{error:#}"));
        }
    }
}

impl<D: Dir> Store<D> {
    /// Makes a new, empty store in `dir`, which holds no store, as `description` describes, and
    /// opens it.
    pub fn create(dir: D, description: Description, options: Options) -> Result<Self, Error> {
        Self::create_kept(dir, description, None, options)
    }

    /// fxdax: makes a new store as [`Store::create`] does, whose description names `bucket`, the
    /// URL of the bucket its branch is kept in, if given ([`Store::bucket`]). The description is
    /// made last: until it is durable, the directory is no store.
    pub fn create_kept(
        dir: D,
        description: Description,
        bucket: Option<&str>,
        options: Options,
    ) -> Result<Self, Error> {
        let Description { block_size, capacity, branch } = description;
        ensure!(
            block_size.is_power_of_two() && block_size as usize <= crate::framed::ALIGNMENT,
            "bad block size {block_size}"
        );
        ensure!(capacity >= block_size as u64, "the store is smaller than a block");
        dir.take().context("taking the store's directory")?;
        let made = || -> Result<(), Error> {
            for path in [SEGMENTS, EXTENTS, INDEX, OPEN_SEGMENTS, OPEN_EXTENTS] {
                dir.create_dir(path).with_context(|| format!("creating {path}"))?;
            }
            create_branch_dirs(&dir, branch)?;
            write_checkpoint(&dir, branch, 0, &Recovered { capacity, ..Default::default() })?;
            let file = dir.create(DESCRIPTION, false)?;
            let bucket = bucket.map(str::to_owned);
            file.write_at(StoreFile { block_size, branch, bucket }.text().as_bytes(), 0)?;
            file.sync()?;
            dir.sync_dir("")?;
            dir.barrier()?;
            Ok(())
        };
        let made = made();
        dir.let_go();
        made?;
        Self::open(dir, options)
    }

    /// Opens the store in `dir`, recovering it as it was at its last commit that is durable, and
    /// removes the files of extents' data no extent is in: what commits that weren't durable
    /// wrote, or that were to go once the commits that let them go were. It fails while another
    /// process has it open ([`Dir::take`]), or this one ([`Dir::claim`]).
    pub fn open(dir: D, options: Options) -> Result<Self, Error> {
        dir.take().context("taking the store's directory")?;
        let dir = Arc::new(dir);
        // Dropped, as an opening that fails drops them, they let the directory go.
        let taken = Arc::new(Taken(dir.clone()));
        dir.claim().context("taking the store's directory")?;
        let claim = Claim(dir.clone());
        let StoreFile { block_size, branch, bucket } = {
            let file = dir.open(DESCRIPTION, false).context("opening the store's description")?;
            let text = String::from_utf8(read_file(&file)?).context("reading the description")?;
            StoreFile::parse(&text)?
        };
        let recovered = recover(&*dir, branch)?;
        remove_unheld(&*dir, branch, &recovered)?;
        let Recovered { epoch, next_sequence, root, capacity, extents, spent, .. } = recovered;
        // Footers of commits not recovered, which were not durable, are no footers: the commits
        // after this epoch are others.
        close_left_open(&*dir, branch, epoch)?;

        // The segments it holds extents' data in, with the bytes of it each holds, and its own
        // extents' files. They must be there.
        let mut tracked: BTreeMap<SegmentId, u64> = BTreeMap::new();
        let (mut stored, mut own_stored) = (0, 0);
        let mut own_sizes = HashMap::new();
        for (extent, entry) in &extents {
            stored += entry.len;
            if extent.branch == branch {
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
        let mut segments = Segments::open(&*dir, branch, tracked, epoch, options.open_files)?;
        // fxdax: the sealed extents in each, read in the bucket once it is acknowledged there
        // (TASK-165.8).
        for (extent, entry) in &extents {
            segments.place(*extent, &entry.data);
        }
        let open_segment = segments.open_segment();

        // A new index file, with a checkpoint of what was recovered, which replaces the others.
        let segment = open_segment.number;
        let state = Recovered { epoch, next_sequence, root, segment, capacity, extents, spent };
        let (index, tail) = IndexLog::open(&*dir, branch, &state)?;
        let Recovered { root, extents, spent, .. } = state;

        let views = Views::new(epoch, &root);
        Ok(Self {
            dir,
            block_size,
            branch,
            bucket,
            opened: Mutex::new(Some((claim, taken))),
            unpacked: Unpacked::default(),
            options,
            state: Arc::new(Mutex::new(State {
                extents,
                space: Space::new(capacity, stored, own_stored),
                epoch,
                root,
                next_sequence,
                spent,
                uncommitted: Uncommitted::default(),
                gate: Gate::default(),
                segments,
                own_files: OwnFiles::new(branch, own_sizes, options.open_files),
                index,
                unreferenced: Unreferenced::default(),
                uploads: None,
                kept: KeptExtents::default(),
                sealed: Sealed::default(),
                failed: false,
                closed: false,
                deferred: Vec::new(),
            })),
            committing: Mutex::new(()),
            durability: Mutex::new(Durability::new(epoch)),
            flushing: Mutex::new(tail),
            idle: Arc::new(Condvar::new()),
            views,
        })
    }

    /// The branch's state as its newest checkpoint and the footers of its closed segments after it
    /// say: as of the last commit a closed segment holds, if one does (doc-7: a branch's history is
    /// in its segments).
    pub fn rebuild(&self) -> Result<Rebuilt, Error> {
        let state = rebuild(&*self.dir, self.branch)?;
        let mut extents: Vec<_> = state
            .extents
            .iter()
            .map(|(extent, entry)| (*extent, entry.len, place_of(entry)))
            .collect();
        extents.sort_by_key(|(extent, ..)| *extent);
        Ok(Rebuilt {
            epoch: state.epoch,
            root: state.root,
            next_sequence: state.next_sequence,
            extents,
            spent: state.spent,
        })
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

    /// fxdax: holds the store's directory taken for this process until what it returns is dropped,
    /// as the store open does: for what outlives the store's closing and still writes in the
    /// directory, as its uploader does as it stops (TASK-129). Fails once it is closed.
    pub fn hold_dir(&self) -> Result<DirHold<D>, Error> {
        let opened = self.opened.lock().unwrap();
        let (_, taken) =
            opened.as_ref().ok_or_else(|| anyhow!("branch {:#x} is closed", self.branch))?;
        Ok(DirHold { _taken: taken.clone() })
    }

    /// fxdax: commits, durably, a commit whose segment's footer names `child`, a fork of the
    /// branch, which its owner and whether it is a point say (doc-7, Forks): the fork is made where
    /// the branch's segments go, as they go, and nowhere here (TASK-165.7). Its segment closes at
    /// the commit. Returns the commit's epoch. One that fails once its commit was made fails with
    /// [`ForkCommitted`]: the segment names the fork, as a host crashing then would leave it.
    pub fn commit_fork(&self, child: Child) -> Result<u64, Error> {
        let committing = self.committing.lock().unwrap();
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
        Locked::new(self.state.lock().unwrap())
    }

    fn state(&self) -> Result<Locked<'_, D::File>, Error> {
        let state = self.lock();
        ensure!(!state.failed, "the extent store failed");
        Ok(state)
    }

    /// The state, to change: unless the branch is closed.
    fn writable(&self) -> Result<Locked<'_, D::File>, Error> {
        self.still_writable(self.lock())
    }

    /// `state`, to change, as found again after waiting, its lock let go of meanwhile: unless the
    /// store failed, or the branch closed, since (TASK-83).
    fn still_writable<'a>(&self, state: Locked<'a, D::File>) -> Result<Locked<'a, D::File>, Error> {
        ensure!(!state.failed, "the extent store failed");
        ensure!(!state.closed, "branch {:#x} is closed", self.branch);
        Ok(state)
    }

    /// Closes the branch, once the commits only ordered are durable, those a flush in flight is
    /// making durable among them: another store may open it, and this one reads what it holds, as
    /// caches of its extents may still, and changes nothing.
    pub fn close(&self) -> Result<(), Error> {
        // After a flush in flight: its commits are durable, and the files they made unreferenced
        // gone, before another store may open the branch.
        let mut flushing = rules().close_waits_for_flushes.then(|| self.flushing.lock().unwrap());
        let flushed = match &mut flushing {
            _ if !self.flush_pending() => Ok(()),
            Some(flushing) => self.flush_locked(flushing),
            None => self.flush(),
        };
        drop(flushing);
        let mut state = self.lock();
        if !state.closed {
            // fxdax: once no segment is being framed, whatever closed it, a commit no flush took
            // or one that failed among them, which another store, opening the branch, would frame
            // again meanwhile from its raw file (TASK-41.1, TASK-79.99): each framing ends itself,
            // and the segments say which are running (TASK-153.8). The opening keeps their framed
            // files, if whole, or frames their raw files anew.
            if rules().close_waits_for_framings {
                state = self.framed(state);
            }
            state.closed = true;
            // Their raw files, of commits durable, go as a flush's would.
            let raw = match state.failed {
                false => state.segments.raw(self.durability.lock().unwrap().durable()),
                true => Vec::new(),
            };
            drop(state);
            self.remove(raw);
            drop(self.opened.lock().unwrap().take());
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
        if extent.branch != self.branch {
            bail!(OtherBranch(extent));
        }
        let mut state = self.writable()?;
        // Not while a commit, or a closing between commits, waits for the writes in progress.
        while state.gate.is_held() {
            state = state.wait(&self.idle);
        }
        let state = self.still_writable(state)?;
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
        if extent.branch != self.branch {
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

    /// fxdax: fails unless `extent`, the branch's, may be opened, as a write opens it, by `state`:
    /// unless it is open, or never sealed by the branch, held or freed ([`Spent`], TASK-79.6).
    fn check_opens(state: &State<D::File>, extent: ExtentId) -> Result<(), Error> {
        if state.uncommitted.is_open(extent) {
            return Ok(());
        }
        if state.extents.get(&extent).is_some() {
            bail!(SealedExtent(extent));
        }
        if state.spent.contains(extent.sequence) {
            bail!(Spent(extent));
        }
        Ok(())
    }

    /// Begins a write, as [`Store::begin_write`] says, with `state`, writable, held, and no commit
    /// holding writes back.
    fn begin_write_locked(
        &self,
        mut state: Locked<'_, D::File>,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> Result<Write<'_, D>, Error> {
        let end = offset + len;
        let current = state.extents.get(&extent).map(|entry| entry.len);
        Self::check_opens(&state, extent)?;
        let grows = end.saturating_sub(current.unwrap_or(0));
        let to_segment = match state.uncommitted.placement(extent) {
            Some(placement) => matches!(placement, Placement::Segment),
            None => end < self.options.large_extent,
        };
        // fxdax: what it writes over, in a segment, takes room too (TASK-79.2), as the writes
        // that finished left the extent: not its holes (TASK-160.16).
        let overwrites = match state.extents.get(&extent).map(|entry| &entry.data) {
            Some(Data::Pieces(pieces)) if to_segment => covered(pieces, offset, end),
            _ => 0,
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
    /// go to the next (TASK-79.2). The open segment, full, is set aside, holding no commit's data,
    /// or else closed, as between commits, and framed behind, a framing that ends itself, whether
    /// or not a commit comes after ([`Store::frame_behind`], TASK-150.2): holding the state alone,
    /// so that the write waits neither for a commit in progress, nor for the writes in the
    /// segment, nor for the framing, as the uploader's closing between commits does
    /// ([`Store::close_now`]). Those writes go on: their extents are open, which the next commit
    /// copies out, from the segment's raw file, which reads read until it is framed.
    fn make_room(&self, state: &mut State<D::File>, len: u64) -> Result<(), Error> {
        if state.segments.fits(len, self.options.segment_limit()) {
            return Ok(());
        }
        let full = state.segments.open_segment();
        if state.segments.has_commits(state.epoch) {
            let unframed = self.close_open_segment(state, state.epoch)?;
            match rules().framings_end_themselves {
                true => self.frame_behind(state, unframed),
                false => state.deferred.push(unframed),
            }
        } else {
            state.segments.retire(&*self.dir)?;
            state.index.set_aside(full.number);
        }
        Ok(())
    }

    fn finish_write(&self, write: &Write<'_, D>) {
        let mut state = self.lock();
        let state = &mut *state;
        // A write to an extent freed since it began is dropped, as though it finished before the
        // free: the extent, if written again since, is another opening's, whose data may go
        // elsewhere, and which its bytes are no part of (TASK-79.1). A commit waits for the writes
        // in progress before it seals what they write to: a free, not a seal, ended the opening.
        if rules().drop_writes_to_freed_extents
            && !state.uncommitted.is_open_in(write.extent, write.opening)
        {
            return self.drop_write(state, write);
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
        if placement.is_none() && extent.branch == self.branch {
            state.spent.insert(extent.sequence);
        }
        state.space.removed(entry.len, extent.branch == self.branch);
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
            let uploader = state.uploads.is_some() && rules().keep_frees;
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

    /// fxdax: holds `extent`, sealed, whose data is in the bucket, at `location`: reads of it fail
    /// with [`RemoteData`], and the store's user reads it from there. A branch kept in object
    /// storage, opened where its data is not, holds its extents so.
    pub fn adopt(&self, extent: ExtentId, location: Location) -> Result<(), Error> {
        let mut state = self.writable()?;
        ensure!(!state.extents.contains_key(&extent), "the store holds {extent:?}");
        let len = location.len;
        state.extents.insert(extent, Extent { len, data: Data::Remote(location.clone()) });
        state.space.added(len, extent.branch == self.branch);
        if extent.branch == self.branch {
            state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        }
        state.uncommitted.push(Change::Adopt { extent, location });
        // Footers name no extent whose data is elsewhere: a checkpoint does.
        state.index.want_checkpoint();
        Ok(())
    }

    /// fxdax: spends what `spent` says of the store's branch, as its commits would have: its
    /// extents freed where else it is kept, which a store taking it there knows of by this alone,
    /// and writes no more (TASK-150.6). Its next commit keeps it, in a checkpoint.
    pub fn spend(&self, spent: &extent_frame::spent::Spent) -> Result<(), Error> {
        let mut state = self.writable()?;
        for (start, end) in spent.runs() {
            state.spent.insert_run(start, end);
        }
        state.index.want_checkpoint();
        Ok(())
    }

    /// fxdax: what the store holds as of now.
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let state = self.state()?;
        let mut extents: Vec<_> =
            state.extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        extents.sort();
        let (root, capacity) = (state.root.clone(), state.space.capacity());
        Ok(Snapshot { epoch: state.epoch, extents, root, capacity })
    }

    /// fxdax: from now on, keeps for an uploader the segments it closes, until acknowledged
    /// ([`Store::closed`], [`Store::acknowledged`]), their footers, stamped by `chain`, the chain of
    /// the uploader's writer of the branch, from where it is, saying what each commit changed; and
    /// the data of the extents its commits free, and the bytes they write
    /// ([`Store::unreleased_bytes`]), until released ([`Store::release`]): unless a commit came
    /// since `epoch`, the snapshot's the uploader starts from. Returns whether it does. What is
    /// acknowledged goes into `cache`, the host's cache of the bucket's objects, if given, and
    /// leaves the store (TASK-165.8).
    pub fn upload(
        &self,
        chain: Chain,
        epoch: u64,
        cache: Option<Arc<Cache>>,
    ) -> Result<bool, Error> {
        ensure!(
            chain.branch() == self.branch,
            "branch {:#x} uploaded by a writer of branch {:#x}",
            self.branch,
            chain.branch()
        );
        let mut state = self.state()?;
        if state.epoch != epoch {
            return Ok(false);
        }
        state.uploads.get_or_insert_with(Uploads::default).cache = cache;
        state.segments.set_chain(chain);
        Ok(true)
    }

    /// fxdax: lets the data of the extents the commits up to `epoch` freed go: the uploader has
    /// what it needs of them, having taken ([`Store::closed`]) the segments holding those commits,
    /// whose footers may name those extents' own files. Their files go with the commit after.
    pub fn release(&self, epoch: u64) -> Result<(), Error> {
        let mut state = self.state()?;
        let state = &mut *state;
        let Some(uploads) = &mut state.uploads else { return Ok(()) };
        uploads.release(epoch);
        state.release_kept(epoch);
        state.trim_kept(&self.views.keeping(), state.epoch);
        Ok(())
    }

    /// fxdax: the bytes the commits not yet released wrote: what the uploader has yet to upload.
    pub fn unreleased_bytes(&self) -> Result<u64, Error> {
        let state = self.state()?;
        Ok(state.uploads.as_ref().map_or(0, Uploads::unreleased_bytes))
    }

    /// fxdax: reads `buf.len()` bytes at `offset` in `extent`, held, or freed and kept for the
    /// uploader.
    pub fn read_kept(&self, extent: ExtentId, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.read_into_entry(extent, offset, buf, Reader::Uploader)
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
        let committing = self.committing.lock().unwrap();
        self.commit_locked(&committing, root)
    }

    /// Commits, as [`Store::commit_ordered`] does, holding `_committing`.
    fn commit_locked(
        &self,
        _committing: &MutexGuard<'_, ()>,
        root: Option<&[u8]>,
    ) -> Result<(), Error> {
        let mut state = self.writable()?;
        // It includes the writes in progress, which may be to extents it seals: what they write
        // must be in them, not after. New ones wait for it.
        if rules().commit_waits_for_writes {
            state = self.hold_writes(state, |state| state.gate.is_writing());
            // fxdax: as before it waited, which let the state go meanwhile (TASK-83).
            if rules().recheck_after_waiting {
                state = self.still_writable(state)?;
            }
        }
        // From its first seal on, the state runs ahead of the files: a commit failing part way
        // fails the store.
        let committed = state.failing(|state| self.commit_in(state, root));
        committed.context("committing the extent store")
    }

    /// The steps of the next commit, which sets `root`, if any, in their order: each takes what
    /// those before it leave.
    fn commit_in(&self, state: &mut State<D::File>, root: Option<&[u8]>) -> Result<(), Error> {
        let epoch = state.epoch + 1;
        let sealing = self.seal_open(state)?;
        self.publish(state, epoch, root, &sealing);
        let (changes, capacity) = (state.uncommitted.take(), state.uncommitted.take_capacity());
        if let Some(uploads) = &mut state.uploads {
            uploads.record(epoch, &changes, &state.extents);
        }
        // The open segment's footer records them, before the segment closes.
        state.segments.record(&changes, root, capacity, &state.extents);
        // fxdax: with the rule off, the closings for room since the last commit are framed only
        // now, by a commit that may never come (TASK-150.2).
        for unframed in std::mem::take(&mut state.deferred) {
            self.frame_behind(state, unframed);
        }
        let closed = self.close_if_due(state, epoch)?;
        // What its flush syncs, after the closing, whose directories its flush must sync: the
        // framed file's entry is durable before its raw file goes.
        let mut syncs = Syncs::default();
        state.own_files.take_syncs(&mut syncs);
        state.segments.take_syncs(&mut syncs);
        // The segments none of its extents' data is in any more, by this commit, if not before:
        // the branch lets them go; those it closed, once a checkpoint covers their footers. fxdax:
        // but those the uploader has yet to upload (TASK-85).
        let uploads = &state.uploads;
        let reaped = state.segments.reap(|segment| uploader_keeps(uploads, self.branch, segment));
        // fxdax: those acknowledged in the bucket leave, their footers the bucket's (TASK-165.8).
        let (acknowledged, reaped): (Vec<_>, Vec<_>) =
            reaped.into_iter().partition(|reaped| reaped.acknowledged.is_some());
        let leaving = (acknowledged.into_iter())
            .map(|reaped| (segment_path(reaped.number), reaped.acknowledged.unwrap()))
            .collect();
        let released = state.index.let_go(reaped);
        state.unreferenced.doomed.extend(released.iter().flat_map(FileKey::paths));
        let appended = self.append_record(state, epoch, root, capacity, changes);
        state.unreferenced.doomed.extend(appended.replaced);
        state.unreferenced.doomed.extend(appended.covered.iter().flat_map(FileKey::paths));
        let Unreferenced { doomed } = state.unreferenced.take();
        state.epoch = epoch;
        if let Some(root) = root {
            state.root = root.to_vec();
        }
        // Queued while the state is locked, so that commits queue in their order.
        let pending = Pending { syncs, write: appended.write, doomed, leaving, closed };
        self.durability.lock().unwrap().push(epoch, pending);
        Ok(())
    }

    /// Holds new writes back ([`Store::begin_write`]) while the writes in progress are `busy`, as
    /// a commit, or a closing between commits, waits for them, and returns the state once they
    /// are not: which its caller finds again, as it let it go meanwhile.
    fn hold_writes<'a>(
        &self,
        mut state: Locked<'a, D::File>,
        busy: impl Fn(&State<D::File>) -> bool,
    ) -> Locked<'a, D::File> {
        state.gate.hold();
        while busy(&state) {
            state = state.wait(&self.idle);
        }
        state.gate.release();
        self.idle.notify_all();
        state
    }

    /// Seals the extents open, as a commit does, each in one run, or a file of its own if large
    /// (doc-7, Segments), and returns them.
    fn seal_open(&self, state: &mut State<D::File>) -> Result<Vec<ExtentId>, Error> {
        let sealing = state.uncommitted.open_extents();
        for &extent in &sealing {
            self.seal(state, extent)?;
        }
        Ok(sealing)
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
        let keeping = if rules().decide_at_publish {
            let kept = &mut state.kept;
            let keeping =
                self.views.publish(epoch, &published, limits, |keeping| kept.views_bytes(keeping));
            state.trim_kept(&keeping, epoch);
            keeping
        } else {
            state.trim_kept(&self.views.keeping(), epoch);
            let kept = &mut state.kept;
            self.views.publish(epoch, &published, limits, |keeping| kept.views_bytes(keeping))
        };
        // What it sealed, as views older than it, if any lives, must refuse; what all of them
        // were sealed by is theirs.
        state.sealed.published(keeping.oldest(), sealing, epoch);
    }

    /// Closes the open segment after commit `epoch`, once no write is in progress in it, if it is
    /// due: full, old, wanted closed, or at a fork; writes go to a new one. fxdax: it is framed
    /// off the commit ([`Store::frame_behind`]), which its flush waits for. Returns it.
    fn close_if_due(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
    ) -> Result<Option<SegmentId>, Error> {
        if !state.segments.due(&self.options) {
            return Ok(None);
        }
        let unframed = self.close_open_segment(state, epoch)?;
        let number = unframed.number();
        self.frame_behind(state, unframed);
        Ok(Some(number))
    }

    /// Appends commit `epoch`'s record, of `changes` and `root`, if it sets one, to the index:
    /// its delta, or, if one is due, a checkpoint of the branch as of it, in a new file, which
    /// replaces the last once the commit is durable. fxdax: what the checkpoint holds is taken
    /// here, holding the state; its flush makes its file and encodes it there, holding the flushes'
    /// lock alone, so that writes wait for neither, as they did for 89-143 ms of a load's commits
    /// (TASK-92.56).
    fn append_record(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        root: Option<&[u8]>,
        capacity: Option<u64>,
        changes: Vec<Change>,
    ) -> Appended {
        let next_sequence = state.next_sequence;
        let root_set = root.map(<[u8]>::to_vec);
        let delta = Delta { epoch, next_sequence, root: root_set, capacity, changes };
        let checkpoint = || {
            let (root, segment) = (root.unwrap_or(&state.root), state.segments.open_segment());
            let rest = (root, segment.number, state.space.capacity());
            Checkpoint::of(epoch, next_sequence, rest, &state.extents, (self.branch, &state.spent))
        };
        state.index.append(delta, checkpoint, &self.options)
    }

    /// Seals `extent`, which a commit seals, in one run in the open segment, or in a file of its
    /// own, framed, if it is large, copying its data there unless it is. fxdax: or if the open
    /// segment has no room for it, as when the extents written since the last commit filled
    /// others (TASK-79.2).
    fn seal(&self, state: &mut State<D::File>, extent: ExtentId) -> Result<(), Error> {
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
            return Ok(());
        };
        let large = entry.len >= self.options.large_extent;
        let open = state.segments.open_segment();
        match (&entry.data, state.uncommitted.placement(extent)) {
            (Data::Remote(_), _) => return Ok(()),
            // Written to a raw file of its own, which it is framed from.
            (Data::Own, Some(Placement::Own(raw))) => {
                let raw = raw.clone();
                self.write_own(state, extent, entry.len, |at, buf| raw.read_cached(buf, at))?;
                state.unreferenced.doomed.push(open_extent_path(extent));
                return Ok(());
            }
            (Data::Pieces(pieces), _) if !large => {
                let one_run = matches!(pieces[..], [piece] if piece.segment == open
                    && piece.extent_offset == 0
                    && piece.len == entry.len);
                if one_run {
                    state.segments.place(extent, &entry.data);
                    return Ok(());
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
        let data_now = if large || !state.segments.fits(entry.len, self.options.segment_limit()) {
            let data = data.as_slice();
            self.write_own(state, extent, entry.len, |at, buf| {
                buf.copy_from_slice(&data[at as usize..at as usize + buf.len()]);
                Ok(())
            })?;
            state.uncommitted.push(Change::MoveOwn { extent });
            Data::Own
        } else {
            let (file, at) = state.segments.append(entry.len);
            file.write_at(data.as_slice(), at)?;
            let pieces =
                vec![Piece { segment: open, offset: at, extent_offset: 0, len: entry.len }];
            state.uncommitted.push(Change::Move { extent, pieces: pieces.clone() });
            Data::Pieces(pieces)
        };
        state.segments.place(extent, &data_now);
        state.extents.get_mut(&extent).unwrap().data = data_now;
        Ok(())
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
        /// The bytes framed at once.
        const CHUNK: u64 = 16 * extent_frame::PART;
        let path = extent_path(extent);
        let file = self.dir.create(&path, true).with_context(|| format!("creating {path}"))?;
        let mut packer = extent_frame::Packer::new();
        let mut data = Aligned::new(len.min(CHUNK) as usize);
        for at in (0..len).step_by(CHUNK as usize) {
            let buf = &mut data.as_mut_slice()[..(len - at).min(CHUNK) as usize];
            read(at, buf)?;
            let end = packer.end();
            file.write_at(&packer.push(buf), end)?;
        }
        let end = packer.end();
        let tail = packer.finish(&[]);
        file.write_at(&tail, end)?;
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
        if let Some(uploads) = &mut state.uploads {
            uploads.closed_segment(unframed.number().number, unframed.footer.clone());
        }
        Ok(unframed)
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
        let Some(number) = self.close_between_commits()? else { return Ok(false) };
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
    /// closed.
    fn close_between_commits(&self) -> Result<Option<SegmentId>, Error> {
        let _committing = self.committing.lock().unwrap();
        let state = self.state()?;
        // Closed, it changes nothing: its last segment closes when it is next opened.
        if state.closed || !state.segments.has_commits(state.epoch) {
            return Ok(None);
        }
        let mut state = self.hold_writes(state, |state| {
            rules().close_waits_for_writers && state.segments.open_writers() > 0
        });
        // fxdax: as before it waited, which let the state go meanwhile (TASK-83).
        if rules().recheck_after_waiting {
            ensure!(!state.failed, "the extent store failed");
            if state.closed {
                return Ok(None);
            }
        }
        let epoch = state.epoch;
        let unframed = state.failing(|state| self.close_open_segment(state, epoch));
        let unframed = unframed.context("closing the open segment")?;
        let number = unframed.number();
        self.frame_behind(&mut state, unframed);
        Ok(Some(number))
    }

    /// fxdax: frames `unframed`, a segment just closed, which `state` holds, off what closed it:
    /// a commit, a write finding no room, a closing between commits or a fork's commit, alike. Its
    /// framing makes it durable ([`Unframed::frame`]), then ends itself, holding the state: the
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
    fn frame_behind(&self, state: &mut State<D::File>, unframed: Unframed<D::File>) {
        let number = unframed.number();
        state.segments.framing_begun(number);
        let unframed = match self.dir.concurrent() {
            true => {
                let (dir, shared, idle) = (self.dir.clone(), self.state.clone(), self.idle.clone());
                spawn_or_return(unframed, move |unframed| {
                    let framed = unframed.frame(&*dir);
                    framing_ended(&mut Locked::new(shared.lock().unwrap()), number, framed);
                    idle.notify_all();
                })
            }
            false => Some(unframed),
        };
        if let Some(unframed) = unframed {
            framing_ended(state, number, unframed.frame(&*self.dir));
            self.idle.notify_all();
        }
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
        let ours = framing.filter(|segment| segment.branch == self.branch);
        ours.map(|segment| segment.number).collect()
    }

    /// fxdax: `state`, once no segment is being framed ([`Store::frame_behind`]), which it let go
    /// of meanwhile.
    fn framed<'a>(&self, mut state: Locked<'a, D::File>) -> Locked<'a, D::File> {
        while !state.segments.framings().is_empty() {
            state = state.wait(&self.idle);
        }
        state
    }

    /// fxdax: the segments closed and not yet taken whose commits are durable, oldest first,
    /// `limit` of them at most, for the uploader, which has them until it says they are uploaded
    /// ([`Store::uploaded`]): the rest it takes later, their files not opened meanwhile. fxdax:
    /// those being framed among them it waits for, holding no lock: each framing ends itself,
    /// whatever comes next, which may be nothing, as after a write that closed one for want of room
    /// (TASK-150.2, TASK-153.8).
    pub fn closed(&self, limit: usize) -> Result<Vec<Closed<D::File>>, Error> {
        let framing = self.state()?.segments.framing_through(self.uploadable());
        self.wait_for_framings(framing)?;
        let mut state = self.state()?;
        let state = &mut *state;
        let Some(uploads) = &mut state.uploads else { return Ok(Vec::new()) };
        // fxdax: and framed: its framed file is what is uploaded.
        let segments = &state.segments;
        let framing = |number| segments.is_framing(SegmentId { branch: self.branch, number });
        let taken = uploads.take_closed(self.uploadable(), framing, limit);
        let mut closed = Vec::with_capacity(taken.len());
        for (number, footer) in taken {
            let segment = SegmentId { branch: self.branch, number };
            let file = self.segment_file(state, segment)?.file().clone();
            let mut own = Vec::with_capacity(footer.contents.own.len());
            for (extent, location) in footer.own() {
                let file = state.own_files.file(&*self.dir, extent)?;
                own.push((location.key, file.file.clone()));
            }
            closed.push(Closed { number, footer, file, own });
        }
        Ok(closed)
    }

    /// fxdax: segment `number`, which [`Store::closed`] gave, is acknowledged in the bucket, at
    /// `key`, its object there: the extents sealed in it, and those stored on their own its footer
    /// names, are read there from now on ([`RemoteData`]), and their files leave the store for the
    /// host's cache, or go: the extents' own files, and the segment's, once no extent's data is in
    /// it here (TASK-165.8). Durably first: the index records it after the commits durable so far,
    /// which it makes durable first, so that the store opened again finds them in the bucket.
    pub fn acknowledged(&self, number: u64, key: &str) -> Result<(), Error> {
        let mut tail = self.flushing.lock().unwrap();
        self.flush_locked(&mut tail)?;
        let (record, leaving) = {
            let mut state = self.state()?;
            let state = &mut *state;
            let Some(uploads) = &mut state.uploads else { return Ok(()) };
            let own = uploads.acknowledged(number);
            let segment = SegmentId { branch: self.branch, number };
            let mut leaving = state.acknowledge(segment, key, &own);
            leaving.extend(state.segments.reap_acknowledged());
            (Record::Acknowledged { segment: number, key: key.to_owned(), own }, leaving)
        };
        let record = IndexWrite::Delta(encode(&record));
        let written = tail.write(&*self.dir, [&record]).and_then(|()| Ok(self.dir.barrier()?));
        if written.is_err() {
            self.lock().failed = true;
        }
        written.context("recording an acknowledgement")?;
        drop(tail);
        self.leave(leaving);
        Ok(())
    }

    /// fxdax: the files at the paths of `leaving` leave the store, the objects at their keys in
    /// the bucket: into the host's cache, if the uploader gave one and it takes them, the file
    /// renamed; or removed (TASK-165.8). What fails to leave is logged, and removed, the bucket
    /// holding it.
    fn leave(&self, leaving: Vec<(String, String)>) {
        if leaving.is_empty() {
            return;
        }
        let cache = self.lock().uploads.as_ref().and_then(|uploads| uploads.cache.clone());
        for (path, key) in leaving {
            let adopted = match (&cache, self.dir.host_path(&path)) {
                (Some(cache), Some(from)) => cache.adopt(&key, &from),
                _ => Ok(false),
            };
            match adopted {
                Ok(true) => continue,
                Ok(false) => {}
                Err(err) => log::warn!("extent store: {path} into the cache, as {key}: {err}"),
            }
            self.remove([path]);
        }
    }

    /// fxdax: makes the commits ordered so far durable, in order, as few syncs as they need
    /// together: their data, then their records in the index, then the files they made
    /// unreferenced go; and the raw files of the segments framed whose commits are durable.
    pub fn flush(&self) -> Result<(), Error> {
        self.flush_locked(&mut self.flushing.lock().unwrap())
    }

    /// [`Store::flush`], holding `flushing`, and so `tail`, where the index's next record goes.
    fn flush_locked(&self, tail: &mut IndexTail<D::File>) -> Result<(), Error> {
        // A store that failed makes nothing more durable.
        drop(self.state()?);
        let batch = self.durability.lock().unwrap().take();
        let durable = match batch {
            None => self.durability.lock().unwrap().durable(),
            Some(Batch { last, mut commits }) => {
                if let Err(error) = self.make_durable(tail, &mut commits) {
                    self.lock().failed = true;
                    return Err(error.context("committing the extent store"));
                }
                self.durability.lock().unwrap().finish(last);
                // The files the commits let go of go, once they are durable; fxdax: those of the
                // bucket's, into the host's cache (TASK-165.8).
                let mut leaving = Vec::new();
                for pending in commits {
                    self.remove(pending.doomed);
                    leaving.extend(pending.leaving);
                }
                self.leave(leaving);
                last
            }
        };
        // fxdax: and the raw files of the segments framed, whose footers name commits durable now
        // (TASK-153.8).
        let raw = self.state()?.segments.raw(durable);
        self.remove(raw);
        Ok(())
    }

    /// Makes `commits`, a flush's, durable, in order: once the segments they closed are framed,
    /// their data, then their records in the index, at `tail`; but for the files they made
    /// unreferenced.
    fn make_durable(
        &self,
        tail: &mut IndexTail<D::File>,
        commits: &mut [Pending<D::File>],
    ) -> Result<(), Error> {
        // fxdax: the segments the commits closed, holding their data, framed and synced by their
        // framings, which end themselves, waited for holding no lock (TASK-153.8).
        let closed = commits.iter().filter_map(|pending| pending.closed);
        self.wait_for_framings(closed)?;
        let mut syncs = Syncs::default();
        for pending in commits.iter_mut() {
            syncs.extend(std::mem::take(&mut pending.syncs));
        }
        syncs.sync(&*self.dir)?;
        tail.write(&*self.dir, commits.iter().map(|pending| &pending.write))?;
        self.dir.barrier()?;
        Ok(())
    }

    /// fxdax: waits, holding no lock, for the framings of `segments` to end, each of which ends
    /// itself ([`Store::frame_behind`]): fails if one failed.
    fn wait_for_framings(
        &self,
        segments: impl IntoIterator<Item = SegmentId>,
    ) -> Result<(), Error> {
        let segments: Vec<SegmentId> = segments.into_iter().collect();
        let mut state = self.lock();
        while segments.iter().any(|&number| state.segments.is_framing(number)) {
            state = state.wait(&self.idle);
        }
        match segments.iter().find_map(|&number| state.segments.framing_failure(number)) {
            Some(error) => bail!("framing a segment: {error}"),
            None => Ok(()),
        }
    }

    /// fxdax: whether commits are waiting for [`Store::flush`] to make them durable: ordered, and
    /// not taken by a flush in flight.
    pub fn flush_pending(&self) -> bool {
        self.durability.lock().unwrap().queued()
    }

    /// fxdax: the epoch of the last commit the uploader may take, and of the last commit of the
    /// segments closed it may take: the last durable.
    fn uploadable(&self) -> u64 {
        if !rules().durable_only {
            return u64::MAX;
        }
        let durability = self.durability.lock().unwrap();
        if rules().durable_once_synced { durability.durable() } else { durability.taken() }
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
        let write = self.begin_write(extent, offset, data.len() as u64)?;
        let mut aligned = Aligned::new(data.len());
        aligned.as_mut_slice().copy_from_slice(data);
        write.file().write_at(aligned.as_slice(), write.offset())?;
        write.finish();
        Ok(())
    }

    /// Sets the most bytes the branch's extents may hold, durably: the index holds it from now on,
    /// after the commits made so far, which it makes durable first; and the next commit sets it in
    /// the branch's state, as its delta and its segment's footer say (TASK-165.3, TASK-165.7). It
    /// may grow it, or shrink it no lower than what the extents hold, with what writes in progress
    /// may add ([`TooSmall`]).
    pub fn set_capacity(&self, capacity: u64) -> Result<(), Error> {
        let mut tail = self.flushing.lock().unwrap();
        {
            let mut state = self.writable()?;
            let block_size = self.block_size as u64;
            ensure!(capacity >= block_size, "a capacity of {capacity} is less than a block");
            state.space.check_capacity(capacity)?;
            state.space.set_capacity(capacity);
            // fxdax: and the branch's in object storage, as of the next commit (TASK-165.3).
            state.uncommitted.resize(capacity);
        }
        // After the commits before it, which a crash keeps with it, or with none of it.
        self.flush_locked(&mut tail)?;
        let record = IndexWrite::Delta(encode(&Record::Capacity(capacity)));
        let written = tail.write(&*self.dir, [&record]).and_then(|()| Ok(self.dir.barrier()?));
        if written.is_err() {
            self.lock().failed = true;
        }
        written.context("recording the capacity")
    }

    /// fxdax: has the next commit set the branch's capacity, as it is, in the branch's state as
    /// object storage keeps it, its segment's footer saying so: for an uploader whose branch says
    /// none there, or another (TASK-165.3).
    pub fn restate_capacity(&self) -> Result<(), Error> {
        let mut state = self.writable()?;
        let capacity = state.space.capacity();
        state.uncommitted.resize(capacity);
        Ok(())
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
        let open = state.uncommitted.is_open(extent);
        Ok(state.extents.get(&extent).map(|entry| (entry.len, open)))
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

/// Removes the files of `branch`'s extents' data in `dir` no extent of `recovered`, its state, is
/// in: what commits that weren't durable wrote, or that were to go once the commits that let them
/// go were; and the raw files of extents open, which no commit made durable refers to, and which
/// are framed once sealed.
fn remove_unheld<D: Dir>(dir: &D, branch: u64, recovered: &Recovered) -> Result<(), Error> {
    let held = files_of(&recovered.extents);
    for kind in [SEGMENTS, EXTENTS, OPEN_SEGMENTS, OPEN_EXTENTS] {
        for number in list_numbers(dir, &branch_dir(kind, branch))? {
            let file = match kind {
                SEGMENTS | OPEN_SEGMENTS => Some(FileKey::Segment(SegmentId { branch, number })),
                EXTENTS => Some(FileKey::Own(ExtentId::new(branch, number))),
                _ => None,
            };
            if !file.is_some_and(|file| held.contains(&file)) {
                let path = format!("{}/{:016x}", branch_dir(kind, branch), number);
                dir.remove(&path).with_context(|| format!("removing {path}"))?;
            }
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
                let description = Description { block_size: 4096, capacity: 1 << 20, branch: 1 };
                let store =
                    Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
                let _committing = store.committing.lock().unwrap();
                let _state = store.state.lock().unwrap();
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
                let description = Description { block_size: 4096, capacity: 1 << 20, branch: 1 };
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
        let description = Description { block_size: 4096, capacity: 1 << 20, branch: 1 };
        let store = Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
        // Whether extent `sequence` is recorded as sealed after commit `epoch`.
        let sealed_after = |sequence, epoch| {
            store.state.lock().unwrap().sealed.after(ExtentId::new(1, sequence), epoch)
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
        let description = Description { block_size: 4096, capacity: 1 << 20, branch: 1 };
        let store = Store::create(crate::MemDir::new(), description, Options::default()).unwrap();
        store.write(ExtentId::new(1, 1), 0, &[1; 4096]).unwrap();
        store.state.lock().unwrap().space.added(4096, true);
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
