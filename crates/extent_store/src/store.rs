// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store. See the crate's docs.

use crate::dir::{Dir, File, not_found};
use crate::durability::{Batch, Durability, Syncs};
use crate::framed::{Aligned, Framed, Packed, Part, Source, read_aligned, read_frame};
use crate::index::{Recovered, rebuild, recover, write_checkpoint};
use crate::kept::{Kept, KeptExtents};
use crate::layers::{Extents, Layer};
use crate::layout::{
    BranchRecord, Change, Checkpoint, Data, Delta, Description, EXTENTS, Extent, INDEX,
    OPEN_SEGMENTS, Origin, Piece, Place, Record, SEGMENTS, SegmentId, branch_dir, encode,
    extent_path, index_path, open_extent_path, open_segment_path, segment_path,
};
use crate::lineage::{FileKey, Lineage, create_branch_dirs, dropped, files_of, list_numbers};
use crate::segments::{Framing, Window, close_left_open, footer_last, frame_segment};
use crate::sync::{Condvar, Mutex, MutexGuard, rules};
use crate::uploads::{Closed, Committed, Footing, Snapshot, Uploads};
use crate::views::{Keeping, Limits, NotHeld, View, Views};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Footer, Location, extent_key, tag};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
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

/// How a store lays its extents out.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The size past which a segment takes no more extents: the next commit closes it, and starts
    /// another.
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

