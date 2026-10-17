//! The store. See the crate's docs.

use crate::dir::{Dir, File};
use crate::layout::{
    BranchRecord, Change, Checkpoint, Data, Delta, Description, EXTENTS, Extent, INDEX, Origin,
    Piece, Record, SEGMENTS, SegmentId, branch_dir, decode, encode, extent_path, index_path,
    segment_path,
};
use crate::lineage::{FileKey, Lineage, create_branch_dirs, files_of, list_numbers};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use storage_device::{ExtentId, NoExtent, OtherBranch, SealedExtent};

/// The error of a write the store has no room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoSpace;

impl std::fmt::Display for NoSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the extent store is full")
    }
}

impl std::error::Error for NoSpace {}

/// fxdax: the error of a read of an extent the store holds whose data is elsewhere
/// ([`Store::adopt`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteData(pub ExtentId);

impl std::fmt::Display for RemoteData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}'s data is not in the store", self.0)
    }
}

impl std::error::Error for RemoteData {}

/// The error of I/O at an offset, or of a length, not a multiple of the block size, or empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unaligned {
    pub offset: u64,
    pub len: u64,
}

impl std::fmt::Display for Unaligned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unaligned I/O: {} bytes at {}", self.len, self.offset)
    }
}

impl std::error::Error for Unaligned {}

/// How a store lays its extents out.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The size past which a segment takes no more extents: the next commit starts another.
    pub segment_size: u64,
    /// The size from which an extent is stored in a file of its own: one whose first write ends
    /// there or past it.
    pub large_extent: u64,
    /// The bytes of deltas an index file holds after which a commit starts another, with a
    /// checkpoint.
    pub checkpoint_after: u64,
    /// How many segments' files the store keeps open between reads.
    pub open_files: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            segment_size: 32 << 20,
            large_extent: 4 << 20,
            checkpoint_after: 16 << 20,
            open_files: 64,
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

/// fxdax: what a store keeps for its uploader.
#[derive(Default)]
struct Uploads {
    /// The commits ordered and not yet taken, oldest first.
    committed: VecDeque<Committed>,
    /// The last epoch durable.
    durable: u64,
    /// The extents freed and not yet released, and the epochs of the commits that freed them:
    /// their data stays, for the uploader to read.
    kept: HashMap<ExtentId, (Extent, u64)>,
    /// The bytes each commit not yet released wrote.
    unreleased: BTreeMap<u64, u64>,
}

/// A part of a range of an extent: bytes of a file, or zeros, where nothing was written.
pub enum Part<F> {
    Data { file: Arc<F>, offset: u64, len: u64 },
    Zeros(u64),
}

/// A branch of an extent store, open. See the crate's docs.
pub struct Store<D: Dir> {
    lineage: Arc<Lineage<D>>,
    branch: u64,
    /// Its opening of the branch, which closing it ends.
    token: u64,
    capacity: u64,
    options: Options,
    state: Mutex<State<D::File>>,
    /// Held by a commit, or by cleaning, which commits.
    committing: Mutex<()>,
    /// The commits ordered but not yet durable, oldest first, which [`Store::flush`] makes durable.
    pending: Mutex<VecDeque<Pending<D::File>>>,
    /// Held by a flush: commits become durable in order.
    flushing: Mutex<()>,
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
    /// The files of extents stored on their own, open.
    own_files: HashMap<ExtentId, Arc<F>>,
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
    /// The segments' files open.
    open_files: usize,
    /// Whether a commit failed, after which the store's state may differ from what its files say,
    /// so it does nothing more.
    failed: bool,
    /// Whether it was closed ([`Store::close`]): it reads what it holds, and changes nothing.
    closed: bool,
    /// fxdax: what an uploader takes, if one does.
    uploads: Option<Uploads>,
}

struct Segment<F> {
    /// Its file, if open.
    file: Option<Arc<F>>,
    /// The bytes appended to it, if it is the branch's own.
    len: u64,
    /// The bytes of extents' data it holds: pieces of extents not freed.
    live: u64,
    /// The writes to it in progress.
    writers: u64,
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

/// The alignment of I/O that bypasses the host's page cache: the largest block size a store has.
pub(crate) const ALIGNMENT: usize = 4096;

/// A buffer aligned for I/O that bypasses the host's page cache.
struct Aligned {
    bytes: Vec<u8>,
    start: usize,
    len: usize,
}

impl Aligned {
    const ALIGNMENT: usize = ALIGNMENT;

