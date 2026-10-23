// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store. See the crate's docs.

use crate::dir::{Dir, File, not_found};
use crate::durability::{Batch, Durability, Syncs};
use crate::framed::{Aligned, Framed, Packed, Part, Source, read_aligned};
use crate::index::{
    Appended, IndexLog, IndexWrite, Recovered, rebuild, recover, start_fork, write_records,
};
use crate::kept::{Kept, KeptExtents};
use crate::layers::Layer;
use crate::layout::{
    BranchRecord, Change, Checkpoint, Data, Delta, Description, Extent, FileKey, OPEN_EXTENTS,
    OPEN_SEGMENTS, Origin, Piece, Place, SEGMENTS, SegmentId, add_piece, branch_dir, extent_path,
    list_numbers, open_extent_path, open_segment_path,
};
use crate::lineage::Lineage;
use crate::segments::{Closing, Segments, Unframed, Victim, close_left_open};
use crate::state::{
    Copied, Gate, OwnFiles, Placement, Reader, Space, State, Uncommitted, Unreferenced,
};
use crate::sync::{Condvar, Mutex, MutexGuard, rules};
use crate::uploads::{Closed, Snapshot, Uploads};
use crate::views::{Limits, Sealed, View, Views};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::Chain;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use storage_device::{ExtentId, NoExtent, OtherBranch, SealedExtent};

/// The error of a write the store has no room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the extent store is full")]
pub struct NoSpace;

/// The error of a capacity smaller than what the store holds: its extents' bytes, and those that
/// writes in progress may add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the extent store holds {held} bytes, more than {capacity}")]
pub struct TooSmall {
    pub capacity: u64,
    pub held: u64,
}

/// fxdax: the error of a read of an extent the store holds whose data is elsewhere
/// ([`Store::adopt`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:?}'s data is not in the store")]
pub struct RemoteData(pub ExtentId);

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
    /// The bytes of extents' data in segments, which cleaning keeps.
    pub live_bytes: u64,
    /// fxdax: of the bytes of the segments' data, those no extent is in, which cleaning takes back:
    /// but those another branch may hold extents in (TASK-79.2).
    pub dead_bytes: u64,
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

/// fxdax: what an open branch holds in memory of its own (TASK-31): its extents' entries over the
/// state it shares with its parent and its forks, and the segments it tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub entries: usize,
    pub segments: usize,
}

/// What cleaning did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cleaned {
    /// The segments removed, and the bytes they took.
    pub segments: u64,
    pub bytes: u64,
    /// The bytes of extents' data copied out of them.
    pub copied: u64,
}

/// A branch of an extent store, open. See the crate's docs.
pub struct Store<D: Dir> {
    lineage: Arc<Lineage<D>>,
    branch: u64,
    /// Its opening of the branch, which closing it ends.
    token: u64,
    /// The branch the branch was forked from, if it is a fork.
    parent: Option<u64>,
    /// The layer its index is over, its parent's state at the fork, if its index holds what it
    /// changed over it (TASK-31).
    origin: Option<Arc<Layer>>,
    options: Options,
    state: Mutex<State<D::File>>,
    /// Held by a commit, or by cleaning, which commits.
    committing: Mutex<()>,
    /// When its commits are durable: those ordered that no flush has taken yet, oldest first, for
    /// [`Store::flush`] to make durable, and the last that is.
    durability: Mutex<Durability<Pending<D::File>>>,
    /// Held by a flush: commits become durable in order.
    flushing: Mutex<()>,
    /// Notified when the open segment has no write in progress, and when it closed: what
    /// [`Store::close_now`] and the writes it holds up wait for; fxdax: and when the segments it
    /// closed are framed, which forks and the branch's closing wait for.
    idle: Condvar,
    /// fxdax: its views, which followers read it through ([`Store::view`]).
    views: Arc<Views>,
    /// fxdax: how often the branch changed as may make cleaning behind its commits due, each
    /// counted holding its state ([`Store::cleaning_changes`], TASK-79.2).
    changes: AtomicU64,
}

/// What cleaning takes: the branch's segments whose extents' data takes less than a fraction of
/// them, and which no other branch holds extents in; and their sealed extents to copy out, as
/// they are, those of the least live segments first, so many bytes of them at most.
struct Cleanable {
    victims: BTreeMap<SegmentId, Victim>,
    copies: Vec<(ExtentId, Extent)>,
}

/// What a fork's commit froze: the branch's state as of it, and where it was.
struct Forked {
    layer: Arc<Layer>,
    origin: Origin,
}

/// What a commit ordered but not yet durable leaves for the flush that makes it durable.
struct Pending<F> {
    /// The files whose data it refers to, and the directories whose entries changed, to sync.
    syncs: Syncs<F>,
    /// Its record in the index.
    write: IndexWrite<F>,
    /// The files of the branch's alone it made unreferenced ([`Unreferenced::doomed`]), and the
    /// raw files of the segments it closed, once framed: they go once it is durable.
    doomed: Vec<String>,
    /// fxdax: the segment it closed, if any, and those closed for want of room since the last
    /// commit ([`State::closings`]), being framed, whose framed files it waits for and syncs.
    closings: Vec<Closing<F>>,
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
        Data::Remote => Place::Remote,
    }
}

impl<D: Dir> Store<D> {
    /// Makes a new, empty store in `dir`, which is empty, of one branch, as `description`
    /// describes, and opens that branch.
    pub fn create(dir: D, description: Description, options: Options) -> Result<Self, Error> {
        Lineage::create(dir, description, options)?.open_branch(description.branch)
    }

    /// Opens the store in `dir`, recovering it as it was at its last commit that is durable, and
    /// its first branch.
    pub fn open(dir: D, options: Options) -> Result<Self, Error> {
        let lineage = Lineage::open(dir, options)?;
        let first = lineage.first();
        lineage.open_branch(first)
    }