impl Default for Options {
    fn default() -> Self {
        Self {
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

/// Whom a read is for: the store's own, of what it holds; the uploader's, of what it holds or
/// keeps; or a view's, as of the commit of `epoch`, of what that commit and those since `since`
/// held, which the store holds or keeps, and of what it pins, if `pinned`: decided in the critical
/// section that locates it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    Store,
    Uploader,
    View { epoch: u64, since: u64, pinned: bool },
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
    /// [`Store::close_now`] and the writes it holds up wait for.
    idle: Condvar,
    /// fxdax: its views, which followers read it through ([`Store::view`]).
    views: Arc<Views>,
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
    /// Its record, at `offset` in the index file `index`: a new one, with a checkpoint, if
    /// `new_index`.
    record: Vec<u8>,
    index: Arc<F>,
    offset: u64,
    new_index: bool,
    /// The files it made unreferenced, which go once it is durable.
    doomed: Vec<String>,
    /// fxdax: the segments it closed, being framed, whose framed files it waits for and syncs.
    framing: Vec<(SegmentId, Arc<Framing<F>>)>,
}

struct State<F> {
    /// The most bytes its extents may hold, as its record says ([`Store::set_capacity`]).
    capacity: u64,
    /// Its extents: over the state it shares with its parent at the fork and its forks, if any
    /// (TASK-31).
    extents: Extents,
    /// The segments it tracks: those its extents' data is in, its own, taking writes or not, and
    /// those it inherited that it holds otherwise than the state it shares does, having let
    /// extents in them go. It holds the others, untracked, as that state does. Its own segments'
    /// files it keeps open itself; those of segments it inherited are the lineage's, which its
    /// branches share ([`Lineage::segment_file`]).
    segments: BTreeMap<SegmentId, Segment<F>>,
    /// The segment taking writes.
    open_segment: SegmentId,
    /// The bytes extents hold.
    stored: u64,
    /// fxdax: of those, the bytes of the extents its branch wrote (TASK-49).
    own: u64,
    /// The bytes writes in progress may add.
    reserved: u64,
    next_sequence: u64,
    /// The root as of the last commit.
    root: Vec<u8>,
    /// The last epoch committed.
    epoch: u64,
    /// The changes since the last commit, which the next records.
    changes: Vec<Change>,
    /// The extents written since the last commit, which are open, with where their data goes.
    open: HashMap<ExtentId, Placement<F>>,
    /// The files of extents stored on their own, sealed, open.
    own_files: HashMap<ExtentId, Arc<Framed<F>>>,
    /// The bytes the files of extents stored on their own take.
    own_sizes: HashMap<ExtentId, u64>,
    /// What the next commit syncs: the segments and extents' files written, and the directories
    /// whose entries changed, since the last.
    dirty_segments: BTreeSet<SegmentId>,
    dirty_own: HashMap<ExtentId, Arc<F>>,
    dirty_dirs: BTreeSet<String>,
    /// fxdax: the segments closed since the last commit, being framed, which its flush waits for.
    framing: Vec<(SegmentId, Arc<Framing<F>>)>,
    /// The files the next commit removes, once it is durable.
    doomed: Vec<String>,
    /// The files of extents' data the next commit lets go: no extent of the branch is in them.
    released: Vec<FileKey>,
    /// The index's file the next commit appends to.
    index: IndexFile<F>,
    /// The branch's index files replaced, which forks not yet opened need still.
    retained: BTreeSet<u64>,
    /// What the commits since the open segment opened changed, which its footer records.
    window: Window,
    /// When the open segment opened.
    opened: Instant,
    /// Whether the next commit closes the open segment, whatever it holds.
    close_wanted: bool,
    /// The epoch of the branch's newest checkpoint, in its index.
    checkpointed: u64,
    /// The branch's closed segments no extent is in any more, with the last epochs their footers
    /// cover and their sizes, which go once a checkpoint covers them.
    uncovered: Vec<(SegmentId, u64, u64)>,
    /// The segments' files open.
    open_files: usize,
    /// Whether a commit, or the open segment's closing between commits ([`Store::close_now`]),
    /// waits for the writes in progress: new ones wait.
    closing: bool,
    /// The writes in progress, to segments or extents' own files, which a commit waits for: the
    /// extents it seals hold what they write.
    writing: u64,
    /// fxdax: who the closed segments are for, if a writer of the branch in object storage.
    footing: Option<Footing>,
    /// The number and epoch, in their footers, of the last segment closed since the branch was
    /// opened, which the next follows.
    last_closed: Option<(u64, u64)>,
    /// Whether the next commit writes a checkpoint: it adopted extents, which footers don't name.
    checkpoint_wanted: bool,
    /// Whether a commit failed, after which the store's state may differ from what its files say,
    /// so it does nothing more.
    failed: bool,
    /// Whether it was closed ([`Store::close`]): it reads what it holds, and changes nothing.
    closed: bool,
    /// fxdax: what an uploader takes, if one does.
    uploads: Option<Uploads>,
    /// fxdax: the extents freed whose data stays.
    kept: KeptExtents,
    /// fxdax: the bytes of them views keep, which count as used.
    kept_for_views: u64,
    /// fxdax: the epochs of the commits that sealed extents, held or kept, since the oldest live
    /// view's: a view refuses those sealed after its own, which its commit doesn't hold. Those
    /// sealed by then, or while no view lived, are sealed as of every view.
    sealed: HashMap<ExtentId, u64>,
}

struct Segment<F> {
    /// Its file, if open: raw while the segment is open, framed once it is closed.
    file: Option<Source<F>>,
    /// The bytes of its data, if it is the branch's own.
    len: u64,
    /// The bytes its file takes, if it is the branch's own.
    size: u64,
    /// The bytes of extents' data it holds: pieces of extents not freed.
    live: u64,
    /// The writes to it in progress.
    writers: u64,
    /// The last epoch of its footer, if the branch closed it since it was opened.
    closed: Option<u64>,
    /// fxdax: its framing, closed, until the flush of the commit that closed it has its framed
    /// file in place of its raw one, which it reads meanwhile.
    framing: Option<Arc<Framing<F>>>,
}

impl<F> Segment<F> {
    /// A segment, not open, holding `live` bytes of extents' data.
    fn new(live: u64) -> Self {
        Self { file: None, len: 0, size: 0, live, writers: 0, closed: None, framing: None }
    }
}

enum Placement<F> {
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

struct IndexFile<F> {
    number: u64,
    file: Arc<F>,
    len: u64,
    /// The bytes of deltas it holds.
    deltas: u64,
}

/// A write in progress (see [`Store::begin_write`]): its data goes to `file()` at `offset()`.
/// Dropping it before it is finished abandons it.
pub struct Write<'a, D: Dir> {
    store: &'a Store<D>,
    extent: ExtentId,
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
                // A branch of its own, which has minted nothing, committed nothing, and changed
                // nothing over its parent's state at the fork.
                let state = Recovered {
                    epoch: 0,
                    next_sequence: 0,
                    root: origin.root().to_vec(),
                    segment: 0,
                    extents: Extents::over(Some(origin.clone())),
                };
                create_branch_dirs(dir, branch)?;
                // What an earlier opening wrote, before it could record that it had.
                for number in list_numbers(dir, &branch_dir(INDEX, branch))? {
                    dir.remove(&index_path(branch, number))?;
                }
                write_checkpoint(dir, branch, 0, &state, Some(origin))?;
                dir.barrier()?;
                lineage.resolved(branch)?;
                state
            }
            (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
        };
        let Recovered { epoch, next_sequence, root, extents, .. } = recovered;
        // Footers of commits not recovered, which were not durable, are no footers: the commits
        // after this epoch are others.
        close_left_open(dir, branch, epoch)?;

        // The segments it tracks: those of the extents it holds over its origin, and of its
        // origin's it hides, which it holds otherwise than its origin does; and its own extents'
        // files. They must be there. Of the rest, it holds what its origin does.
        let base_live = |segment| origin.as_ref().map_or(0, |origin| origin.live(segment));
        let mut segments: BTreeMap<SegmentId, Segment<D::File>> = BTreeMap::new();
        let mut stored = origin.as_ref().map_or(0, |origin| origin.stored());
        let mut own_sizes = HashMap::new();
        for (_, entry) in extents.shadowed() {
            stored -= entry.len;
            if let Data::Pieces(pieces) = &entry.data {
                for piece in pieces {
                    let segment = segments
                        .entry(piece.segment)
                        .or_insert_with(|| Segment::new(base_live(piece.segment)));
                    segment.live -= piece.len;
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
                        let segment = segments
                            .entry(piece.segment)
                            .or_insert_with(|| Segment::new(base_live(piece.segment)));
                        segment.live += piece.len;
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
        segments.retain(|_, segment| segment.live > 0);
        for (id, segment) in &mut segments {
            let path = segment_path(*id);
            let file = dir.open(&path, true).with_context(|| format!("segment {path}"))?;
            if id.branch == branch {
                let (parts, _) = read_frame(&file)?
                    .ok_or_else(|| anyhow!("{path} is not a whole framed file"))?;
                segment.len = parts.frame.data_len;
                segment.size = parts.frame.size();
            }
        }

        // Writes go to a new segment, past whatever the branch's last one holds.
        let own = list_numbers(dir, &branch_dir(SEGMENTS, branch))?;
        let open_segment = SegmentId { branch, number: own.last().map_or(0, |last| last + 1) };
        let file = Arc::new(dir.create(&open_segment_path(open_segment), true)?);
        let mut segment = Segment::new(0);
        segment.file = Some(Source::Raw(file));
        segments.insert(open_segment, segment);

        // A new index file, with a checkpoint of what was recovered, which replaces the others but
        // those forks need.
        let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
        let index_number = numbers.last().map_or(0, |number| number + 1);
        let segment = open_segment.number;
        let state = Recovered { epoch, next_sequence, root, segment, extents };
        let (index_file, len) =
            write_checkpoint(dir, branch, index_number, &state, origin.as_ref())?;
        dir.sync_dir(&branch_dir(OPEN_SEGMENTS, branch))?;
        dir.barrier()?;
        let pinned = lineage.pinned(branch);
        let mut retained = BTreeSet::new();
        for number in numbers {
            if pinned.contains(&number) {
                retained.insert(number);
            } else {
                dir.remove(&index_path(branch, number))?;
            }
        }
        let Recovered { root, mut extents, .. } = state;
        let mut files = files_of(extents.changed());
        files.insert(FileKey::Segment(open_segment));
        lineage.reset(branch, epoch, &files, &dropped(&extents));
        // A branch whose index holds all it holds shares in memory what its forks inherited of it,
        // the latest of it, holding over that what it changed since.
        if origin.is_none()
            && let Some(layer) = lineage.latest_layer_of(branch)
        {
            extents.rebase(layer);
        }

        let options = lineage.options;
        let views = Views::new(epoch, &root);
        Ok(Self {
            lineage,
            branch,
            token,
            parent: record.origin.map(|origin| origin.parent),
            origin,
            options,
            state: Mutex::new(State {
                capacity: record.capacity,
                extents,
                segments,
                open_segment,
                stored,
                own: own_stored,
                reserved: 0,
                next_sequence,
                root,
                epoch,
                changes: Vec::new(),
                open: HashMap::new(),
                own_files: HashMap::new(),
                own_sizes,
                dirty_segments: BTreeSet::new(),
                dirty_own: HashMap::new(),
                dirty_dirs: BTreeSet::new(),
                framing: Vec::new(),
                doomed: Vec::new(),
                released: Vec::new(),
                index: IndexFile {
                    number: index_number,
                    file: Arc::new(index_file),
                    len,
                    deltas: 0,
                },
                retained,
                window: Window::starting(epoch + 1),
                opened: Instant::now(),
                close_wanted: false,
                checkpointed: epoch,
                uncovered: Vec::new(),
                open_files: 1,
                closing: false,
                writing: 0,
                footing: None,
                last_closed: None,
                checkpoint_wanted: false,
                failed: false,
                closed: false,
                uploads: None,
                kept: KeptExtents::default(),
                kept_for_views: 0,
                sealed: HashMap::new(),
            }),
            committing: Mutex::new(()),
            durability: Mutex::new(Durability::new(epoch)),
            flushing: Mutex::new(()),
            idle: Condvar::new(),
            views,
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
    /// goes on over it, and with the branch's other forks of the same point (TASK-31).
    pub fn fork(&self, child: u64) -> Result<(), Error> {
        let committing = self.committing.lock().unwrap();
        // At the end of its segment, which the commit closes, and whose footer names it (doc-7,
        // Forks).
        {
            let mut state = self.writable()?;
            state.close_wanted = true;
            state.window.forks.push(child);
        }
        let forked = self.commit_locked(&committing, None, true)?;
        let Forked { layer, origin } = forked.expect("a fork's commit freezes the branch's state");
        self.flush()?;
        let capacity = self.state()?.capacity;
        self.lineage.fork(origin, child, capacity, layer)
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
            capacity: self.state.lock().unwrap().capacity,
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
        let state = self.state()?;
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
            state.closed = true;
            drop(state);
            self.lineage.closed(self.branch, self.token);
        }
        flushed
    }

    fn check_io(&self, offset: u64, len: u64) -> Result<(), Error> {
        let block_size = self.lineage.block_size as u64;
        if !offset.is_multiple_of(block_size) || !len.is_multiple_of(block_size) || len == 0 {
            bail!(Unaligned { offset, len });
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
        let segment =
            state.segments.get(&number).ok_or_else(|| anyhow!("no segment {number:?}"))?;
        if let Some(file) = &segment.file {
            return Ok(file.clone());
        }
        // Only closed segments' files are closed.
        let path = segment_path(number);
        let framed = Framed::open(self.lineage.dir.open(&path, true)?, &path)?;
        let file = Source::Framed(Arc::new(framed));
        state.segments.get_mut(&number).unwrap().file = Some(file.clone());
        state.open_files += 1;
        self.close_idle_files(state);
        Ok(file)
    }

    /// Tracks the segments of `entry`'s data, as the branch lets it go: those it inherited and held
    /// as the state it shares holds them, it holds otherwise from now on.
    fn track_segments(state: &mut State<D::File>, entry: &Extent) {
        let Data::Pieces(pieces) = &entry.data else { return };
        for piece in pieces {
            if state.segments.contains_key(&piece.segment) {
                continue;
            }
            let live = state.extents.base().map_or(0, |base| base.live(piece.segment));
            state.segments.insert(piece.segment, Segment::new(live));
        }
    }

    /// Closes the closed segments' files no read is using, nor the next commit syncs, if more are
    /// open than the options say: as a segment's file opens, or a commit closes a segment.
    fn close_idle_files(&self, state: &mut State<D::File>) {
        if state.open_files <= self.options.open_files {
            return;
        }
        let (open_segment, dirty) = (state.open_segment, &state.dirty_segments);
        let mut closed = 0;
        for (number, segment) in &mut state.segments {
            if *number != open_segment
                && !dirty.contains(number)
                && segment.file.as_ref().is_some_and(|file| Arc::strong_count(file.file()) == 1)
            {
                segment.file = None;
                closed += 1;
            }
        }
        state.open_files -= closed;
    }

    /// The open segment's file, raw.
    fn open_segment_file(&self, state: &mut State<D::File>) -> Result<Arc<D::File>, Error> {
        match self.segment_file(state, state.open_segment)? {
            Source::Raw(file) => Ok(file),
            Source::Framed(_) => bail!("the open segment is framed"),
        }
    }

    /// `extent`'s own file, sealed, opened if it isn't.
    fn own_file(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
    ) -> Result<Arc<Framed<D::File>>, Error> {
        if let Some(file) = state.own_files.get(&extent) {
            return Ok(file.clone());
        }
        let path = extent_path(extent);
        let file = Arc::new(Framed::open(self.lineage.dir.open(&path, true)?, &path)?);
        if state.own_files.len() >= self.options.open_files {
            state.own_files.retain(|_, file| Arc::strong_count(&file.file) > 1);
        }
        state.own_files.insert(extent, file.clone());
        Ok(file)
    }

    /// The file `extent`'s data is in, stored on its own: raw while it is open, framed once sealed.
    fn own_source(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
    ) -> Result<Source<D::File>, Error> {
        match state.open.get(&extent) {
            Some(Placement::Own(file)) => Ok(Source::Raw(file.clone())),
            _ => Ok(Source::Framed(self.own_file(state, extent)?)),
        }
    }

    /// Whether a commit, or a closing between commits, waits for the writes in progress, holding
    /// new ones back ([`Store::begin_write`]).
    pub fn holding_writes(&self) -> bool {
        self.state.lock().unwrap().closing
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
        while state.closing {
            state = self.idle.wait(state).unwrap();
        }
        ensure!(!state.failed, "the extent store failed");
        let current = state.extents.get(&extent).map(|entry| entry.len);
        if current.is_some() && !state.open.contains_key(&extent) {
            bail!(SealedExtent(extent));
        }
        let end = offset + len;
        let grows = end.saturating_sub(current.unwrap_or(0));
        if state.stored + state.kept_for_views + state.reserved + grows > state.capacity {
            bail!(NoSpace);
        }
        let placement = match state.open.get(&extent) {
            Some(placement) => placement.clone(),
            None => {
                let placement = if end >= self.options.large_extent {
                    // Raw until it is sealed, when it is framed in its own file.
                    let path = open_extent_path(extent);
                    let file = self
                        .lineage
                        .dir
                        .create(&path, true)
                        .with_context(|| format!("creating {path}"))?;
                    Placement::Own(Arc::new(file))
                } else {
                    Placement::Segment
                };
                state.open.insert(extent, placement.clone());
                placement
            }
        };
        let (file, file_offset, segment) = match placement {
            Placement::Own(file) => (file, offset, None),
            Placement::Segment => {
                let number = state.open_segment;
                let file = self.open_segment_file(&mut state)?;
                let segment = state.segments.get_mut(&number).unwrap();
                let at = segment.len;
                segment.len += len;
                segment.size += len;
                segment.writers += 1;
                (file, at, Some(number))
            }
        };
        state.reserved += grows;
        state.writing += 1;
        Ok(Write {
            store: self,
            extent,
            extent_offset: offset,
            len,
            file,
            offset: file_offset,
            segment,
            grows,
            finished: false,
        })
    }

    fn finish_write(&self, write: &Write<'_, D>) {
        let mut state = self.state.lock().unwrap();
        let state = &mut *state;
        state.reserved -= write.grows;
        let extent = write.extent;
        let end = write.extent_offset + write.len;
        let entry = state.extents.get_or_insert_with(extent, || Extent {
            len: 0,
            data: if write.segment.is_some() { Data::Pieces(Vec::new()) } else { Data::Own },
        });
        if end > entry.len {
            // A write's extent is its branch's.
            state.stored += end - entry.len;
            state.own += end - entry.len;
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
                pieces.push(piece);
                let segment = state.segments.get_mut(&number).unwrap();
                segment.writers -= 1;
                segment.live += write.len;
                if segment.writers == 0 {
                    self.idle.notify_all();
                }
                state.dirty_segments.insert(number);
                state.changes.push(Change::Write { extent, piece });
            }
            // Its file is the branch's to claim once it is sealed, and framed.
            (None, Data::Own) => state.changes.push(Change::WriteOwn { extent, len: entry.len }),
            _ => unreachable!("an extent's data is in segments or its own file"),
        }
        state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        self.written(state);
    }

    fn abandon_write(&self, write: &Write<'_, D>) {
        let mut state = self.state.lock().unwrap();
        state.reserved -= write.grows;
        if let Some(number) = write.segment {
            // What it wrote, if anything, is dead space.
            let segment = state.segments.get_mut(&number).unwrap();
            segment.writers -= 1;
            if segment.writers == 0 {
                self.idle.notify_all();
            }
        }
        self.written(&mut state);
    }

    /// A write in progress is done, finished or abandoned: a commit waiting for the last goes on.
    fn written(&self, state: &mut State<D::File>) {
        state.writing -= 1;
        if state.writing == 0 {
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
        let entry = Self::entry_for(&state, extent, reader)?;
        self.parts_locked(&mut state, extent, &entry, offset, len)
    }

    /// `extent`'s entry, which `reader` may read: of what the store holds, or keeps, if not the
    /// store's own; a view's, of what its commits hold (not open, not sealed after its commit, not
    /// freed before the one it holds since), or it pins. Fails with `NoExtent` otherwise.
    fn entry_for(
        state: &State<D::File>,
        extent: ExtentId,
        reader: Reader,
    ) -> Result<Extent, Error> {
        if let Reader::View { epoch, .. } = reader {
            let later = state.sealed.get(&extent).is_some_and(|sealed| *sealed > epoch);
            if later || state.open.contains_key(&extent) {
                bail!(NoExtent(extent));
            }
        }
        let kept = |kept: &&Kept| match reader {
            Reader::Store => false,
            Reader::Uploader => true,
            // Freed by a commit after the one the view holds since, which held it sealed; or
            // pinned.
            Reader::View { since, pinned, .. } => {
                pinned || (kept.sealed && kept.freed > since) || !rules().views_hold_their_commit
            }
        };
        match (state.extents.get(&extent), state.kept.get(&extent)) {
            (Some(entry), _) => Ok(entry.clone()),
            (None, Some(held)) if kept(&held) => Ok(held.entry.clone()),
            // Kept for another, freed before what the view holds: what it let go of.
            (None, Some(_)) if matches!(reader, Reader::View { .. }) => {
                Err(Error::new(NoExtent(extent)).context(NotHeld(extent)))
            }
            _ => bail!(NoExtent(extent)),
        }
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
                    source.locate(offset, held_end - offset, unpacked, &mut parts);
                }
            }
            Data::Remote => bail!(RemoteData(extent)),
            Data::Pieces(pieces) => {
                for (start, end, source) in paint(pieces, offset, held_end) {
                    match source {
                        None => parts.push(Part::Zeros(end - start)),
                        Some((number, at)) => {
                            let source = self.segment_file(state, number)?;
                            source.locate(at, end - start, unpacked, &mut parts);
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
        self.unpack(part, &stored)
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
        let placement = state.open.remove(&extent);
        if let Some(Placement::Own(_)) = placement {
            // Its raw file, which it is not sealed from, or a write abandoned created.
            state.doomed.push(open_extent_path(extent));
        }
        let Some(entry) = state.extents.remove(&extent) else { return Ok(()) };
        state.stored -= entry.len;
        if extent.branch == self.branch {
            state.own -= entry.len;
        }
        state.changes.push(Change::Free(extent));
        Self::track_segments(state, &entry);
        // Its data stays until the uploader has what it needs of it, and, if it was sealed, until
        // the commit that frees it decides whether views keep it.
        let uploader = state.uploads.is_some() && rules().keep_frees;
        let sealed = placement.is_none();
        if uploader || sealed {
            let freed = state.epoch + 1;
            state.kept.insert(extent, Kept { entry, freed, uploader, sealed });
        } else {
            Self::forget(state, extent, entry);
        }
        Ok(())
    }

    /// Lets the kept extents freed by commits up to `published`, the last published to views, go
    /// that no one needs any more: neither the uploader, nor, if sealed, views, as `keeping` says.
    /// Those freed since are for the commit that frees them to decide: a view may yet open as of
    /// the last.
    fn trim_kept(state: &mut State<D::File>, keeping: &Keeping, published: u64) {
        for (extent, kept) in state.kept.trim(keeping, published) {
            Self::forget(state, extent, kept.entry);
        }
        state.kept_for_views = state.kept.views_bytes(keeping);
    }

    /// The bytes of the kept extents views would keep, as `keeping` says.
    fn kept_bytes(state: &State<D::File>, keeping: &Keeping) -> u64 {
        state.kept.views_bytes(keeping)
    }

    /// Lets the data of `extent`, freed, go.
    fn forget(state: &mut State<D::File>, extent: ExtentId, entry: Extent) {
        state.sealed.remove(&extent);
        match entry.data {
            Data::Pieces(pieces) => {
                for piece in pieces {
                    state.segments.get_mut(&piece.segment).unwrap().live -= piece.len;
                }
            }
            Data::Own => {
                state.own_files.remove(&extent);
                state.own_sizes.remove(&extent);
                state.dirty_own.remove(&extent);
                state.released.push(FileKey::Own(extent));
            }
            Data::Remote => {}
        }
    }

    /// fxdax: holds `extent`, sealed, of `len` bytes, whose data is elsewhere: reads of it fail
    /// with [`RemoteData`], and the store's user reads it from there. A branch kept in object
    /// storage, opened where its data is not, holds its extents so.
    pub fn adopt(&self, extent: ExtentId, len: u64) -> Result<(), Error> {
        let mut state = self.writable()?;
        ensure!(!state.extents.contains_key(&extent), "the store holds {extent:?}");
        state.extents.insert(extent, Extent { len, data: Data::Remote });
        state.stored += len;
        if extent.branch == self.branch {
            state.own += len;
            state.next_sequence = state.next_sequence.max(extent.sequence + 1);
        }
        state.changes.push(Change::Adopt { extent, len });
        // Footers name no extent whose data is elsewhere: a checkpoint does.
        state.checkpoint_wanted = true;
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

    /// fxdax: from now on, keeps for an uploader what each commit changes ([`Store::committed`]),
    /// the data of the extents it frees, until released ([`Store::release`]), and the segments it
    /// closes, for `footing`'s writer, until uploaded ([`Store::closed`], [`Store::uploaded`]):
    /// unless a commit came since `epoch`, the snapshot's the uploader starts from. Returns whether
    /// it does.
    pub fn upload(&self, footing: Footing, epoch: u64) -> Result<bool, Error> {
        let mut state = self.state()?;
        if state.epoch != epoch {
            return Ok(false);
        }
        state.uploads.get_or_insert_with(Uploads::default);
        state.footing = Some(footing);
        Ok(true)
    }

    /// fxdax: the commits made durable since the last call, oldest first.
    pub fn committed(&self) -> Result<Vec<Committed>, Error> {
        let mut state = self.state()?;
        let Some(uploads) = &mut state.uploads else { return Ok(Vec::new()) };
        let durable = self.uploadable();
        let mut committed = Vec::new();
        while uploads.committed.front().is_some_and(|front| front.epoch <= durable) {
            committed.push(uploads.committed.pop_front().unwrap());
        }
        Ok(committed)
    }

    /// fxdax: lets the data of the extents the commits up to `epoch` freed go: the uploader has
    /// what it needs of them, having taken ([`Store::closed`]) the segments holding those commits,
    /// whose footers may name those extents' own files. Their files go with the commit after.
    pub fn release(&self, epoch: u64) -> Result<(), Error> {
        let mut state = self.state()?;
        let state = &mut *state;
        let Some(uploads) = &mut state.uploads else { return Ok(()) };
        uploads.unreleased.retain(|committed, _| *committed > epoch);
        state.kept.release(epoch);
        Self::trim_kept(state, &self.views.keeping(), state.epoch);
        Ok(())
    }

    /// fxdax: the bytes the commits not yet released wrote: what the uploader has yet to upload.
    pub fn unreleased_bytes(&self) -> Result<u64, Error> {
        let state = self.state()?;
        Ok(state.uploads.as_ref().map_or(0, |uploads| uploads.unreleased.values().sum()))
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
            state.closing = true;
            while state.writing > 0 {
                state = self.idle.wait(state).unwrap();
            }
            state.closing = false;
            self.idle.notify_all();
        }
        let state = &mut *state;
        let epoch = state.epoch + 1;
        // The extents it seals, each in one run, or a file of its own if large (doc-7, Segments).
        let sealing: Vec<ExtentId> = state.open.keys().copied().collect();
        for &extent in &sealing {
            if let Err(error) = self.seal(state, extent) {
                state.failed = true;
                return Err(error.context("committing the extent store"));
            }
        }
        // Published to views, which open as of it from now on, deciding at once what of the frees
        // they keep: those of views as of earlier commits (see `Views`).
        let published = root.unwrap_or(&state.root).to_vec();
        let limits =
            Limits { lag: self.options.view_lag, kept_bytes: self.options.view_kept_bytes };
        let keeping = if rules().decide_at_publish {
            let kept = &*state;
            let keeping = self
                .views
                .publish(epoch, &published, limits, |keeping| Self::kept_bytes(kept, keeping));
            Self::trim_kept(state, &keeping, epoch);
            keeping
        } else {
            Self::trim_kept(state, &self.views.keeping(), epoch);
            let kept = &*state;
            self.views.publish(epoch, &published, limits, |keeping| Self::kept_bytes(kept, keeping))
        };
        // What it sealed, as views older than it, if any lives, must refuse; what all of them
        // were sealed by is theirs.
        match keeping.oldest() {
            Some(oldest) if rules().views_refuse_later_extents => {
                state.sealed.retain(|_, sealed| *sealed > oldest);
                state.sealed.extend(sealing.iter().map(|extent| (*extent, epoch)));
            }
            _ => state.sealed.clear(),
        }
        let changes = std::mem::take(&mut state.changes);
        state.open.clear();
        if let Some(uploads) = &mut state.uploads {
            let mut committed =
                Committed { epoch, root: root.map(<[u8]>::to_vec), ..Committed::default() };
            for change in &changes {
                match change {
                    Change::Write { extent, .. } | Change::WriteOwn { extent, .. } => {
                        // Freed since, if not held: the commit frees it too.
                        let len = state.extents.get(extent).map_or(0, |entry| entry.len);
                        match committed.written.last_mut() {
                            Some((last, last_len)) if last == extent => *last_len = len,
                            _ => committed.written.push((*extent, len)),
                        }
                    }
                    Change::Free(extent) => committed.freed.push(*extent),
                    Change::Move { .. } | Change::MoveOwn { .. } | Change::Adopt { .. } => {}
                }
            }
            let bytes = committed.written.iter().map(|(_, len)| len).sum();
            uploads.unreleased.insert(epoch, bytes);
            uploads.committed.push_back(committed);
        }
        state.window.commit(&changes, root, &state.extents);
        // The open segment closes, its footer after its data, once no write is in progress in it:
        // full, old, wanted closed, or at a fork. Writes go to a new one.
        let open = &state.segments[&state.open_segment];
        let old = open.len > 0 && state.opened.elapsed() >= self.options.segment_age;
        if open.writers == 0 && (open.len >= self.options.segment_size || old || state.close_wanted)
        {
            if let Err(error) = self.close_open_segment(state, epoch, false) {
                state.failed = true;
                return Err(error.context("committing the extent store"));
            }
        }
        let mut syncs = Syncs::default();
        for file in std::mem::take(&mut state.dirty_own).into_values() {
            syncs.file(file);
        }
        // A segment being framed, its framed file, which the flush waits for, rather than its raw.
        for number in std::mem::take(&mut state.dirty_segments) {
            if state.segments.get(&number).is_none_or(|segment| segment.framing.is_none()) {
                syncs.file(self.segment_file(state, number)?.file().clone());
            }
        }
        let framing = std::mem::take(&mut state.framing);
        for dir in std::mem::take(&mut state.dirty_dirs) {
            syncs.dir(dir);
        }
        let mut doomed = std::mem::take(&mut state.doomed);
        let mut released = std::mem::take(&mut state.released);
        // The segments none of its extents' data is in any more, by this commit, if not before:
        // the branch lets them go; those it closed, once a checkpoint covers their footers.
        let dead: Vec<SegmentId> = state
            .segments
            .iter()
            .filter(|(number, segment)| {
                **number != state.open_segment
                    && segment.live == 0
                    && segment.writers == 0
                    && segment.framing.is_none()
                    && !(state.uploads.as_ref())
                        .is_some_and(|uploads| uploads.unuploaded.contains(&number.number))
            })
            .map(|(number, _)| *number)
            .collect();
        for number in dead {
            let segment = state.segments.remove(&number).unwrap();
            if segment.file.is_some() {
                state.open_files -= 1;
            }
            match segment.closed {
                Some(last) if last > state.checkpointed => {
                    state.uncovered.push((number, last, segment.len));
                }
                _ => released.push(FileKey::Segment(number)),
            }
        }
        // A checkpoint once the index holds enough deltas, or the closed segments only footers need
        // take a segment's room.
        let uncovered: u64 = state.uncovered.iter().map(|(_, _, size)| size).sum();
        let checkpoint = state.index.deltas >= self.options.checkpoint_after
            || uncovered >= self.options.segment_size
            || std::mem::take(&mut state.checkpoint_wanted);
        let record = encode(&if checkpoint {
            let (extents, gone) = state.extents.overlay(self.origin.as_ref());
            let table = Checkpoint {
                epoch,
                next_sequence: state.next_sequence,
                root: root.map_or_else(|| state.root.clone(), <[u8]>::to_vec),
                segment: state.open_segment.number,
                extents,
                gone,
            }
            .table();
            match table {
                Ok(table) => Record::Checkpoint(table),
                Err(error) => {
                    state.failed = true;
                    return Err(error.context("committing the extent store"));
                }
            }
        } else {
            Record::Delta(Delta {
                epoch,
                next_sequence: state.next_sequence,
                root: root.map(<[u8]>::to_vec),
                changes,
            })
        });
        // Its place in the index: a new file, which replaces the last once this is durable, or
        // after the records before it.
        let (index, offset) = if checkpoint {
            let number = state.index.number + 1;
            let file = match self.lineage.dir.create(&index_path(self.branch, number), false) {
                Ok(file) => Arc::new(file),
                Err(error) => {
                    state.failed = true;
                    return Err(Error::from(error).context("committing the extent store"));
                }
            };
            // But for what forks need.
            state.retained.insert(state.index.number);
            let pinned = self.lineage.pinned(self.branch);
            let (kept, gone) = std::mem::take(&mut state.retained)
                .into_iter()
                .partition(|number| pinned.contains(number));
            state.retained = kept;
            doomed.extend(gone.into_iter().map(|number| index_path(self.branch, number)));
            state.index =
                IndexFile { number, file: file.clone(), len: record.len() as u64, deltas: 0 };
            // The closed segments its checkpoint covers no longer need their footers.
            state.checkpointed = epoch;
            let (covered, uncovered): (Vec<_>, Vec<_>) = std::mem::take(&mut state.uncovered)
                .into_iter()
                .partition(|(_, last, _)| *last <= epoch);
            state.uncovered = uncovered;
            released.extend(covered.into_iter().map(|(number, ..)| FileKey::Segment(number)));
            (file, 0)
        } else {
            let offset = state.index.len;
            state.index.len += record.len() as u64;
            state.index.deltas += record.len() as u64;
            (state.index.file.clone(), offset)
        };
        self.lineage.release(self.branch, epoch, released);
        state.epoch = epoch;
        if let Some(root) = root {
            state.root = root.to_vec();
        }
        let forked = fork.then(|| Forked {
            layer: state.extents.freeze(self.origin.as_ref(), &state.root),
            origin: Origin {
                parent: self.branch,
                epoch,
                next_sequence: state.next_sequence,
                index: state.index.number,
            },
        });
        // Queued while the state is locked, so that commits queue in their order.
        self.durability.lock().unwrap().push(
            epoch,
            Pending { syncs, record, index, offset, new_index: checkpoint, doomed, framing },
        );
        Ok(forked)
    }

    /// Seals `extent`, which a commit seals, in one run in the open segment, or in a file of its
    /// own, framed, if it is large, copying its data there unless it is.
    fn seal(&self, state: &mut State<D::File>, extent: ExtentId) -> Result<(), Error> {
        let Some(entry) = state.extents.get(&extent).cloned() else { return Ok(()) };
        let large = entry.len >= self.options.large_extent;
        let open = state.open_segment;
        match (&entry.data, state.open.get(&extent)) {
            (Data::Remote, _) => return Ok(()),
            // Written to a raw file of its own, which it is framed from.
            (Data::Own, Some(Placement::Own(raw))) => {
                let raw = raw.clone();
                self.write_own(state, extent, entry.len, |at, buf| raw.read_at(buf, at))?;
                state.doomed.push(open_extent_path(extent));
                return Ok(());
            }
            (Data::Pieces(pieces), _) if !large => {
                let one_run = matches!(pieces[..], [piece] if piece.segment == open
                    && piece.extent_offset == 0
                    && piece.len == entry.len);
                if one_run {
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
            state.segments.get_mut(&piece.segment).unwrap().live -= piece.len;
        }
        let data_now = if large {
            let data = data.as_slice();
            self.write_own(state, extent, entry.len, |at, buf| {
                buf.copy_from_slice(&data[at as usize..at as usize + buf.len()]);
                Ok(())
            })?;
            state.changes.push(Change::MoveOwn { extent });
            Data::Own
        } else {
            let file = self.open_segment_file(state)?;
            let segment = state.segments.get_mut(&open).unwrap();
            let at = segment.len;
            segment.len += entry.len;
            segment.size += entry.len;
            segment.live += entry.len;
            file.write_at(data.as_slice(), at)?;
            state.dirty_segments.insert(open);
            let pieces =
                vec![Piece { segment: open, offset: at, extent_offset: 0, len: entry.len }];
            state.changes.push(Change::Move { extent, pieces: pieces.clone() });
            Data::Pieces(pieces)
        };
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
        state.own_sizes.insert(extent, framed.size());
        state.dirty_own.insert(extent, framed.file.clone());
        state.own_files.insert(extent, framed);
        state.dirty_dirs.insert(branch_dir(EXTENTS, self.branch));
        Ok(())
    }

    /// Closes the open segment, after commit `epoch`: its data framed, with its footer, of what the
    /// commits since it opened changed, in a file of its own, as object storage keeps segments, for
    /// the writer the footing names, if any; and opens another. Framed now if `now`, or else on a
    /// thread of its own ([`Framing`]), which the flush of the commit waits for.
    fn close_open_segment(
        &self,
        state: &mut State<D::File>,
        epoch: u64,
        now: bool,
    ) -> Result<(), Error> {
        let number = state.open_segment;
        let window = std::mem::replace(&mut state.window, Window::starting(epoch + 1));
        let (seq, writer, after, floor) = match &state.footing {
            Some(footing) => {
                (footing.next_seq, footing.epoch, footing.after, footing.next_sequence)
            }
            None => (number.number, 0, state.last_closed, 0),
        };
        let (mut extents, mut own) = (Vec::new(), Vec::new());
        for (extent, entry) in &window.touched {
            match &entry.data {
                Data::Pieces(pieces) => {
                    let [piece] = pieces[..] else { bail!("{extent:?} is not sealed in one run") };
                    ensure!(piece.segment == number, "{extent:?} is sealed in another segment");
                    extents.push((*extent, piece.offset, entry.len));
                }
                Data::Own => {
                    let key = extent_key(writer, *extent);
                    own.push((*extent, Location { key, offset: 0, len: entry.len }));
                }
                // Held elsewhere already: a checkpoint names it.
                Data::Remote => {}
            }
        }
        let footer = Footer {
            branch: self.branch,
            seq,
            epoch: writer,
            after,
            commits: vec![tag(writer, epoch)],
            extents,
            own,
            frees: window.freed.into_iter().collect(),
            root: window.root,
            next_sequence: state.next_sequence.max(floor),
            forks: window.forks,
        };
        if let Some(footing) = &mut state.footing {
            footing.next_seq += 1;
            footing.after = Some((seq, writer));
        }
        state.last_closed = Some((seq, writer));
        if let Some(uploads) = &mut state.uploads {
            uploads.closed.push_back((number.number, footer.clone()));
            uploads.unuploaded.insert(number.number);
        }
        // Its raw file goes once the commit is durable, its data in a framed file of its own.
        let raw = self.open_segment_file(state)?;
        let len = state.segments[&number].len;
        let (path, footer) = (segment_path(number), footer.encode());
        state.segments.get_mut(&number).unwrap().closed = Some(epoch);
        state.dirty_dirs.insert(branch_dir(SEGMENTS, self.branch));
        if now {
            let framed = frame_segment(&self.lineage.dir, &path, &raw, len, &footer)?;
            Self::framed(state, number, framed);
            state.dirty_segments.insert(number);
            state.doomed.push(open_segment_path(number));
        } else {
            let framing = Framing::start(self.lineage.clone(), path, raw, len, footer);
            state.segments.get_mut(&number).unwrap().framing = Some(framing.clone());
            state.framing.push((number, framing));
        }

        let next = SegmentId { number: number.number + 1, ..number };
        let file = Arc::new(self.lineage.dir.create(&open_segment_path(next), true)?);
        self.lineage.claim(self.branch, [FileKey::Segment(next)]);
        state.segments.insert(next, Segment { file: Some(Source::Raw(file)), ..Segment::new(0) });
        state.open_files += 1;
        state.open_segment = next;
        state.opened = Instant::now();
        state.close_wanted = false;
        state.dirty_dirs.insert(branch_dir(OPEN_SEGMENTS, self.branch));
        self.close_idle_files(state);
        Ok(())
    }

    /// fxdax: segment `number`, closed, is in `framed` from now on, its raw file no more.
    fn framed(state: &mut State<D::File>, number: SegmentId, framed: Arc<Framed<D::File>>) {
        let Some(segment) = state.segments.get_mut(&number) else { return };
        segment.size = framed.size();
        segment.framing = None;
        // Unless closed as idle meanwhile: opened again, it opens its framed file.
        if segment.file.is_some() {
            segment.file = Some(Source::Framed(framed));
        }
    }

    /// Has the next commit close the open segment, whatever it holds: for its data to be uploaded
    /// with its footer.
    pub fn close_segment(&self) -> Result<(), Error> {
        self.writable()?.close_wanted = true;
        Ok(())
    }

    /// fxdax: closes the open segment now, between commits, once no write is in progress in it,
    /// if commits made since it opened, or forks: for its uploader to upload what the commits made
    /// so far without waiting for the next. Writes meanwhile wait. Its framed file is durable when
    /// this returns; its raw file is gone. Returns whether it closed.
    pub fn close_now(&self) -> Result<bool, Error> {
        let _committing = self.committing.lock().unwrap();
        let mut state = self.state()?;
        // Closed, it changes nothing: its last segment closes when it is next opened.
        if state.closed || (state.window.first > state.epoch && state.window.forks.is_empty()) {
            return Ok(false);
        }
        state.closing = true;
        while rules().close_waits_for_writers && state.segments[&state.open_segment].writers > 0 {
            state = self.idle.wait(state).unwrap();
        }
        state.closing = false;
        self.idle.notify_all();
        let state = &mut *state;
        let number = state.open_segment;
        let epoch = state.epoch;
        let closed = (|| -> Result<(), Error> {
            self.close_open_segment(state, epoch, true)?;
            self.segment_file(state, number)?.file().sync()?;
            for kind in [SEGMENTS, OPEN_SEGMENTS] {
                self.lineage.dir.sync_dir(&branch_dir(kind, self.branch))?;
            }
            self.lineage.dir.barrier()?;
            Ok(())
        })();
        if let Err(error) = closed {
            state.failed = true;
            return Err(error.context("closing the open segment"));
        }
        // Its data is in its framed file, which opening the branch closes again with no footer
        // if its commits are not durable then.
        let raw = open_segment_path(number);
        state.doomed.retain(|path| *path != raw);
        if let Err(err) = self.lineage.dir.remove(&raw)
            && !not_found(&err)
        {
            log::warn!("extent store: failed to remove {raw}: {err}");
        }
        Ok(true)
    }

    /// fxdax: the segments closed since the last call whose commits are durable, oldest first, for
    /// the uploader, which has them until it says they are uploaded ([`Store::uploaded`]).
    pub fn closed(&self) -> Result<Vec<Closed<D::File>>, Error> {
        let mut state = self.state()?;
        let state = &mut *state;
        let Some(uploads) = &mut state.uploads else { return Ok(Vec::new()) };
        let durable = self.uploadable();
        let mut taken = Vec::new();
        // fxdax: and framed: its framed file is what is uploaded.
        let framed = |number: u64| {
            let segment = state.segments.get(&SegmentId { branch: self.branch, number });
            segment.is_none_or(|segment| segment.framing.is_none())
        };
        while uploads
            .closed
            .front()
            .is_some_and(|(number, footer)| footer_last(footer) <= durable && framed(*number))
        {
            taken.push(uploads.closed.pop_front().unwrap());
        }
        let mut closed = Vec::with_capacity(taken.len());
        for (number, footer) in taken {
            let segment = SegmentId { branch: self.branch, number };
            let file = self.segment_file(state, segment)?.file().clone();
            let mut own = Vec::with_capacity(footer.own.len());
            for (extent, location) in &footer.own {
                own.push((location.key.clone(), self.own_file(state, *extent)?.file.clone()));
            }
            closed.push(Closed { number, footer, file, own });
        }
        Ok(closed)
    }

    /// fxdax: segment `number`, which [`Store::closed`] gave, is uploaded: it may go once no
    /// extent is in it.
    pub fn uploaded(&self, number: u64) -> Result<(), Error> {
        if let Some(uploads) = &mut self.state()?.uploads {
            uploads.unuploaded.remove(&number);
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
            for (number, framing) in std::mem::take(&mut pending.framing) {
                let framed = framing.wait()?;
                pending.syncs.file(framed.file.clone());
                pending.doomed.push(open_segment_path(number));
                Self::framed(&mut self.state.lock().unwrap(), number, framed);
            }
            syncs.extend(std::mem::take(&mut pending.syncs));
        }
        syncs.sync(&self.lineage.dir)?;
        let mut indexes = Vec::new();
        for pending in commits.iter() {
            pending.index.write_at(&pending.record, pending.offset)?;
            if !indexes.iter().any(|index| Arc::ptr_eq(index, &pending.index)) {
                indexes.push(pending.index.clone());
            }
        }
        for index in &indexes {
            index.sync()?;
        }
        if commits.iter().any(|pending| pending.new_index) {
            self.lineage.dir.sync_dir(&branch_dir(INDEX, self.branch))?;
        }
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
    /// sealed to the open segment, and commits, which removes them.
    pub fn clean(&self, live_fraction: f64) -> Result<Cleaned, Error> {
        let committing = self.committing.lock().unwrap();
        let (victims, moves) = {
            let state = self.writable()?;
            let victims: BTreeMap<SegmentId, u64> = state
                .segments
                .iter()
                .filter(|(number, segment)| {
                    number.branch == self.branch
                        && **number != state.open_segment
                        && !self.lineage.shared(self.branch, FileKey::Segment(**number))
                        && segment.writers == 0
                        && segment.len > 0
                        && (segment.live as f64) < live_fraction * segment.len as f64
                })
                .map(|(number, segment)| (*number, segment.len))
                .collect();
            let moves: Vec<(ExtentId, Extent)> = state
                .extents
                .iter()
                .into_iter()
                .filter(|(extent, entry)| {
                    !state.open.contains_key(extent)
                        && matches!(&entry.data, Data::Pieces(pieces)
                            if pieces.iter().any(|piece| victims.contains_key(&piece.segment)))
                })
                .map(|(extent, entry)| (*extent, entry.clone()))
                .collect();
            (victims, moves)
        };
        let mut copied = 0;
        for (extent, entry) in moves {
            let mut data = Aligned::new(entry.len as usize);
            match self.read_into(extent, 0, data.as_mut_slice()) {
                Ok(()) => {}
                // Freed meanwhile.
                Err(error) if error.downcast_ref::<NoExtent>().is_some() => continue,
                Err(error) => return Err(error),
            }
            let (file, number, at) = {
                let mut state = self.state()?;
                let number = state.open_segment;
                let file = self.open_segment_file(&mut state)?;
                let segment = state.segments.get_mut(&number).unwrap();
                let at = segment.len;
                segment.len += entry.len;
                segment.size += entry.len;
                segment.writers += 1;
                (file, number, at)
            };
            let written = file.write_at(data.as_slice(), at);
            let mut state = self.state.lock().unwrap();
            let state = &mut *state;
            state.segments.get_mut(&number).unwrap().writers -= 1;
            written.context("copying an extent")?;
            // Unless freed meanwhile.
            let Some(current) = state.extents.get_mut(&extent) else { continue };
            if *current != entry {
                continue;
            }
            let Data::Pieces(old) = &entry.data else { unreachable!() };
            let piece = Piece { segment: number, offset: at, extent_offset: 0, len: entry.len };
            current.data = Data::Pieces(vec![piece]);
            for old in old {
                state.segments.get_mut(&old.segment).unwrap().live -= old.len;
            }
            state.segments.get_mut(&number).unwrap().live += entry.len;
            state.dirty_segments.insert(number);
            state.changes.push(Change::Move { extent, pieces: vec![piece] });
            copied += entry.len;
        }
        self.commit_locked(&committing, None, false)?;
        // The segments go once the commit moving their extents out is durable.
        self.flush()?;
        let state = self.state()?;
        let removed: Vec<_> =
            victims.iter().filter(|(number, _)| !state.segments.contains_key(number)).collect();
        Ok(Cleaned {
            segments: removed.len() as u64,
            bytes: removed.iter().map(|(_, len)| **len).sum(),
            copied,
        })
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
        let held = state.stored + state.reserved;
        if capacity < held {
            bail!(TooSmall { capacity, held });
        }
        // Held while the record changes, so that no write grows past the capacity meanwhile.
        self.lineage.set_capacity(self.branch, capacity)?;
        state.capacity = capacity;
        Ok(())
    }

    /// The root as of the last commit: empty if none set one.
    pub fn root(&self) -> Result<Vec<u8>, Error> {
        Ok(self.state()?.root.clone())
    }

    pub fn info(&self) -> Result<Info, Error> {
        let state = self.state()?;
        let capacity = state.capacity;
        // What views keep of the frees takes room as the extents did.
        let used = state.stored + state.kept_for_views;
        Ok(Info {
            capacity,
            used,
            available: capacity.saturating_sub(used + state.reserved),
            next_sequence: state.next_sequence,
            own: state.own,
            inherited: state.stored - state.own,
        })
    }

    pub fn usage(&self) -> Result<Usage, Error> {
        let state = self.state()?;
        // The branch's own segments.
        let own = || state.segments.iter().filter(|(id, _)| id.branch == self.branch);
        Ok(Usage {
            segments: own().count() as u64,
            segment_bytes: own().map(|(_, segment)| segment.len).sum(),
            live_bytes: own().map(|(_, segment)| segment.live).sum(),
            own_bytes: (state.extents.iter().into_iter())
                .filter(|(_, entry)| entry.data == Data::Own)
                .map(|(_, entry)| entry.len)
                .sum(),
            disk_bytes: own().map(|(_, segment)| segment.size).sum::<u64>()
                + state.own_sizes.values().sum::<u64>(),
            kept_bytes: state.kept.bytes(),
            kept_for_views: state.kept_for_views,
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

    /// fxdax: cuts the branch's views off: their reads, advances and pins fail with [`CutOff`],
    /// and the commits after keep nothing for them.
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
        Ok(Self::entry_for(&state, extent, self.view_reader(view, extent))?.len)
    }

    /// `view`'s reads of `extent`.
    fn view_reader(&self, view: &View, extent: ExtentId) -> Reader {
        Reader::View { epoch: view.epoch(), since: view.since(), pinned: view.pins(extent) }
    }

    /// `extent`'s length, and whether it is open (written since the last commit), if the store
    /// holds it.
    pub fn extent(&self, extent: ExtentId) -> Result<Option<(u64, bool)>, Error> {
        let state = self.state()?;
        Ok(state.extents.get(&extent).map(|entry| (entry.len, state.open.contains_key(&extent))))
    }

    /// fxdax: whether an extent is open: written since the last commit, which seals it.
    pub fn has_open(&self) -> Result<bool, Error> {
        Ok(!self.state()?.open.is_empty())
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
        Ok(Footprint { entries: state.extents.own_entries(), segments: state.segments.len() })
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
