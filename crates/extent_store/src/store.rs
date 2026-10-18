// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store. See the crate's docs.

use crate::dir::{Dir, File};
use crate::framed::{Aligned, Framed, Packed, Part, Source, read_aligned, read_frame, read_whole};
use crate::layout::{
    BranchRecord, Change, Checkpoint, Data, Delta, Description, EXTENTS, Extent, INDEX,
    OPEN_SEGMENTS, Origin, Piece, Place, Record, SEGMENTS, SegmentId, branch_dir, decode, encode,
    extent_path, index_path, open_extent_path, open_segment_path, segment_path,
};
use crate::lineage::{FileKey, Lineage, create_branch_dirs, files_of, list_numbers};
use crate::sync::{Condvar, Mutex, MutexGuard, rules};
use crate::views::{Keeping, Limits, View, Views};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Footer, Location, epoch_of, extent_key, tag};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
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

/// What cleaning did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cleaned {
    /// The segments removed, and the bytes they took.
    pub segments: u64,
    pub bytes: u64,
    /// The bytes of extents' data copied out of them.
    pub copied: u64,
}

/// fxdax: what a commit changed, kept for an uploader ([`Store::upload`]): the extents it wrote,
/// with their lengths, the extents it freed, and the root, if it set one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    pub epoch: u64,
    pub written: Vec<(ExtentId, u64)>,
    pub freed: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
}

/// fxdax: what a store held when its uploader began: its extents, with their lengths, and root, as
/// of `epoch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub extents: Vec<(ExtentId, u64)>,
    pub root: Vec<u8>,
}

/// fxdax: who the store's closed segments are for, which their footers say: the writer of a
/// branch in object storage, by its lease's epoch, which uploads them as they are (doc-7, decision
/// 1). Its segments are numbered on from `next_seq`, the first following `after`, the segment
/// before it; their footers say the branch's next sequence number is `next_sequence` at least,
/// the writer's floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footing {
    pub epoch: u64,
    pub next_seq: u64,
    pub after: Option<(u64, u64)>,
    pub next_sequence: u64,
}

/// fxdax: a segment the store closed, for its uploader to upload: its footer, its file, framed,
/// and the files of the extents stored on their own its footer names, by their keys.
pub struct Closed<F> {
    /// Its number in the store.
    pub number: u64,
    pub footer: Footer,
    pub file: Arc<F>,
    pub own: Vec<(String, Arc<F>)>,
}

impl<F: File> Closed<F> {
    /// The segment's bytes: the object it is.
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        read_whole(&*self.file)
    }

    /// The bytes of the files of its extents stored on their own, by their keys.
    pub fn own_bytes(&self) -> io::Result<Vec<(String, Vec<u8>)>> {
        self.own.iter().map(|(key, file)| Ok((key.clone(), read_whole(&**file)?))).collect()
    }

    /// The store's epoch of the last commit it holds.
    pub fn last(&self) -> u64 {
        footer_last(&self.footer)
    }
}

/// The store's epoch of the last commit of the segment whose footer is `footer`.
fn footer_last(footer: &Footer) -> u64 {
    footer.commits.last().map_or(0, |tag| epoch_of(*tag))
}

/// fxdax: what a store keeps for its uploader.
#[derive(Default)]
struct Uploads {
    /// The commits ordered and not yet taken, oldest first.
    committed: VecDeque<Committed>,
    /// The last epoch durable.
    durable: u64,
    /// The bytes each commit not yet released wrote.
    unreleased: BTreeMap<u64, u64>,
    /// The segments closed and not yet taken, oldest first, by their numbers, with their footers.
    closed: VecDeque<(u64, Footer)>,
    /// The segments closed and not yet uploaded, which stay until they are.
    unuploaded: BTreeSet<u64>,
}

/// fxdax: an extent freed whose data stays: for the uploader to read, until it releases the commit
/// that freed it, and for views older than that commit, or pinning it ([`Views`]). A free's is
/// kept until the commit that frees it decides.
struct Kept {
    entry: Extent,
    /// The epoch of the commit that freed it.
    freed: u64,
    /// Whether the uploader still needs it.
    uploader: bool,
    /// Whether it was sealed when freed, so that views may hold it.
    sealed: bool,
}