    /// Opens `branch` of `lineage`, whose record is `record`: its own state, over its parent's at
    /// the fork if it is a fork whose index holds what it changed over that (TASK-31); or, at a
    /// fork's first opening, its parent's at the fork alone, which its index then holds it changed
    /// nothing over. That state is a layer the fork shares with its parent and its parent's other
    /// forks of the same point, which the lineage holds: opening a fork reads and writes what it
    /// changed, whatever its parent holds.
    pub(crate) fn open_in(
        lineage: Arc<Lineage<D>>,
        branch: u64,
        token: u64,
        record: BranchRecord,
    ) -> Result<Self, Error> {
        let dir = &lineage.dir;
        let origin = match record.over_parent() {
            Some(_) => Some(
                lineage.base(branch).ok_or_else(|| anyhow!("branch {branch:#x} has no base"))?,
            ),
            None => None,
        };
        let recovered = match (record.resolved, &origin) {
            (true, _) => recover(dir, branch, origin.clone())?,
            (false, Some(origin)) => {
                let state = start_fork(dir, branch, origin)?;
                lineage.resolved(branch, token)?;
                state
            }
            (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
        };
        let Recovered { epoch, next_sequence, root, extents, .. } = recovered;
        // Footers of commits not recovered, which were not durable, are no footers: the commits
        // after this epoch are others.
        close_left_open(dir, branch, epoch)?;
        // fxdax: the raw files of the extents the branch's last store had open, which no commit
        // made durable refers to: those of writes abandoned, or of a store that failed, which
        // opening the store would have removed (TASK-79.5). The extents may be written anew.
        for sequence in list_numbers(dir, &branch_dir(OPEN_EXTENTS, branch))? {
            dir.remove(&open_extent_path(ExtentId::new(branch, sequence)))?;
        }

        // The segments it tracks, with the bytes of extents' data each holds: those of the
        // extents it holds over its origin, and of its origin's it hides, which it holds otherwise
        // than its origin does; and its own extents' files. They must be there. Of the rest, it
        // holds what its origin does.
        let base_live = |segment| origin.as_ref().map_or(0, |origin| origin.live(segment));
        let mut tracked: BTreeMap<SegmentId, u64> = BTreeMap::new();
        let mut stored = origin.as_ref().map_or(0, |origin| origin.stored());
        let mut own_sizes = HashMap::new();
        for (_, entry) in extents.shadowed() {
            stored -= entry.len;
            if let Data::Pieces(pieces) = &entry.data {
                for piece in pieces {
                    let live =
                        tracked.entry(piece.segment).or_insert_with(|| base_live(piece.segment));
                    *live -= piece.len;
                }
            }
        }
        // fxdax: its own extents are all over its origin, which holds those it inherited.
        let mut own_stored = 0;
        for (extent, entry) in extents.changed() {
            stored += entry.len;
            if extent.branch == branch {
                own_stored += entry.len;
            }
            match &entry.data {
                Data::Pieces(pieces) => {
                    for piece in pieces {
                        let live = tracked
                            .entry(piece.segment)
                            .or_insert_with(|| base_live(piece.segment));
                        *live += piece.len;
                    }
                }
                Data::Own => {
                    let path = extent_path(*extent);
                    let file = dir.open(&path, true);
                    let file = file.with_context(|| format!("extent {extent:?}'s file"))?;
                    own_sizes.insert(*extent, file.size()?);
                }
                Data::Remote => {}
            }
        }
        // Those it holds nothing in, all of its origin's extents in them hidden, it let go of.
        tracked.retain(|_, live| *live > 0);
        let options = lineage.options;
        let mut segments = Segments::open(dir, branch, tracked, epoch, options.open_files)?;
        // fxdax: those its forks hold extents in; and the sealed extents in each, which cleaning
        // copies out, of those it holds over its origin, if any, as no other is in its own
        // segments (TASK-79.2).
        segments.share(|number| lineage.shared(branch, FileKey::Segment(number)));
        for (extent, entry) in extents.changed() {
            segments.place(*extent, &entry.data);
        }
        let open_segment = segments.open_segment();

        // A new index file, with a checkpoint of what was recovered, which replaces the others but
        // those forks need.
        let segment = open_segment.number;
        let state = Recovered { epoch, next_sequence, root, segment, extents };
        let pinned = || lineage.pinned(branch);
        let index = IndexLog::open(dir, branch, &state, origin.as_ref(), pinned)?;
        let Recovered { root, mut extents, .. } = state;
        let mut holdings = extents.holdings();
        holdings.own.insert(FileKey::Segment(open_segment));
        lineage.recovered(branch, epoch, &holdings);
        // A branch whose index holds all it holds shares in memory what its forks inherited of it,
        // the latest of it, holding over that what it changed since.
        if origin.is_none()
            && let Some(layer) = lineage.latest_layer_of(branch)
        {
            extents.rebase(layer);
        }

        let views = Views::new(epoch, &root);
        Ok(Self {
            lineage,
            branch,
            token,
            parent: record.origin.map(|origin| origin.parent),
            origin,
            options,
            state: Mutex::new(State {
                extents,
                space: Space::new(record.capacity, stored, own_stored),
                epoch,
                root,
                next_sequence,
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
                framings: 0,
                closings: Vec::new(),
            }),
            committing: Mutex::new(()),
            durability: Mutex::new(Durability::new(epoch)),
            flushing: Mutex::new(()),
            idle: Condvar::new(),
            views,
            changes: AtomicU64::new(0),
        })
    }

    /// The branch's state as its newest checkpoint and the footers of its closed segments after it
    /// say: as of the last commit a closed segment holds, if one does (doc-7: a branch's history is
    /// in its segments).
    pub fn rebuild(&self) -> Result<Rebuilt, Error> {
        let state = rebuild(&self.lineage.dir, self.branch, self.origin.clone())?;
        let mut extents: Vec<_> = state
            .extents
            .iter()
            .into_iter()
            .map(|(extent, entry)| (*extent, entry.len, place_of(entry)))
            .collect();
        extents.sort_by_key(|(extent, ..)| *extent);
        Ok(Rebuilt {
            epoch: state.epoch,
            root: state.root,
            next_sequence: state.next_sequence,
            extents,
        })
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
            .into_iter()
            .map(|(extent, entry)| (*extent, entry.len, place_of(entry)))
            .collect();
        extents.sort_by_key(|(extent, ..)| *extent);
        Ok(extents)
    }

    /// The store's branches.
    pub fn lineage(&self) -> &Arc<Lineage<D>> {
        &self.lineage
    }

    /// Forks the branch: a new branch, `child`, holding what this one holds once its open extents
    /// are sealed, and none of its own yet, which [`Lineage::open_branch`] opens. A barrier, which
    /// commits durably; then it takes the same time and space whatever the branch holds: the child
    /// inherits the extents, in this branch's files and those it inherited, rather than copies,
    /// and, in memory, the branch's state as of the fork, a layer it shares with the branch, which
    /// goes on over it, and with the branch's other forks of the same point (TASK-31). Returns the
    /// epoch of the fork's commit, which ends the segment that names the fork. A fork to a branch
    /// the store has fails, committing nothing.
    pub fn fork(&self, child: u64) -> Result<u64, Error> {
        let committing = self.committing.lock().unwrap();
        // fxdax: the child reserved before the commit that names it: else a fork to a branch the
        // store has would fail once its footer, durable, named the branch again, from another
        // point, which the uploader uploads, and the branch's writers in object storage find
        // forked twice, failing for ever (TASK-79.78).
        let forking = self.lineage.forking(child)?;
        // At the end of its segment, which the commit closes, and whose footer names it (doc-7,
        // Forks).
        self.writable()?.segments.close_for_fork(child);
        let forked = self.commit_locked(&committing, None, true)?;
        let Forked { layer, origin } = forked.expect("a fork's commit freezes the branch's state");
        self.flush()?;
        // fxdax: and the segments closed between commits framed, from whose framed files the child
        // reads what it inherited of them (TASK-41.1).
        let mut state = self.state()?;
        if rules().forks_wait_for_framings {
            state = self.framed(state);
        }
        let capacity = state.space.capacity();
        drop(state);
        let epoch = origin.epoch;
        self.lineage.fork(forking, origin, capacity, layer)?;
        Ok(epoch)
    }

    /// The branch the branch was forked from, if it is a fork.
    pub fn parent(&self) -> Option<u64> {
        self.parent
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.lineage.dir
    }

    /// The branch, as its user sees it.
    pub fn description(&self) -> Description {
        Description {
            block_size: self.lineage.block_size,
            capacity: self.state.lock().unwrap().space.capacity(),
            branch: self.branch,
        }
    }

    fn state(&self) -> Result<MutexGuard<'_, State<D::File>>, Error> {
        let state = self.state.lock().unwrap();
        ensure!(!state.failed, "the extent store failed");
        Ok(state)
    }