    fn new(len: usize) -> Self {
        let bytes = vec![0; len + Self::ALIGNMENT];
        let start = bytes.as_ptr().align_offset(Self::ALIGNMENT);
        Self { bytes, start, len }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[self.start..self.start + self.len]
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..self.start + self.len]
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
    pub extents: HashMap<ExtentId, Extent>,
}

impl Recovered {
    /// The state a checkpoint holds.
    fn of(checkpoint: Checkpoint) -> Self {
        Self {
            epoch: checkpoint.epoch,
            next_sequence: checkpoint.next_sequence,
            root: checkpoint.root,
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
        if let Some(Record::Checkpoint(checkpoint)) = records.next() {
            let mut recovered = Recovered::of(checkpoint);
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
    let Some(Record::Checkpoint(checkpoint)) = records.next() else {
        bail!("branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    };
    let mut recovered = Recovered::of(checkpoint);
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
    let bytes = encode(&Record::Checkpoint(state.checkpoint()));
    let file = dir.create(&index_path(branch, number), false)?;
    file.write_at(&bytes, 0)?;
    file.sync()?;
    dir.sync_dir(&branch_dir(INDEX, branch))?;
    Ok((file, bytes.len() as u64))
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
                let state = Recovered { epoch: 0, next_sequence: 0, ..at_fork };
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
        let Recovered { epoch, next_sequence, root, extents } = recovered;

        // The segments its extents' data is in, and the extents' own files, which must be there.
        let mut segments = BTreeMap::new();
        let mut stored = 0;
        for (extent, entry) in &extents {
            stored += entry.len;
            match &entry.data {
                Data::Pieces(pieces) => {
                    for piece in pieces {
                        let segment = segments.entry(piece.segment).or_insert(Segment {
                            file: None,
                            len: 0,
                            live: 0,
                            writers: 0,
                        });
                        segment.live += piece.len;
                    }
                }
                Data::Own => {
                    let path = extent_path(*extent);
                    dir.open(&path, true).with_context(|| format!("extent {extent:?}'s file"))?;
                }
                Data::Remote => {}
            }
        }
        for (id, segment) in &mut segments {
            let path = segment_path(*id);
            let file = dir.open(&path, true).with_context(|| format!("segment {path}"))?;
            if id.branch == branch {
                segment.len = file.size()?;
            }
        }

        // Writes go to a new segment, past whatever the branch's last one holds.
        let own = list_numbers(dir, &branch_dir(SEGMENTS, branch))?;
        let open_segment = SegmentId { branch, number: own.last().map_or(0, |last| last + 1) };
        let file = Arc::new(dir.create(&segment_path(open_segment), true)?);
        segments.insert(open_segment, Segment { file: Some(file), len: 0, live: 0, writers: 0 });

        // A new index file, with a checkpoint of what was recovered, which replaces the others but
        // those forks not yet opened need.
        let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
        let index_number = numbers.last().map_or(0, |number| number + 1);
        let state = Recovered { epoch, next_sequence, root, extents };
        let (index_file, len) = write_checkpoint(dir, branch, index_number, &state)?;
        dir.sync_dir(&branch_dir(SEGMENTS, branch))?;
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
        Ok(Self {
            lineage,
            branch,
            token,
            capacity: record.capacity,
            options,
            state: Mutex::new(State {
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
                open_files: 1,
                failed: false,
                closed: false,
                uploads: None,
            }),
            committing: Mutex::new(()),
            pending: Mutex::new(VecDeque::new()),
            flushing: Mutex::new(()),
        })
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
        self.commit_locked(&committing, None)?;
        self.flush()?;
        let origin = {
            let state = self.state()?;
            Origin {
                parent: self.branch,
                epoch: state.epoch,
                next_sequence: state.next_sequence,
                index: state.index.number,
            }
        };
        self.lineage.fork(origin, child, self.capacity)
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.lineage.dir
    }

    /// The branch, as its user sees it.
    pub fn description(&self) -> Description {
        Description {
            block_size: self.lineage.block_size,
            capacity: self.capacity,
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

    /// A segment's file, opened if it isn't.
    fn segment_file(
        &self,
        state: &mut State<D::File>,
        number: SegmentId,
    ) -> Result<Arc<D::File>, Error> {
        let segment =
            state.segments.get(&number).ok_or_else(|| anyhow!("no segment {number:?}"))?;
        if let Some(file) = &segment.file {
            return Ok(file.clone());
        }
        let file = Arc::new(self.lineage.dir.open(&segment_path(number), true)?);
        state.segments.get_mut(&number).unwrap().file = Some(file.clone());
        state.open_files += 1;
        if state.open_files > self.options.open_files {
            // Close the files no read is using.
            let open_segment = state.open_segment;
            let mut closed = 0;
            for (number, segment) in &mut state.segments {
                if *number != open_segment
                    && segment.file.as_ref().is_some_and(|file| Arc::strong_count(file) == 1)
                {
                    segment.file = None;
                    closed += 1;
                }
            }
            state.open_files -= closed;
        }
        Ok(file)
    }

    /// `extent`'s own file, opened if it isn't.
    fn own_file(
        &self,
        state: &mut State<D::File>,
        extent: ExtentId,
    ) -> Result<Arc<D::File>, Error> {
        if let Some(file) = state.own_files.get(&extent) {
            return Ok(file.clone());
        }
        let file = Arc::new(self.lineage.dir.open(&extent_path(extent), true)?);
        if state.own_files.len() >= self.options.open_files {
            state.own_files.retain(|_, file| Arc::strong_count(file) > 1);
        }
        state.own_files.insert(extent, file.clone());
        Ok(file)
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
        let current = state.extents.get(&extent).map(|entry| entry.len);
        if current.is_some() && !state.open.contains_key(&extent) {
            bail!(SealedExtent(extent));
        }
        let end = offset + len;
        let grows = end.saturating_sub(current.unwrap_or(0));
        if state.stored + state.reserved + grows > self.capacity {
            bail!(NoSpace);
        }
        let placement = match state.open.get(&extent) {
            Some(placement) => placement.clone(),
            None => {
                let placement = if end >= self.options.large_extent {
                    let path = extent_path(extent);
                    let file = self
                        .lineage
                        .dir
                        .create(&path, true)
                        .with_context(|| format!("creating {path}"))?;
                    state.dirty_dirs.insert(branch_dir(EXTENTS, self.branch));
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
                let file = self.segment_file(&mut state, number)?;
                let segment = state.segments.get_mut(&number).unwrap();
                let at = segment.len;
                segment.len += len;
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
                state.dirty_segments.insert(number);
                state.changes.push(Change::Write { extent, piece });
            }
            (None, Data::Own) => {
                if state.dirty_own.insert(extent, write.file.clone()).is_none() {
                    self.lineage.claim(self.branch, [FileKey::Own(extent)]);
                }
                state.own_files.insert(extent, write.file.clone());
                state.changes.push(Change::WriteOwn { extent, len: entry.len });
            }
            _ => unreachable!("an extent's data is in segments or its own file"),
        }
        state.next_sequence = state.next_sequence.max(extent.sequence + 1);
    }

    fn abandon_write(&self, write: &Write<'_, D>) {
        let mut state = self.state.lock().unwrap();
        state.reserved -= write.grows;
        if let Some(number) = write.segment {
            // What it wrote, if anything, is dead space.
            state.segments.get_mut(&number).unwrap().writers -= 1;
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
        self.locate_entry(extent, offset, len, false)
    }

    /// Where the bytes are, as [`Store::locate`] says, of an extent held, or, if `kept`, freed and
    /// kept for the uploader.
    fn locate_entry(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        kept: bool,
    ) -> Result<Vec<Part<D::File>>, Error> {
        self.check_io(offset, len)?;
        let mut state = self.state()?;
        let entry = match state.extents.get(&extent) {
            Some(entry) => entry.clone(),
            None => state
                .uploads
                .as_ref()
                .filter(|_| kept)
                .and_then(|uploads| uploads.kept.get(&extent))
                .map(|(entry, _)| entry.clone())
                .ok_or(NoExtent(extent))?,
        };
        let end = offset + len;
        let held_end = end.min(entry.len).max(offset);
        let mut parts = Vec::new();
        match entry.data {
            Data::Own => {
                if held_end > offset {
                    let file = self.own_file(&mut state, extent)?;
                    parts.push(Part::Data { file, offset, len: held_end - offset });
                }
            }
            Data::Remote => bail!(RemoteData(extent)),
            Data::Pieces(pieces) => {
                for (start, end, source) in paint(&pieces, offset, held_end) {
                    parts.push(match source {
                        None => Part::Zeros(end - start),
                        Some((number, at)) => Part::Data {
                            file: self.segment_file(&mut state, number)?,
                            offset: at,
                            len: end - start,
                        },
                    });
                }
            }
        }
        if end > held_end {
            parts.push(Part::Zeros(end - held_end));
        }
        Ok(parts)
    }

    /// Frees `extent`, which may be open, if the store holds it.
    pub fn free(&self, extent: ExtentId) -> Result<(), Error> {
        let mut state = self.writable()?;
        let state = &mut *state;
        let placement = state.open.remove(&extent);
        let Some(entry) = state.extents.remove(&extent) else {
            if let Some(Placement::Own(_)) = placement {
                // Created by a write abandoned.
                state.doomed.push(extent_path(extent));
            }
            return Ok(());
        };
        state.stored -= entry.len;
        state.changes.push(Change::Free(extent));
        // Forks not yet opened that inherited it hold its files now.
        self.lineage.transfer(self.branch, extent, &files_of([(&extent, &entry)]));
        match &mut state.uploads {
            // Its data stays until the uploader has what it needs of it.
            Some(uploads) => {
                uploads.kept.insert(extent, (entry, state.epoch + 1));
            }
            None => Self::forget(state, extent, entry),
        }
        Ok(())
    }

    /// Lets the data of `extent`, freed, go.
    fn forget(state: &mut State<D::File>, extent: ExtentId, entry: Extent) {
        match entry.data {
            Data::Pieces(pieces) => {
                for piece in pieces {
                    state.segments.get_mut(&piece.segment).unwrap().live -= piece.len;
                }
            }
            Data::Own => {
                state.own_files.remove(&extent);
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
        Ok(())
    }

    /// fxdax: from now on, keeps for an uploader what each commit changes ([`Store::committed`]),
    /// and the data of the extents it frees, until released ([`Store::release`]). Returns what the
    /// store holds as of now, which the commits it keeps follow.
    pub fn upload(&self) -> Result<Snapshot, Error> {
        let mut state = self.state()?;
        let durable = state.epoch - self.pending.lock().unwrap().len() as u64;
        state.uploads.get_or_insert_with(|| Uploads { durable, ..Uploads::default() });
        let mut extents: Vec<_> =
            state.extents.iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        extents.sort();
        Ok(Snapshot { epoch: state.epoch, extents, root: state.root.clone() })
    }

    /// fxdax: the commits made durable since the last call, oldest first.
    pub fn committed(&self) -> Result<Vec<Committed>, Error> {
        let mut state = self.state()?;
        let Some(uploads) = &mut state.uploads else { return Ok(Vec::new()) };
        let durable = uploads.durable;
        let mut committed = Vec::new();
        while uploads.committed.front().is_some_and(|front| front.epoch <= durable) {
            committed.push(uploads.committed.pop_front().unwrap());
        }
        Ok(committed)
    }

    /// fxdax: lets the data of the extents the commits up to `epoch` freed go: the uploader has
    /// what it needs of them. Their files go with the commit after.
    pub fn release(&self, epoch: u64) -> Result<(), Error> {
        let mut state = self.state()?;
        let state = &mut *state;
        let Some(uploads) = &mut state.uploads else { return Ok(()) };
        uploads.unreleased.retain(|committed, _| *committed > epoch);
        let released: Vec<_> = uploads
            .kept
            .iter()
            .filter(|(_, (_, freed))| *freed <= epoch)
            .map(|(extent, _)| *extent)
            .collect();
        for extent in released {
            let (entry, _) = state.uploads.as_mut().unwrap().kept.remove(&extent).unwrap();
            Self::forget(state, extent, entry);
        }
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
        self.read_into_entry(extent, offset, buf, true)
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
                    Change::Move { .. } | Change::Adopt { .. } => {}
                }
            }
            let bytes = committed.written.iter().map(|(_, len)| len).sum();
            uploads.unreleased.insert(epoch, bytes);
            uploads.committed.push_back(committed);
        }
        let mut syncs: Vec<Arc<D::File>> =
            std::mem::take(&mut state.dirty_own).into_values().collect();
        for number in std::mem::take(&mut state.dirty_segments) {
            syncs.push(self.segment_file(state, number)?);
        }
        let dirs = std::mem::take(&mut state.dirty_dirs);
        let mut doomed = std::mem::take(&mut state.doomed);
        let mut released = std::mem::take(&mut state.released);
        // The segments none of its extents' data is in any more, by this commit, if not before:
        // the branch lets them go. Writes go to the open one, and to one just filled, until they
        // finish.
        let dead: Vec<SegmentId> = state
            .segments
            .iter()
            .filter(|(number, segment)| {
                **number != state.open_segment && segment.live == 0 && segment.writers == 0
            })
            .map(|(number, _)| *number)
            .collect();
        for number in dead {
            if state.segments.remove(&number).unwrap().file.is_some() {
                state.open_files -= 1;
            }
            released.push(FileKey::Segment(number));
        }
        self.lineage.release(self.branch, epoch, released);
        // Once the open segment is full, writes go to a new one. Writes finishing in the old one
        // belong to the next commit, as do the new one's directory entry.
        if state.segments[&state.open_segment].len >= self.options.segment_size {
            let number = SegmentId { number: state.open_segment.number + 1, ..state.open_segment };
            let file = Arc::new(self.lineage.dir.create(&segment_path(number), true)?);
            self.lineage.claim(self.branch, [FileKey::Segment(number)]);
            state
                .segments
                .insert(number, Segment { file: Some(file), len: 0, live: 0, writers: 0 });
            state.open_files += 1;
            state.open_segment = number;
            state.dirty_dirs.insert(branch_dir(SEGMENTS, self.branch));
        }
        let checkpoint = state.index.deltas >= self.options.checkpoint_after;
        let record = encode(&if checkpoint {
            Record::Checkpoint(Checkpoint {
                epoch,
                next_sequence: state.next_sequence,
                root: root.map_or_else(|| state.root.clone(), <[u8]>::to_vec),
                extents: state
                    .extents
                    .iter()
                    .map(|(extent, entry)| (*extent, entry.clone()))
                    .collect(),
            })
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
            (file, 0)
        } else {
            let offset = state.index.len;
            state.index.len += record.len() as u64;
            state.index.deltas += record.len() as u64;
            (state.index.file.clone(), offset)
        };
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
                let file = self.segment_file(&mut state, number)?;
                let segment = state.segments.get_mut(&number).unwrap();
                let at = segment.len;
                segment.len += entry.len;
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
        self.read_into_entry(extent, offset, buf, false)
    }

    fn read_into_entry(
        &self,
        extent: ExtentId,
        offset: u64,
        buf: &mut [u8],
        kept: bool,
    ) -> Result<(), Error> {
        let mut at = 0;
        for part in self.locate_entry(extent, offset, buf.len() as u64, kept)? {
            match part {
                Part::Data { file, offset, len } => {
                    let mut data = Aligned::new(len as usize);
                    file.read_at(data.as_mut_slice(), offset)?;
                    buf[at..at + len as usize].copy_from_slice(data.as_slice());
                    at += len as usize;
                }
                Part::Zeros(len) => {
                    buf[at..at + len as usize].fill(0);
                    at += len as usize;
                }
            }
        }
        Ok(())
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

    /// The root as of the last commit: empty if none set one.
    pub fn root(&self) -> Result<Vec<u8>, Error> {
        Ok(self.state()?.root.clone())
    }

    pub fn info(&self) -> Result<Info, Error> {
        let state = self.state()?;
        let capacity = self.capacity;
        Ok(Info {
            capacity,
            used: state.stored,
            available: capacity.saturating_sub(state.stored + state.reserved),
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
        })
    }

    /// `extent`'s length, and whether it is open (written since the last commit), if the store
    /// holds it.
    pub fn extent(&self, extent: ExtentId) -> Result<Option<(u64, bool)>, Error> {
        let state = self.state()?;
        Ok(state.extents.get(&extent).map(|entry| (entry.len, state.open.contains_key(&extent))))
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