/// Whom a read is for: the store's own, of what it holds; the uploader's, of what it holds or
/// keeps; or a view's, as of the commit of `epoch`, of what that commit holds, which the store
/// holds or keeps, decided in the critical section that locates it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    Store,
    Uploader,
    View(u64),
}

/// A branch of an extent store, open. See the crate's docs.
pub struct Store<D: Dir> {
    lineage: Arc<Lineage<D>>,
    branch: u64,
    /// Its opening of the branch, which closing it ends.
    token: u64,
    /// The branch the branch was forked from, if it is a fork.
    parent: Option<u64>,
    options: Options,
    state: Mutex<State<D::File>>,
    /// Held by a commit, or by cleaning, which commits.
    committing: Mutex<()>,
    /// The commits ordered but not yet durable, oldest first, which [`Store::flush`] makes durable.
    pending: Mutex<VecDeque<Pending<D::File>>>,
    /// Held by a flush: commits become durable in order.
    flushing: Mutex<()>,
    /// Notified when the open segment has no write in progress, and when it closed: what
    /// [`Store::close_now`] and the writes it holds up wait for.
    idle: Condvar,
    /// fxdax: its views, which followers read it through ([`Store::view`]).
    views: Arc<Views>,
}

/// What a commit ordered but not yet durable leaves for the flush that makes it durable.
struct Pending<F> {
    epoch: u64,
    /// The files whose data it refers to, and the directories whose entries changed, to sync.
    syncs: Vec<Arc<F>>,
    dirs: BTreeSet<String>,
    /// Its record, at `offset` in the index file `index`: a new one, with a checkpoint, if
    /// `new_index`.
    record: Vec<u8>,
    index: Arc<F>,
    offset: u64,
    new_index: bool,
    /// The files it made unreferenced, which go once it is durable.
    doomed: Vec<String>,
}

struct State<F> {
    /// The most bytes its extents may hold, as its record says ([`Store::set_capacity`]).
    capacity: u64,
    extents: HashMap<ExtentId, Extent>,
    /// The segments its extents' data is in, its own and those it inherited, and its own taking
    /// writes.
    segments: BTreeMap<SegmentId, Segment<F>>,
    /// The segment taking writes.
    open_segment: SegmentId,
    /// The bytes extents hold.
    stored: u64,
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
    /// Whether the open segment is closing between commits ([`Store::close_now`]): writes wait.
    closing: bool,
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
    kept: HashMap<ExtentId, Kept>,
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
}

/// What the commits since a segment opened changed: the extents they sealed, adopted or moved,
/// which the branch holds still, those they freed, and the root. Applying it to the branch's state
/// as of any of those commits gives its state as of the last.
#[derive(Default)]
struct Window {
    /// Its first commit's epoch.
    first: u64,
    /// The extents its commits sealed, adopted or moved, which the branch holds as of its last, as
    /// they were then: a segment closed between commits holds what they made, not what came after.
    touched: BTreeMap<ExtentId, Extent>,
    freed: BTreeSet<ExtentId>,
    root: Option<Vec<u8>>,
    /// The branches forked at its end.
    forks: Vec<u64>,
}

impl Window {
    fn starting(first: u64) -> Self {
        Self { first, ..Self::default() }
    }

    /// Records what a commit changed, `extents` the branch's extents as of it.
    fn commit(
        &mut self,
        changes: &[Change],
        root: Option<&[u8]>,
        extents: &HashMap<ExtentId, Extent>,
    ) {
        let mut touched = BTreeSet::new();
        for change in changes {
            match change {
                Change::Write { extent, .. }
                | Change::WriteOwn { extent, .. }
                | Change::Adopt { extent, .. }
                | Change::Move { extent, .. }
                | Change::MoveOwn { extent } => {
                    touched.insert(*extent);
                }
                Change::Free(extent) => {
                    touched.remove(extent);
                    self.touched.remove(extent);
                    self.freed.insert(*extent);
                }
            }
        }
        for extent in touched {
            self.touched.insert(extent, extents[&extent].clone());
        }
        if let Some(root) = root {
            self.root = Some(root.to_vec());
        }
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

pub(crate) fn read_file<F: File>(file: &F) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; file.size()? as usize];
    file.read_at(&mut bytes, 0)?;
    Ok(bytes)
}