    /// The state, to change: unless the branch is closed.
    fn writable(&self) -> Result<MutexGuard<'_, State<D::File>>, Error> {
        self.still_writable(self.state.lock().unwrap())
    }

    /// `state`, to change, as found again after waiting, its lock let go of meanwhile: unless the
    /// store failed, or the branch closed, since (TASK-83).
    fn still_writable<'a>(
        &self,
        state: MutexGuard<'a, State<D::File>>,
    ) -> Result<MutexGuard<'a, State<D::File>>, Error> {
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
        let flushing = rules().close_waits_for_flushes.then(|| self.flushing.lock().unwrap());
        let flushed = match &flushing {
            _ if !self.flush_pending() => Ok(()),
            Some(flushing) => self.flush_locked(flushing),
            None => self.flush(),
        };
        drop(flushing);
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            // fxdax: once the segments closed between commits are framed, which another store,
            // opening the branch, would close again meanwhile (TASK-41.1); and those closed for
            // want of room since the last commit, or by a commit that failed, which no flush waited
            // for: the opening keeps their framed files, if whole, or frames their raw files anew
            // (TASK-79.2).
            if rules().close_waits_for_framings {
                state = self.framed(state);
            }
            let closings = std::mem::take(&mut state.closings);
            state.closed = true;
            drop(state);
            if rules().close_waits_for_framings {
                for closing in &closings {
                    if let Err(err) = closing.wait() {
                        log::warn!("extent store: {err:#}, which opening the branch does again");
                    }
                }
            }
            self.lineage.closed(self.branch, self.token);
        }
        flushed
    }

    /// Fails unless I/O of `len` bytes at `offset` in an extent is aligned to blocks, and ends
    /// within the most an extent holds.
    fn check_io(&self, offset: u64, len: u64) -> Result<(), Error> {
        let block_size = self.lineage.block_size as u64;
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
        // Another branch's, which it inherited: the lineage's, which its branches share, one file
        // open for all the forks of a base reading it (TASK-34).
        if number.branch != self.branch {
            return Ok(Source::Framed(self.lineage.segment_file(number)?));
        }
        state.segments.source(&self.lineage.dir, number)
    }

    /// The file `extent`'s data is in, stored on its own: raw while it is open, framed once sealed.
    fn own_source(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
    ) -> Result<Source<D::File>, Error> {
        match state.uncommitted.placement(extent) {
            Some(Placement::Own(file)) => Ok(Source::Raw(file.clone())),
            _ => Ok(Source::Framed(state.own_files.file(&self.lineage.dir, extent)?)),
        }
    }

    /// Whether a commit, or a closing between commits, waits for the writes in progress, holding
    /// new ones back ([`Store::begin_write`]).
    pub fn holding_writes(&self) -> bool {
        self.state.lock().unwrap().gate.is_held()
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
            state = self.idle.wait(state).unwrap();
        }
        let mut state = self.still_writable(state)?;
        let end = offset + len;
        let current = state.extents.get(&extent).map(|entry| entry.len);
        if current.is_some() && !state.uncommitted.is_open(extent) {
            bail!(SealedExtent(extent));
        }
        let grows = end.saturating_sub(current.unwrap_or(0));
        let to_segment = match state.uncommitted.placement(extent) {
            Some(placement) => matches!(placement, Placement::Segment),
            None => end < self.options.large_extent,
        };
        // fxdax: what it writes over, in a segment, takes room too (TASK-79.2).
        let overwrites = if to_segment { len - grows.min(len) } else { 0 };
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
        let (dir, path) = (&self.lineage.dir, open_extent_path(extent));
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
    /// or else closed, as between commits, and framed behind, which the next commit's flush waits
    /// for, as for its own closing ([`State::closings`]): holding the state alone, so that the
    /// write waits neither for a commit in progress, nor for the writes in the segment, nor for
    /// the framing, as the uploader's closing between commits does ([`Store::close_now`]). Those
    /// writes go on: their extents are open, which the next commit copies out, from the segment's
    /// raw file, which reads read until its flush has the framed file.
    fn make_room(&self, state: &mut State<D::File>, len: u64) -> Result<(), Error> {
        if state.segments.fits(len, self.options.segment_limit()) {
            return Ok(());
        }
        let full = state.segments.open_segment();
        if state.segments.has_commits(state.epoch) {
            let unframed = self.close_open_segment(state, state.epoch)?;
            state.closings.push(unframed.frame_behind(self.lineage.clone()));
        } else {
            state.segments.retire(&self.lineage.dir)?;
            state.index.set_aside(full.number);
            self.lineage.claim(self.branch, [FileKey::Segment(state.segments.open_segment())]);
        }
        Ok(())
    }

    fn finish_write(&self, write: &Write<'_, D>) {
        let mut state = self.state.lock().unwrap();
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
        let entry = state.extents.get_or_insert_with(extent, || Extent {
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
                add_piece(pieces, piece);
                if state.segments.held(number, write.len) {
                    self.idle.notify_all();
                }
                state.uncommitted.wrote_over(extent, write.len - grew.min(write.len));
                state.uncommitted.push(Change::Write { extent, piece });
            }
            // Its file is the branch's to claim once it is sealed, and framed.
            (None, Data::Own) => {
                state.uncommitted.push(Change::WriteOwn { extent, len: entry.len })
            }
            _ => unreachable!("an extent's data is in segments or its own file"),
        }
        state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        self.written(state);
    }

    fn abandon_write(&self, write: &Write<'_, D>) {
        self.drop_write(&mut self.state.lock().unwrap(), write);
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
        self.changed(state);
    }

    /// fxdax: the branch changed as may make cleaning behind its commits due, as `state`, which
    /// its caller holds, has it ([`Store::cleaning_changes`], TASK-79.2).
    fn changed(&self, _state: &State<D::File>) {
        self.changes.fetch_add(1, Ordering::Relaxed);
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
        let unpacked = &self.lineage.unpacked;
        let mut parts = Vec::new();
        match &entry.data {
            Data::Own => {
                if held_end > offset {
                    let source = self.own_source(state, extent)?;
                    source.locate(offset, held_end - offset, unpacked, &mut parts)?;
                }
            }
            Data::Remote => bail!(RemoteData(extent)),
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
                    file.read_at(data.as_mut_slice(), offset)?;
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

    /// A part stored compressed, read and unpacked.
    pub fn read_packed(&self, part: &Packed<D::File>) -> Result<Arc<[u8]>, Error> {
        let stored = read_aligned(&*part.file, part.stored.offset, part.stored.len as usize)?;
        self.unpack(part, stored.as_slice())
    }

    /// A part stored compressed, unpacked from `bytes`, its stored bytes and any after them, read
    /// from its file, checked against its CRC; reads of it find it unpacked for a while.
    pub fn unpack(&self, part: &Packed<D::File>, bytes: &[u8]) -> Result<Arc<[u8]>, Error> {
        part.unpack(bytes, &self.lineage.unpacked)
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
        state.space.removed(entry.len, extent.branch == self.branch);
        state.uncommitted.push(Change::Free(extent));
        state.track(&entry);
        // A sealed one's data stays until the uploader has what it needs of it, and until the
        // commit that frees it decides whether views keep it. fxdax: an open one's goes at once:
        // no commit held it, so that no reader needs it, and what it took of segments is dead
        // space, counted so (TASK-79.2). Of a file of its own it has none, which its seal would
        // have made, but its raw file, which goes with the next commit: the lineage has nothing to
        // let go of, which would take the file the extent's next opening seals at that path
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
            Data::Remote => unreachable!("an open extent's data is in segments or a raw file"),
        }
        self.changed(state);
        Ok(())
    }

    /// fxdax: holds `extent`, sealed, of `len` bytes, whose data is elsewhere: reads of it fail
    /// with [`RemoteData`], and the store's user reads it from there. A branch kept in object
    /// storage, opened where its data is not, holds its extents so.
    pub fn adopt(&self, extent: ExtentId, len: u64) -> Result<(), Error> {
        let mut state = self.writable()?;
        ensure!(!state.extents.contains_key(&extent), "the store holds {extent:?}");
        state.extents.insert(extent, Extent { len, data: Data::Remote });
        state.space.added(len, extent.branch == self.branch);
        if extent.branch == self.branch {
            state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        }
        state.uncommitted.push(Change::Adopt { extent, len });
        // Footers name no extent whose data is elsewhere: a checkpoint does.
        state.index.want_checkpoint();
        Ok(())
    }

    /// fxdax: what the store holds as of now.
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let state = self.state()?;
        let mut extents: Vec<_> =
            state.extents.iter().into_iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        extents.sort();
        Ok(Snapshot { epoch: state.epoch, extents, root: state.root.clone() })
    }

    /// fxdax: from now on, keeps for an uploader the segments it closes, until uploaded
    /// ([`Store::closed`], [`Store::uploaded`]), their footers, stamped by `chain`, the chain of the
    /// uploader's writer of the branch, from where it is, saying what each commit changed; and the
    /// data of the extents its commits free, and the bytes they write
    /// ([`Store::unreleased_bytes`]), until released ([`Store::release`]): unless a commit came
    /// since `epoch`, the snapshot's the uploader starts from. Returns whether it does.
    pub fn upload(&self, chain: Chain, epoch: u64) -> Result<bool, Error> {
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
        state.uploads.get_or_insert_with(Uploads::default);
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
        state.kept.release(epoch);
        state.trim_kept(&self.views.keeping(), state.epoch);
        self.changed(state);
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
        self.commit_locked(&committing, root, false).map(drop)
    }

    /// Commits, as [`Store::commit_ordered`] does; and, if `fork`, freezes the branch's state as of
    /// the commit, which a fork of it inherits (TASK-31).
    fn commit_locked(
        &self,
        _committing: &MutexGuard<'_, ()>,
        root: Option<&[u8]>,
        fork: bool,
    ) -> Result<Option<Forked>, Error> {
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
        let committed = state.failing(|state| self.commit_in(state, root, fork));
        committed.context("committing the extent store")
    }

    /// The steps of the next commit, which sets `root`, if any, and, if `fork`, freezes the
    /// branch's state as of it, in their order: each takes what those before it leave.
    fn commit_in(
        &self,
        state: &mut State<D::File>,
        root: Option<&[u8]>,
        fork: bool,
    ) -> Result<Option<Forked>, Error> {
        let epoch = state.epoch + 1;
        self.unshare(state);
        self.make_moves(state);
        let sealing = self.seal_open(state)?;
        self.publish(state, epoch, root, &sealing);
        let changes = state.uncommitted.take();
        if let Some(uploads) = &mut state.uploads {
            uploads.record(epoch, &changes, &state.extents);
        }
        // The open segment's footer records them, before the segment closes. fxdax: with those
        // closed for want of room since the last commit, which it waits for, framed, too.
        state.segments.record(&changes, root, &state.extents);
        if let Some(closing) = self.close_if_due(state, epoch)? {
            state.closings.push(closing);
        }
        // What its flush syncs, after the closing, whose directories its flush must sync: the
        // framed file's entry is durable before its raw file goes.
        let mut syncs = Syncs::default();
        state.own_files.take_syncs(&mut syncs);
        state.segments.take_syncs(&mut syncs);
        // The segments none of its extents' data is in any more, by this commit, if not before:
        // the branch lets them go; those it closed, once a checkpoint covers their footers. fxdax:
        // but those the uploader has yet to upload, which are the branch's own (TASK-85).
        let uploads = &state.uploads;
        let reaped = state.segments.reap(|segment| {
            segment.branch == self.branch
                && uploads.as_ref().is_some_and(|uploads| uploads.holds(segment.number))
        });
        let released = state.index.let_go(reaped);
        state.unreferenced.released.extend(released);
        let appended = self.append_record(state, epoch, root, changes)?;
        state.unreferenced.doomed.extend(appended.replaced);
        state.unreferenced.released.extend(appended.covered);
        let Unreferenced { doomed, released } = state.unreferenced.take();
        self.lineage.release(self.branch, epoch, released);
        state.epoch = epoch;
        if let Some(root) = root {
            state.root = root.to_vec();
        }
        let forked = fork.then(|| {
            let layer = state.extents.freeze(self.origin.as_ref(), &state.root);
            // fxdax: the fork holds what is in them, and what is freed in them: no dead space the
            // branch can take back alone, nor counts (TASK-79.2).
            state.segments.share(|number| layer.holds(FileKey::Segment(number)));
            let origin = Origin {
                parent: self.branch,
                epoch,
                next_sequence: state.next_sequence,
                index: state.index.number(),
            };
            Forked { layer, origin }
        });
        // Queued while the state is locked, so that commits queue in their order.
        let closings = std::mem::take(&mut state.closings);
        let pending = Pending { syncs, write: appended.write, doomed, closings };
        self.durability.lock().unwrap().push(epoch, pending);
        self.changed(state);
        Ok(forked)
    }

    /// fxdax: takes the branch's own segments that forks held for its alone again, once they let
    /// go of what they held in them, or were deleted (TASK-79.2): what is freed in them is then
    /// dead space, which counts, and which cleaning takes back.
    fn unshare(&self, state: &mut State<D::File>) {
        let shared = |segment| self.lineage.shared(self.branch, FileKey::Segment(segment));
        state.segments.unshare(self.lineage.unshared(), shared);
    }

    /// Holds new writes back ([`Store::begin_write`]) while the writes in progress are `busy`, as
    /// a commit, or a closing between commits, waits for them, and returns the state once they
    /// are not: which its caller finds again, as it let it go meanwhile.
    fn hold_writes<'a>(
        &self,
        mut state: MutexGuard<'a, State<D::File>>,
        busy: impl Fn(&State<D::File>) -> bool,
    ) -> MutexGuard<'a, State<D::File>> {
        state.gate.hold();
        while busy(&state) {
            state = self.idle.wait(state).unwrap();
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
    /// off the commit, which its flush waits for.
    fn close_if_due(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
    ) -> Result<Option<Closing<D::File>>, Error> {
        if !state.segments.due(&self.options) {
            return Ok(None);
        }
        let unframed = self.close_open_segment(state, epoch)?;
        Ok(Some(unframed.frame_behind(self.lineage.clone())))
    }

    /// Appends commit `epoch`'s record, of `changes` and `root`, if it sets one, to the index:
    /// its delta, or, if one is due, a checkpoint of the branch as of it, in a new file, which
    /// replaces the last once the commit is durable.
    fn append_record(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        root: Option<&[u8]>,
        changes: Vec<Change>,
    ) -> Result<Appended<D::File>, Error> {
        let next_sequence = state.next_sequence;
        let delta = Delta { epoch, next_sequence, root: root.map(<[u8]>::to_vec), changes };
        let checkpoint = || {
            let (root, segment) = (root.unwrap_or(&state.root), state.segments.open_segment());
            let (extents, origin) = (&state.extents, self.origin.as_ref());
            Checkpoint::of(epoch, next_sequence, root, segment.number, extents, origin)
        };
        let pinned = || self.lineage.pinned(self.branch);
        state.index.append(&self.lineage.dir, delta, checkpoint, &self.options, pinned)
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
                if let Err(err) = self.lineage.dir.remove(&path) {
                    log::warn!("extent store: failed to remove {path}: {err}");
                }
            }
            return Ok(());
        };
        let large = entry.len >= self.options.large_extent;
        let open = state.segments.open_segment();
        match (&entry.data, state.uncommitted.placement(extent)) {
            (Data::Remote, _) => return Ok(()),
            // Written to a raw file of its own, which it is framed from.
            (Data::Own, Some(Placement::Own(raw))) => {
                let raw = raw.clone();
                self.write_own(state, extent, entry.len, |at, buf| raw.read_at(buf, at))?;
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
        let file =
            self.lineage.dir.create(&path, true).with_context(|| format!("creating {path}"))?;
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
        self.lineage.claim(self.branch, [FileKey::Own(extent)]);
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
        let unframed = state.segments.close(&self.lineage.dir, epoch, state.next_sequence)?;
        self.lineage.claim(self.branch, [FileKey::Segment(state.segments.open_segment())]);
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
    /// so far without waiting for the next. Writes wait for the closing alone: it frames the
    /// segment, and makes it durable, holding no lock, as a commit's closing does, so that the
    /// store serves its writes, reads and commits meanwhile, which read the segment from its raw
    /// file (TASK-41.1). Its framed file is durable when this returns; its raw file goes with the
    /// next commit, once durable, as the commits its footer names are then, as a commit's closing
    /// does: so that opening the branch reads no segment whose raw file is gone (TASK-92.16).
    /// Returns whether it closed.
    pub fn close_now(&self) -> Result<bool, Error> {
        let Some(unframed) = self.close_between_commits()? else { return Ok(false) };
        let number = unframed.number();
        let framed = self.frame_durably(unframed);
        let mut state = self.state.lock().unwrap();
        match &framed {
            Ok(framed) => {
                state.segments.framed(number, framed.clone());
                state.unreferenced.doomed.push(open_segment_path(number));
            }
            // Framed, and durable, or the store fails.
            Err(_) => state.failed = true,
        }
        state.framings -= 1;
        if state.framings == 0 {
            self.idle.notify_all();
        }
        drop(state);
        framed.context("closing the open segment")?;
        Ok(true)
    }

    /// fxdax: closes the open segment, between commits, as [`Store::close_now`] does, and returns
    /// it, to be framed: none if no commit came since it opened, nor fork, or the branch is closed.
    /// A fork, and the branch's closing, wait for it to be framed.
    fn close_between_commits(&self) -> Result<Option<Unframed<D::File>>, Error> {
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
        state.framings += 1;
        self.changed(&state);
        Ok(Some(unframed))
    }

    /// fxdax: frames `unframed`, a segment closed between commits, holding no lock, and makes its
    /// framed file durable, and its entry.
    fn frame_durably(&self, unframed: Unframed<D::File>) -> Result<Arc<Framed<D::File>>, Error> {
        let dir = &self.lineage.dir;
        let framed = unframed.frame(dir)?;
        framed.file.sync()?;
        for kind in [SEGMENTS, OPEN_SEGMENTS] {
            dir.sync_dir(&branch_dir(kind, self.branch))?;
        }
        dir.barrier()?;
        Ok(framed)
    }

    /// fxdax: `state`, once no segment closed between commits is being framed
    /// ([`Store::close_now`]), which it let go of meanwhile.
    fn framed<'a>(
        &self,
        mut state: MutexGuard<'a, State<D::File>>,
    ) -> MutexGuard<'a, State<D::File>> {
        while state.framings > 0 {
            state = self.idle.wait(state).unwrap();
        }
        state
    }

    /// fxdax: the segments closed and not yet taken whose commits are durable, oldest first,
    /// `limit` of them at most, for the uploader, which has them until it says they are uploaded
    /// ([`Store::uploaded`]): the rest it takes later, their files not opened meanwhile.
    pub fn closed(&self, limit: usize) -> Result<Vec<Closed<D::File>>, Error> {
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
            let mut own = Vec::with_capacity(footer.own.len());
            for (extent, location) in &footer.own {
                let file = state.own_files.file(&self.lineage.dir, *extent)?;
                own.push((location.key.clone(), file.file.clone()));
            }
            closed.push(Closed { number, footer, file, own });
        }
        Ok(closed)
    }

    /// fxdax: segment `number`, which [`Store::closed`] gave, is uploaded: it may go once no
    /// extent is in it.
    pub fn uploaded(&self, number: u64) -> Result<(), Error> {
        if let Some(uploads) = &mut self.state()?.uploads {
            uploads.uploaded(number);
        }
        Ok(())
    }

    /// fxdax: makes the commits ordered so far durable, in order, as few syncs as they need
    /// together: their data, then their records in the index, then the files they made
    /// unreferenced go.
    pub fn flush(&self) -> Result<(), Error> {
        self.flush_locked(&self.flushing.lock().unwrap())
    }

    /// [`Store::flush`], holding `flushing`.
    fn flush_locked(&self, _flushing: &MutexGuard<'_, ()>) -> Result<(), Error> {
        // A store that failed makes nothing more durable.
        drop(self.state()?);
        let batch = self.durability.lock().unwrap().take();
        let Some(Batch { last, mut commits }) = batch else { return self.state().map(drop) };
        if let Err(error) = self.make_durable(&mut commits) {
            self.state.lock().unwrap().failed = true;
            return Err(error.context("committing the extent store"));
        }
        self.durability.lock().unwrap().finish(last);
        // The files of extents' data no branch holds extents in go, once the commits that let
        // them go are durable.
        self.lineage.durable(self.branch, last);
        for path in commits.into_iter().flat_map(|pending| pending.doomed) {
            if let Err(err) = self.lineage.dir.remove(&path)
                && !not_found(&err)
            {
                // Opening the store removes it, as nothing refers to it.
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
        Ok(())
    }

    /// Makes `commits`, a flush's, durable, in order: once the segments they closed are framed,
    /// their data, then their records in the index; but for the files they made unreferenced.
    fn make_durable(&self, commits: &mut [Pending<D::File>]) -> Result<(), Error> {
        // fxdax: the segments the commits closed, framed, waited for holding no lock: read from
        // their framed files from now on, which are synced with the commits' data, their raw files
        // going once the commits are durable.
        let mut syncs = Syncs::default();
        for pending in commits.iter_mut() {
            // Each waited for, before any failure is said, so that none goes on framing as the
            // branch is opened again.
            let closings = std::mem::take(&mut pending.closings);
            let framed: Vec<_> = closings.iter().map(Closing::wait).collect();
            for framed in framed {
                let (number, framed) = framed?;
                pending.syncs.file(framed.file.clone());
                pending.doomed.push(open_segment_path(number));
                self.state.lock().unwrap().segments.framed(number, framed);
            }
            syncs.extend(std::mem::take(&mut pending.syncs));
        }
        syncs.sync(&self.lineage.dir)?;
        let writes = commits.iter().map(|pending| &pending.write);
        write_records(&self.lineage.dir, self.branch, writes)?;
        self.lineage.dir.barrier()?;
        Ok(())
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

    /// Cleans the branch's segments whose extents' data takes less than `live_fraction` of them,
    /// and which no other branch holds extents in: copies the data of their extents that are
    /// sealed to the open segment, and commits, which removes them. fxdax: as
    /// [`Store::clean_behind`] copies, then commits, holding the commits' lock only for that.
    pub fn clean(&self, live_fraction: f64) -> Result<Cleaned, Error> {
        let Cleanable { victims, copies } = self.cleanable(live_fraction, u64::MAX)?;
        let mut copied = 0;
        for (extent, entry) in copies {
            copied += self.copy_out(extent, entry)?;
        }
        // The segments go once the commit moving their extents out is durable.
        self.commit(None)?;
        let state = self.state()?;
        let removed: Vec<_> =
            victims.iter().filter(|(number, _)| !state.segments.is_tracked(**number)).collect();
        Ok(Cleaned {
            segments: removed.len() as u64,
            bytes: removed.iter().map(|(_, victim)| victim.len).sum(),
            copied,
        })
    }

    /// fxdax: a count that moves with each change of the branch's that may make cleaning behind
    /// its commits due ([`Store::cleaning_due`]): its commits, its writes done and its frees, its
    /// uploader's releases of what its commits freed, its closings between commits, a new
    /// capacity; and with each time a branch let go of files another made, as a fork deleted does
    /// of the segments it shared with the branch. Found without the branch's state, and moved by
    /// none of its reads, nor by looking whether cleaning is due: so that what cleans behind the
    /// commits looks, holding the state, only once it moved, as soon as it did, and a branch only
    /// read is looked at once (TASK-79.2).
    pub fn cleaning_changes(&self) -> u64 {
        // Each counted holding the state, after what it counts, which a look then takes: a look
        // begun once the count is found moved finds what moved it. And the lineage's, holding
        // its ledger, which the look reads again.
        self.changes.load(Ordering::Relaxed) + self.lineage.unshared()
    }

    /// fxdax: whether cleaning behind the commits is due ([`Store::clean_behind`]): what no extent
    /// is in takes more than `past` bytes of the host's disk in the segments it would take, at
    /// `live_fraction`, or any, once what the branch's files take that no extent holds is past its
    /// capacity's worth, the capacity being short of `past` maybe; and no copy of an earlier
    /// pass's waits for a commit (TASK-79.2). Found as the segments count what they hold, not
    /// going through the extents. What their files take, framed, compressed: counted as the bytes
    /// of data it is, a guest's zeros, freed, made cleaning due as often as other data would,
    /// though they took next to none of the disk, and the passes copied out what was live with
    /// them, a quarter of the guest's throughput under pgbench.
    pub fn cleaning_due(&self, past: u64, live_fraction: f64) -> Result<bool, Error> {
        let mut state = self.writable()?;
        if !state.uncommitted.copies().is_empty() {
            return Ok(false);
        }
        self.unshare(&mut state);
        let reclaimable = state.segments.reclaimable(live_fraction);
        Ok(reclaimable > past || reclaimable > 0 && state.garbage_past_capacity())
    }

    /// fxdax: cleans behind the commits, holding none back but as a write does (TASK-79.2):
    /// copies the data of the sealed extents in the branch's segments whose extents' data takes
    /// less than `live_fraction` of them, and which no other branch holds extents in, to the open
    /// segment, one extent at a time, those of the segments that take back the most of the host's
    /// disk for what they copy first, `most` bytes of it at most. The next commit makes each copy
    /// its extent's place, which removes the segments once no extent is in them, and once durable.
    /// Returns the bytes copied.
    pub fn clean_behind(&self, live_fraction: f64, most: u64) -> Result<u64, Error> {
        let Cleanable { copies, .. } = self.cleanable(live_fraction, most)?;
        let mut copied = 0;
        for (extent, entry) in copies {
            copied += self.copy_out(extent, entry)?;
        }
        Ok(copied)
    }

    /// What cleaning takes ([`Cleanable`]), at `live_fraction`, of its extents those that hold
    /// `most` bytes at most: but those copied out already, whose copies wait for the next commit.
    /// fxdax: found as the segments name the extents sealed in them, not going through every
    /// extent the branch holds, which a pass would hold the state for as long as that takes; and
    /// no more of them than it copies (TASK-79.2). Those of the segments that take back the most
    /// of the host's disk for what they copy first, as cleaning is due for that disk
    /// ([`Store::cleaning_due`]): the least live first, a pass took segments whose freed data, a
    /// guest's zeros, took next to none of it, and left cleaning due for the others, pass after
    /// pass.
    fn cleanable(&self, live_fraction: f64, most: u64) -> Result<Cleanable, Error> {
        let mut state = self.writable()?;
        self.unshare(&mut state);
        let state = &*state;
        let shared = |segment| self.lineage.shared(self.branch, FileKey::Segment(segment));
        let victims = state.segments.victims(live_fraction, shared);
        let mut most_back: Vec<(&SegmentId, &Victim)> = victims.iter().collect();
        most_back.sort_by(|(number, victim), (other_number, other)| {
            victim.cmp_cost(other).then(number.cmp(other_number))
        });
        let copying: HashSet<ExtentId> =
            state.uncommitted.copies().iter().map(|copied| copied.extent).collect();
        let (mut copies, mut bytes) = (Vec::new(), 0);
        'victims: for (number, _) in most_back {
            let mut sealed: Vec<&ExtentId> = (state.segments.sealed_in(*number))
                .filter(|extent| !copying.contains(extent))
                .collect();
            sealed.sort();
            for extent in sealed {
                let entry =
                    state.extents.get(extent).expect("an extent sealed in a segment is held");
                debug_assert!(
                    matches!(&entry.data, Data::Pieces(pieces)
                        if pieces.iter().any(|piece| piece.segment == *number)),
                    "{extent:?}, sealed in {number:?}, is not"
                );
                if bytes + entry.len > most {
                    break 'victims;
                }
                bytes += entry.len;
                copies.push((*extent, entry.clone()));
            }
        }
        Ok(Cleanable { victims, copies })
    }

    /// fxdax: copies `extent`'s data, sealed, as `entry` places it, to the open segment, as a write
    /// in progress, which commits wait for: the next commit makes the copy its place, if it is as
    /// it was, and the copy in the open segment (TASK-79.2). Returns the bytes copied: none if it
    /// was freed, or moved, meanwhile.
    fn copy_out(&self, extent: ExtentId, entry: Extent) -> Result<u64, Error> {
        let len = entry.len;
        let mut data = Aligned::new(len as usize);
        match self.read_into(extent, 0, data.as_mut_slice()) {
            Ok(()) => {}
            Err(error) if error.downcast_ref::<NoExtent>().is_some() => return Ok(0),
            Err(error) => return Err(error),
        }
        let mut state = self.writable()?;
        // Not while a commit, or a closing between commits, waits for the writes in progress.
        while state.gate.is_held() {
            state = self.idle.wait(state).unwrap();
        }
        let mut state = self.still_writable(state)?;
        if state.extents.get(&extent) != Some(&entry) {
            return Ok(0);
        }
        self.make_room(&mut state, len)?;
        let (number, file, at) = state.segments.reserve(len);
        let gated = rules().commits_wait_for_copies;
        if gated {
            state.gate.pass();
        }
        drop(state);
        let written = file.write_at(data.as_slice(), at);
        let mut state = self.state.lock().unwrap();
        let state = &mut *state;
        let idle = match written {
            Ok(()) => {
                let piece = Piece { segment: number, offset: at, extent_offset: 0, len };
                state.uncommitted.copied(Copied { extent, entry, piece });
                state.segments.held(number, len)
            }
            Err(_) => state.segments.dropped(number, len),
        };
        if idle {
            self.idle.notify_all();
        }
        if gated {
            self.written(state);
        }
        written.context("copying an extent")?;
        Ok(len)
    }

    /// fxdax: makes cleaning's copies since the last commit, for the commit, their extents'
    /// places: those of extents as they were, in the open segment, whose footer names them there;
    /// what the others copied is dead space (TASK-79.2).
    fn make_moves(&self, state: &mut State<D::File>) {
        let open = state.segments.open_segment();
        for Copied { extent, entry, piece } in state.uncommitted.take_copies() {
            let placed = piece.segment == open || !rules().moves_into_the_open_segment;
            let current = state.extents.get_mut(&extent).filter(|current| **current == entry);
            let Some(current) = current.filter(|_| placed) else {
                state.segments.let_go(&piece);
                continue;
            };
            current.data = Data::Pieces(vec![piece]);
            state.segments.displace(extent, &entry.data);
            state.segments.place(extent, &current.data);
            let Data::Pieces(old) = &entry.data else { unreachable!("copies are of segments'") };
            for old in old {
                state.segments.let_go(old);
            }
            state.uncommitted.push(Change::Move { extent, pieces: vec![piece] });
        }
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

    /// Sets the most bytes the branch's extents may hold, durably: the branch's record holds it
    /// from now on. It may grow it, or shrink it no lower than what the extents hold, with what
    /// writes in progress may add ([`TooSmall`]).
    pub fn set_capacity(&self, capacity: u64) -> Result<(), Error> {
        let mut state = self.writable()?;
        let block_size = self.lineage.block_size as u64;
        ensure!(capacity >= block_size, "a capacity of {capacity} is less than a block");
        state.space.check_capacity(capacity)?;
        // Held while the record changes, so that no write grows past the capacity meanwhile.
        self.lineage.set_capacity(self.branch, self.token, capacity)?;
        state.space.set_capacity(capacity);
        self.changed(&state);
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

    /// fxdax: what the branch holds in memory of its own (TASK-31).
    pub fn footprint(&self) -> Result<Footprint, Error> {
        let state = self.state()?;
        Ok(Footprint { entries: state.extents.own_entries(), segments: state.segments.tracked() })
    }

    /// The last epoch committed.
    pub fn epoch(&self) -> Result<u64, Error> {
        Ok(self.state()?.epoch)
    }
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

        /// fxdax: cleaning behind the commits takes not the commits' lock, which a commit holds as
        /// it seals, and closes segments: a guest's commits wait for it no longer than for a
        /// write, as it copies one extent at a time (TASK-79.2). Under Shuttle, a wait that can't
        /// end is a deadlock it reports: the pass completes while this thread holds the lock, and
        /// the next commit makes its copy the extent's place.
        #[test]
        fn cleaning_takes_not_the_commits_lock() {
            let mut config = shuttle::Config::new();
            config.stack_size = 4 << 20;
            let scheduler = shuttle::scheduler::RandomScheduler::new(1);
            shuttle::Runner::new(scheduler, config).run(|| {
                let description = Description { block_size: 4096, capacity: 1 << 20, branch: 1 };
                let options = Options { segment_size: 4 * 4096, ..Options::default() };
                let store = Store::create(crate::MemDir::new(), description, options).unwrap();
                for sequence in 0..8 {
                    store.write(ExtentId::new(1, sequence), 0, &[sequence as u8; 4096]).unwrap();
                }
                store.commit(None).unwrap();
                for sequence in 1..8 {
                    store.free(ExtentId::new(1, sequence)).unwrap();
                }
                store.commit(None).unwrap();
                let committing = store.committing.lock().unwrap();
                assert_eq!(store.clean_behind(0.5, u64::MAX).unwrap(), 4096);
                drop(committing);
                store.commit(None).unwrap();
                // The segment cleaned gone: the open one is left.
                assert_eq!(store.usage().unwrap().segments, 1);
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
                let framed = dir.list(&branch_dir(SEGMENTS, 1)).unwrap();
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