fn not_found(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound
}

/// Applies `change`, recovering it from an index.
fn apply(extents: &mut HashMap<ExtentId, Extent>, change: Change) {
    match change {
        Change::Write { extent, piece } => {
            let entry =
                extents.entry(extent).or_insert(Extent { len: 0, data: Data::Pieces(Vec::new()) });
            entry.len = entry.len.max(piece.extent_offset + piece.len);
            if let Data::Pieces(pieces) = &mut entry.data {
                pieces.push(piece);
            }
        }
        Change::WriteOwn { extent, len } => {
            let entry = extents.entry(extent).or_insert(Extent { len: 0, data: Data::Own });
            entry.len = entry.len.max(len);
        }
        Change::Free(extent) => {
            extents.remove(&extent);
        }
        Change::Move { extent, pieces } => {
            if let Some(entry) = extents.get_mut(&extent) {
                entry.data = Data::Pieces(pieces);
            }
        }
        Change::MoveOwn { extent } => {
            if let Some(entry) = extents.get_mut(&extent) {
                entry.data = Data::Own;
            }
        }
        Change::Adopt { extent, len } => {
            extents.insert(extent, Extent { len, data: Data::Remote });
        }
    }
}

/// A branch's state, recovered from its index.
#[derive(Debug, Default)]
pub(crate) struct Recovered {
    pub epoch: u64,
    pub next_sequence: u64,
    pub root: Vec<u8>,
    /// The segment open as of its checkpoint, whose footer follows it.
    pub segment: u64,
    pub extents: HashMap<ExtentId, Extent>,
}

impl Recovered {
    /// The state a checkpoint holds.
    fn of(checkpoint: Checkpoint) -> Self {
        Self {
            epoch: checkpoint.epoch,
            next_sequence: checkpoint.next_sequence,
            root: checkpoint.root,
            segment: checkpoint.segment,
            extents: checkpoint.extents.into_iter().collect(),
        }
    }

    /// Applies `delta`, if it follows on, and returns whether it did.
    fn apply(&mut self, delta: Delta) -> bool {
        if delta.epoch != self.epoch + 1 {
            return false;
        }
        for change in delta.changes {
            apply(&mut self.extents, change);
        }
        self.epoch = delta.epoch;
        self.next_sequence = delta.next_sequence;
        if let Some(root) = delta.root {
            self.root = root;
        }
        true
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            epoch: self.epoch,
            next_sequence: self.next_sequence,
            root: self.root.clone(),
            segment: self.segment,
            extents: self.extents.iter().map(|(extent, entry)| (*extent, entry.clone())).collect(),
        }
    }
}

/// `branch`'s state as of its last commit that is durable: its newest index file whose checkpoint
/// is whole, with the deltas after it that are whole and follow on.
pub(crate) fn recover<D: Dir>(dir: &D, branch: u64) -> Result<Recovered, Error> {
    let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
    for &number in numbers.iter().rev() {
        let file = dir.open(&index_path(branch, number), false)?;
        let mut records = decode(&read_file(&file)?).0.into_iter();
        if let Some(Record::Checkpoint(table)) = records.next() {
            let mut recovered = Recovered::of(Checkpoint::of_table(&table)?);
            for record in records {
                let Record::Delta(delta) = record else { break };
                if !recovered.apply(delta) {
                    break;
                }
            }
            return Ok(recovered);
        }
        log::warn!("extent store: branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    }
    bail!("branch {branch:#x} has no index")
}

/// `branch`'s state as of `epoch`, from its index file `number`, which holds it.
pub(crate) fn state_at<D: Dir>(
    dir: &D,
    branch: u64,
    number: u64,
    epoch: u64,
) -> Result<Recovered, Error> {
    let file = dir.open(&index_path(branch, number), false)?;
    let mut records = decode(&read_file(&file)?).0.into_iter();
    let Some(Record::Checkpoint(table)) = records.next() else {
        bail!("branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    };
    let mut recovered = Recovered::of(Checkpoint::of_table(&table)?);
    for record in records {
        if recovered.epoch >= epoch {
            break;
        }
        let Record::Delta(delta) = record else { break };
        if !recovered.apply(delta) {
            break;
        }
    }
    ensure!(
        recovered.epoch == epoch,
        "branch {branch:#x}'s index file {number:#x} holds epoch {}, not {epoch}",
        recovered.epoch
    );
    Ok(recovered)
}

/// Writes `branch`'s index file `number`, of a checkpoint of `state`, durably, and returns it.
pub(crate) fn write_checkpoint<D: Dir>(
    dir: &D,
    branch: u64,
    number: u64,
    state: &Recovered,
) -> Result<(D::File, u64), Error> {
    let bytes = encode(&Record::Checkpoint(state.checkpoint().table()?));
    let file = dir.create(&index_path(branch, number), false)?;
    file.write_at(&bytes, 0)?;
    file.sync()?;
    dir.sync_dir(&branch_dir(INDEX, branch))?;
    Ok((file, bytes.len() as u64))
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

/// The footer of a closed segment, `bytes`: none if the segment closed with none, as one whose
/// footer named commits not durable, which opening its branch closes again.
fn segment_footer(bytes: &[u8]) -> Result<Option<Footer>, Error> {
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Footer::decode(bytes).context("a segment's footer")?))
}

/// Closes `branch`'s segments left open, whose data its state as of `epoch` may need, and those
/// closed whose footers name commits after it, which were not durable: framed, with no footer.
fn close_left_open<D: Dir>(dir: &D, branch: u64, epoch: u64) -> Result<(), Error> {
    let open = list_numbers(dir, &branch_dir(OPEN_SEGMENTS, branch))?;
    let closed = list_numbers(dir, &branch_dir(SEGMENTS, branch))?;
    let numbers: BTreeSet<u64> = open.iter().chain(&closed).copied().collect();
    for number in numbers {
        let segment = SegmentId { branch, number };
        let path = segment_path(segment);
        let raw = || -> Result<Vec<u8>, Error> {
            Ok(read_whole(&dir.open(&open_segment_path(segment), true)?)?)
        };
        // Its data, if it is to be framed again: the framed file's, if it is whole, which a commit
        // made durable, or a closing between commits ([`Store::close_now`]), which removed the
        // raw file; or else the raw file's.
        let data = match dir.open(&path, true) {
            Ok(file) => match read_frame(&file)? {
                Some((_, footer)) => match segment_footer(&footer) {
                    Ok(footer) if footer.as_ref().is_none_or(|f| footer_last(f) <= epoch) => None,
                    _ => match extent_frame::data(&read_whole(&file)?) {
                        Ok(data) => Some(data),
                        Err(_) => Some(raw()?),
                    },
                },
                None => Some(raw()?),
            },
            Err(err) if not_found(&err) => Some(raw()?),
            Err(err) => return Err(err.into()),
        };
        if let Some(data) = data {
            match dir.remove(&path) {
                Err(err) if !not_found(&err) => return Err(err.into()),
                _ => {}
            }
            let file = dir.create(&path, true)?;
            file.write_at(&extent_frame::framed(&data, &[]), 0)?;
            file.sync()?;
            dir.sync_dir(&branch_dir(SEGMENTS, branch))?;
            dir.barrier()?;
        }
        match dir.remove(&open_segment_path(segment)) {
            Err(err) if !not_found(&err) => return Err(err.into()),
            _ => {}
        }
    }
    Ok(())
}

/// `branch`'s state from its newest checkpoint and the footers of its closed segments after it:
/// the segment open as of the checkpoint, and each closed after it that follows on.
pub(crate) fn rebuild<D: Dir>(dir: &D, branch: u64) -> Result<Recovered, Error> {
    let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
    let mut state = None;
    for &number in numbers.iter().rev() {
        let file = dir.open(&index_path(branch, number), false)?;
        if let Some(Record::Checkpoint(table)) = decode(&read_file(&file)?).0.into_iter().next() {
            state = Some(Recovered::of(Checkpoint::of_table(&table)?));
            break;
        }
    }
    let mut state = state.ok_or_else(|| anyhow!("branch {branch:#x} has no checkpoint"))?;
    let mut last: Option<(u64, u64)> = None;
    for number in list_numbers(dir, &branch_dir(SEGMENTS, branch))? {
        if number < state.segment {
            continue;
        }
        let segment = SegmentId { branch, number };
        let path = segment_path(segment);
        let (_, footer) = read_frame(&dir.open(&path, true)?)?
            .ok_or_else(|| anyhow!("{path} is not a whole framed file"))?;
        let Some(footer) = segment_footer(&footer)? else { break };
        if (number == state.segment) != last.is_none()
            || last.is_some_and(|last| footer.after != Some(last))
        {
            break;
        }
        last = Some((footer.seq, footer.epoch));
        let epoch = footer_last(&footer);
        for (extent, offset, len) in footer.extents {
            let piece = Piece { segment, offset, extent_offset: 0, len };
            state.extents.insert(extent, Extent { len, data: Data::Pieces(vec![piece]) });
        }
        for (extent, location) in footer.own {
            state.extents.insert(extent, Extent { len: location.len, data: Data::Own });
        }
        for extent in footer.frees {
            state.extents.remove(&extent);
        }
        if let Some(root) = footer.root {
            state.root = root;
        }
        state.next_sequence = footer.next_sequence;
        state.epoch = state.epoch.max(epoch);
    }
    Ok(state)
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

    /// Opens `branch` of `lineage`, whose record is `record`: its own state, or, a fork's first
    /// opening, its parent's at the fork, which its index then holds.
    pub(crate) fn open_in(
        lineage: Arc<Lineage<D>>,
        branch: u64,
        token: u64,
        record: BranchRecord,
    ) -> Result<Self, Error> {
        let dir = &lineage.dir;
        let recovered = match (record.resolved, record.origin) {
            (true, _) => recover(dir, branch)?,
            (false, Some(origin)) => {
                let Origin { parent, epoch, index, .. } = origin;
                let at_fork = state_at(dir, parent, index, epoch)
                    .with_context(|| format!("branch {branch:#x}'s parent at its fork"))?;
                // A branch of its own, which has minted nothing, and committed nothing.
                let state = Recovered { epoch: 0, next_sequence: 0, segment: 0, ..at_fork };
                create_branch_dirs(dir, branch)?;
                // What an earlier opening wrote, before it could record that it had.
                for number in list_numbers(dir, &branch_dir(INDEX, branch))? {
                    dir.remove(&index_path(branch, number))?;
                }
                write_checkpoint(dir, branch, 0, &state)?;
                dir.barrier()?;
                lineage.claim(branch, files_of(&state.extents));
                lineage.resolved(branch)?;
                state
            }
            (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
        };
        let Recovered { epoch, next_sequence, root, extents, .. } = recovered;
        // Footers of commits not recovered, which were not durable, are no footers: the commits
        // after this epoch are others.
        close_left_open(dir, branch, epoch)?;

        // The segments its extents' data is in, and the extents' own files, which must be there.
        let mut segments = BTreeMap::new();
        let mut stored = 0;
        let mut own_sizes = HashMap::new();
        for (extent, entry) in &extents {
            stored += entry.len;
            match &entry.data {
                Data::Pieces(pieces) => {
                    for piece in pieces {
                        let segment = segments.entry(piece.segment).or_insert(Segment {
                            file: None,
                            len: 0,
                            size: 0,
                            live: 0,
                            writers: 0,
                            closed: None,
                        });
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
        segments.insert(
            open_segment,
            Segment {
                file: Some(Source::Raw(file)),
                len: 0,
                size: 0,
                live: 0,
                writers: 0,
                closed: None,
            },
        );

        // A new index file, with a checkpoint of what was recovered, which replaces the others but
        // those forks not yet opened need.
        let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
        let index_number = numbers.last().map_or(0, |number| number + 1);
        let segment = open_segment.number;
        let state = Recovered { epoch, next_sequence, root, segment, extents };
        let (index_file, len) = write_checkpoint(dir, branch, index_number, &state)?;
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
        let Recovered { root, extents, .. } = state;
        let mut files = files_of(&extents);
        files.insert(FileKey::Segment(open_segment));
        lineage.reset(branch, epoch, &files);

        let options = lineage.options;
        let views = Views::new(epoch, &root);
        Ok(Self {
            lineage,
            branch,
            token,
            parent: record.origin.map(|origin| origin.parent),
            options,
            state: Mutex::new(State {
                capacity: record.capacity,
                extents,
                segments,
                open_segment,
                stored,
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
                footing: None,
                last_closed: None,
                checkpoint_wanted: false,
                failed: false,
                closed: false,
                uploads: None,
                kept: HashMap::new(),
                kept_for_views: 0,
                sealed: HashMap::new(),
            }),
            committing: Mutex::new(()),
            pending: Mutex::new(VecDeque::new()),
            flushing: Mutex::new(()),
            idle: Condvar::new(),
            views,
        })
    }

    /// The branch's state as its newest checkpoint and the footers of its closed segments after it
    /// say: as of the last commit a closed segment holds, if one does (doc-7: a branch's history is
    /// in its segments).
    pub fn rebuild(&self) -> Result<Rebuilt, Error> {
        let state = rebuild(&self.lineage.dir, self.branch)?;
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
    /// inherits the extents, in this branch's files and those it inherited, rather than copies.
    pub fn fork(&self, child: u64) -> Result<(), Error> {
        let committing = self.committing.lock().unwrap();
        // At the end of its segment, which the commit closes, and whose footer names it (doc-7,
        // Forks).
        {
            let mut state = self.writable()?;
            state.close_wanted = true;
            state.window.forks.push(child);
        }
        self.commit_locked(&committing, None)?;
        self.flush()?;
        let (origin, capacity) = {
            let state = self.state()?;
            let origin = Origin {
                parent: self.branch,
                epoch: state.epoch,
                next_sequence: state.next_sequence,
                index: state.index.number,
            };
            (origin, state.capacity)
        };
        self.lineage.fork(origin, child, capacity)
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

    /// Closes the branch, once the commits only ordered are durable: another store may open it,
    /// and this one reads what it holds, as caches of its extents may still, and changes nothing.
    pub fn close(&self) -> Result<(), Error> {
        let flushed = if self.flush_pending() { self.flush() } else { Ok(()) };
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
        if state.open_files > self.options.open_files {
            // Close the files no read is using.
            let open_segment = state.open_segment;
            let mut closed = 0;
            for (number, segment) in &mut state.segments {
                if *number != open_segment
                    && segment.file.as_ref().is_some_and(|file| Arc::strong_count(file.file()) == 1)
                {
                    segment.file = None;
                    closed += 1;
                }
            }
            state.open_files -= closed;
        }
        Ok(file)
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
        // Not into a segment closing between commits.
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
        let entry = state.extents.entry(extent).or_insert(Extent {
            len: 0,
            data: if write.segment.is_some() { Data::Pieces(Vec::new()) } else { Data::Own },
        });
        if end > entry.len {
            state.stored += end - entry.len;
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
        if let Reader::View(epoch) = reader {
            let later = state.sealed.get(&extent).is_some_and(|sealed| *sealed > epoch);
            if later || state.open.contains_key(&extent) {
                bail!(NoExtent(extent));
            }
        }
        let entry = match state.extents.get(&extent) {
            Some(entry) => entry.clone(),
            None => (state.kept.get(&extent))
                .filter(|_| reader != Reader::Store)
                .map(|kept| kept.entry.clone())
                .ok_or(NoExtent(extent))?,
        };
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
        state.changes.push(Change::Free(extent));
        // Forks not yet opened that inherited it hold its files now.
        self.lineage.transfer(self.branch, extent, &files_of([(&extent, &entry)]));
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
        let views_keep =
            |extent: &ExtentId, kept: &Kept| kept.sealed && keeping.keeps(*extent, kept.freed);
        let gone: Vec<ExtentId> = (state.kept.iter())
            .filter(|(_, kept)| kept.freed <= published)
            .filter(|(extent, kept)| !kept.uploader && !views_keep(extent, kept))
            .map(|(extent, _)| *extent)
            .collect();
        for extent in gone {
            let kept = state.kept.remove(&extent).unwrap();
            Self::forget(state, extent, kept.entry);
        }
        state.kept_for_views = (state.kept.iter())
            .filter(|(extent, kept)| views_keep(extent, kept))
            .map(|(_, kept)| kept.entry.len)
            .sum();
    }

    /// The bytes of the kept extents views would keep, as `keeping` says.
    fn kept_bytes(state: &State<D::File>, keeping: &Keeping) -> u64 {
        (state.kept.iter())
            .filter(|(extent, kept)| kept.sealed && keeping.keeps(**extent, kept.freed))
            .map(|(_, kept)| kept.entry.len)
            .sum()
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
            state.extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
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
        let durable = state.epoch - self.pending.lock().unwrap().len() as u64;
        state.uploads.get_or_insert_with(|| Uploads { durable, ..Uploads::default() });
        state.footing = Some(footing);
        Ok(true)
    }

    /// fxdax: the commits made durable since the last call, oldest first.
    pub fn committed(&self) -> Result<Vec<Committed>, Error> {
        let mut state = self.state()?;
        let Some(uploads) = &mut state.uploads else { return Ok(Vec::new()) };
        let durable = if rules().durable_only { uploads.durable } else { u64::MAX };
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
        for kept in state.kept.values_mut().filter(|kept| kept.freed <= epoch) {
            kept.uploader = false;
        }
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
        self.commit_locked(&committing, root)
    }

    fn commit_locked(
        &self,
        _committing: &MutexGuard<'_, ()>,
        root: Option<&[u8]>,
    ) -> Result<(), Error> {
        let mut state = self.writable()?;
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
            if let Err(error) = self.close_open_segment(state, epoch) {
                state.failed = true;
                return Err(error.context("committing the extent store"));
            }
        }
        let mut syncs: Vec<Arc<D::File>> =
            std::mem::take(&mut state.dirty_own).into_values().collect();
        for number in std::mem::take(&mut state.dirty_segments) {
            syncs.push(self.segment_file(state, number)?.file().clone());
        }
        let dirs = std::mem::take(&mut state.dirty_dirs);
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
            let table = Checkpoint {
                epoch,
                next_sequence: state.next_sequence,
                root: root.map_or_else(|| state.root.clone(), <[u8]>::to_vec),
                segment: state.open_segment.number,
                extents: state
                    .extents
                    .iter()
                    .map(|(extent, entry)| (*extent, entry.clone()))
                    .collect(),
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
            // But for what forks not yet opened need.
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
        // Queued while the state is locked, so that commits queue in their order.
        self.pending.lock().unwrap().push_back(Pending {
            epoch,
            syncs,
            dirs,
            record,
            index,
            offset,
            new_index: checkpoint,
            doomed,
        });
        Ok(())
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
    /// the writer the footing names, if any; and opens another.
    fn close_open_segment(&self, state: &mut State<D::File>, epoch: u64) -> Result<(), Error> {
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
        let mut data = Aligned::new(len as usize);
        raw.read_at(data.as_mut_slice(), 0)?;
        let bytes = extent_frame::framed(data.as_slice(), &footer.encode());
        let file = self.lineage.dir.create(&segment_path(number), true)?;
        file.write_at(&bytes, 0)?;
        let framed = Framed::new(Arc::new(file), extent_frame::Parts::of(&bytes)?);
        let segment = state.segments.get_mut(&number).unwrap();
        segment.size = framed.size();
        segment.file = Some(Source::Framed(Arc::new(framed)));
        segment.closed = Some(epoch);
        state.dirty_segments.insert(number);
        state.dirty_dirs.insert(branch_dir(SEGMENTS, self.branch));
        state.doomed.push(open_segment_path(number));

        let next = SegmentId { number: number.number + 1, ..number };
        let file = Arc::new(self.lineage.dir.create(&open_segment_path(next), true)?);
        self.lineage.claim(self.branch, [FileKey::Segment(next)]);
        state.segments.insert(
            next,
            Segment {
                file: Some(Source::Raw(file)),
                len: 0,
                size: 0,
                live: 0,
                writers: 0,
                closed: None,
            },
        );
        state.open_files += 1;
        state.open_segment = next;
        state.opened = Instant::now();
        state.close_wanted = false;
        state.dirty_dirs.insert(branch_dir(OPEN_SEGMENTS, self.branch));
        Ok(())
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
            self.close_open_segment(state, epoch)?;
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
        let durable = if rules().durable_only { uploads.durable } else { u64::MAX };
        let mut taken = Vec::new();
        while uploads.closed.front().is_some_and(|(_, footer)| footer_last(footer) <= durable) {
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
        let _flushing = self.flushing.lock().unwrap();
        // A store that failed makes nothing more durable.
        drop(self.state()?);
        let pending: Vec<Pending<D::File>> = self.pending.lock().unwrap().drain(..).collect();
        if pending.is_empty() {
            return self.state().map(drop);
        }
        debug_assert!(pending.windows(2).all(|w| w[0].epoch + 1 == w[1].epoch), "epochs in order");
        let durable = || -> Result<(), Error> {
            let mut synced = HashSet::new();
            for file in pending.iter().flat_map(|pending| &pending.syncs) {
                if synced.insert(Arc::as_ptr(file)) {
                    file.sync().context("syncing extents' data")?;
                }
            }
            let dirs: BTreeSet<_> = pending.iter().flat_map(|pending| &pending.dirs).collect();
            for dir in dirs {
                let dir = dir.as_str();
                self.lineage.dir.sync_dir(dir).with_context(|| format!("syncing {dir}"))?;
            }
            let mut indexes = Vec::new();
            for pending in &pending {
                pending.index.write_at(&pending.record, pending.offset)?;
                if !indexes.iter().any(|index| Arc::ptr_eq(index, &pending.index)) {
                    indexes.push(pending.index.clone());
                }
            }
            for index in &indexes {
                index.sync()?;
            }
            if pending.iter().any(|pending| pending.new_index) {
                self.lineage.dir.sync_dir(&branch_dir(INDEX, self.branch))?;
            }
            self.lineage.dir.barrier()?;
            Ok(())
        };
        if let Err(error) = durable() {
            self.state.lock().unwrap().failed = true;
            return Err(error.context("committing the extent store"));
        }
        let last = pending.last().unwrap().epoch;
        if let Some(uploads) = &mut self.state.lock().unwrap().uploads {
            uploads.durable = uploads.durable.max(last);
        }
        // The files of extents' data no branch holds extents in go, once the commits that let
        // them go are durable.
        self.lineage.durable(self.branch, last);
        for path in pending.into_iter().flat_map(|pending| pending.doomed) {
            if let Err(err) = self.lineage.dir.remove(&path)
                && !not_found(&err)
            {
                // Opening the store removes it, as nothing refers to it.
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
        Ok(())
    }

    /// fxdax: whether commits are waiting for [`Store::flush`] to make them durable.
    pub fn flush_pending(&self) -> bool {
        !self.pending.lock().unwrap().is_empty()
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
            // Forks not yet opened that inherited it hold its old pieces' segments now.
            self.lineage.transfer(self.branch, extent, &files_of([(&extent, &entry)]));
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
        self.commit_locked(&committing, None)?;
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
            own_bytes: state
                .extents
                .values()
                .filter(|entry| entry.data == Data::Own)
                .map(|entry| entry.len)
                .sum(),
            disk_bytes: own().map(|(_, segment)| segment.size).sum::<u64>()
                + state.own_sizes.values().sum::<u64>(),
            kept_bytes: state.kept.values().map(|kept| kept.entry.len).sum(),
            kept_for_views: state.kept_for_views,
        })
    }

    /// fxdax: a view of the branch as of its latest commit, which a follower reads through
    /// ([`Store::read_view`]) while the writer goes on. It waits for no commit.
    pub fn view(&self) -> View {
        self.views.open()
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
        self.read_into_entry(extent, offset, buf, Reader::View(view.epoch()))
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
        self.locate_entry(extent, offset, len, Reader::View(view.epoch()))
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
            state.extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        extents.sort();
        Ok(extents)
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
