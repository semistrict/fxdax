//! The store. See the crate's docs.

use crate::dir::{Dir, File};
use crate::layout::{
    Change, Checkpoint, DESCRIPTION, Data, Delta, Description, EXTENTS, Extent, INDEX, Piece,
    Record, SEGMENTS, decode, encode, extent_path, index_path, parse_extent_name, parse_number,
    segment_path,
};
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
}

/// A part of a range of an extent: bytes of a file, or zeros, where nothing was written.
pub enum Part<F> {
    Data { file: Arc<F>, offset: u64, len: u64 },
    Zeros(u64),
}

/// An extent store in a directory. See the crate's docs.
pub struct Store<D: Dir> {
    dir: D,
    description: Description,
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
    dirs: BTreeSet<&'static str>,
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
    segments: BTreeMap<u64, Segment<F>>,
    /// The segment taking writes.
    open_segment: u64,
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
    dirty_segments: BTreeSet<u64>,
    dirty_own: HashMap<ExtentId, Arc<F>>,
    dirty_dirs: BTreeSet<&'static str>,
    /// The files the next commit removes, once it is durable.
    doomed: Vec<String>,
    /// The index's file the next commit appends to.
    index: IndexFile<F>,
    /// The segments' files open.
    open_files: usize,
    /// Whether a commit failed, after which the store's state may differ from what its files say,
    /// so it does nothing more.
    failed: bool,
    /// fxdax: what an uploader takes, if one does.
    uploads: Option<Uploads>,
}

struct Segment<F> {
    /// Its file, if open.
    file: Option<Arc<F>>,
    /// The bytes appended to it.
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
    segment: Option<u64>,
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

/// A buffer aligned for I/O that bypasses the host's page cache.
struct Aligned {
    bytes: Vec<u8>,
    start: usize,
    len: usize,
}

impl Aligned {
    const ALIGNMENT: usize = 4096;

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

fn read_file<F: File>(file: &F) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; file.size()? as usize];
    file.read_at(&mut bytes, 0)?;
    Ok(bytes)
}

fn not_found(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound
}

impl<F> State<F> {
    /// Applies `change`, recovering it from the index.
    fn recover(extents: &mut HashMap<ExtentId, Extent>, change: Change) {
        match change {
            Change::Write { extent, piece } => {
                let entry = extents
                    .entry(extent)
                    .or_insert(Extent { len: 0, data: Data::Pieces(Vec::new()) });
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
}

impl<D: Dir> Store<D> {
    /// Makes a new, empty store in `dir`, which is empty, as `description` describes, and opens
    /// it.
    pub fn create(dir: D, description: Description, options: Options) -> Result<Self, Error> {
        ensure!(
            description.block_size.is_power_of_two()
                && description.block_size as usize <= Aligned::ALIGNMENT,
            "bad block size {}",
            description.block_size
        );
        for path in [SEGMENTS, EXTENTS, INDEX] {
            dir.create_dir(path).with_context(|| format!("creating {path}"))?;
        }
        let empty = Checkpoint {
            epoch: 0,
            branch: description.branch,
            next_sequence: 0,
            root: Vec::new(),
            extents: Vec::new(),
        };
        let index = dir.create(&index_path(0), false)?;
        index.write_at(&encode(&Record::Checkpoint(empty)), 0)?;
        index.sync()?;
        dir.sync_dir(INDEX)?;
        // The description is last: until it is durable, the directory is no store.
        let file = dir.create(DESCRIPTION, false)?;
        file.write_at(description.text().as_bytes(), 0)?;
        file.sync()?;
        dir.sync_dir("")?;
        dir.barrier()?;
        Self::open(dir, options)
    }

    /// Opens the store in `dir`, recovering it as it was at its last commit that is durable.
    pub fn open(dir: D, options: Options) -> Result<Self, Error> {
        let description = {
            let file = dir.open(DESCRIPTION, false).context("opening the store's description")?;
            let text = String::from_utf8(read_file(&file)?).context("reading the description")?;
            Description::parse(&text)?
        };
        ensure!(
            description.capacity >= description.block_size as u64,
            "the store is smaller than a block"
        );

        // The index: its newest file whose checkpoint is whole, with the deltas after it that are.
        let mut index_numbers: Vec<u64> =
            dir.list(INDEX)?.iter().filter_map(|name| parse_number(name)).collect();
        index_numbers.sort_unstable_by(|a, b| b.cmp(a));
        let mut recovered = None;
        for &number in &index_numbers {
            let file = dir.open(&index_path(number), false)?;
            let mut records = decode(&read_file(&file)?).0.into_iter();
            if let Some(Record::Checkpoint(checkpoint)) = records.next() {
                recovered = Some((checkpoint, records));
                break;
            }
            log::warn!("extent store: index file {number:#x} holds no checkpoint");
        }
        let (checkpoint, records) = recovered.ok_or_else(|| anyhow!("the store has no index"))?;
        let mut extents: HashMap<ExtentId, Extent> = checkpoint.extents.into_iter().collect();
        let checkpoint_branch = checkpoint.branch;
        let (mut epoch, mut next_sequence, mut root) =
            (checkpoint.epoch, checkpoint.next_sequence, checkpoint.root);
        for record in records {
            let Record::Delta(delta) = record else { break };
            if delta.epoch != epoch + 1 {
                break;
            }
            for change in delta.changes {
                State::<D::File>::recover(&mut extents, change);
            }
            epoch = delta.epoch;
            next_sequence = delta.next_sequence;
            if let Some(new_root) = delta.root {
                root = new_root;
            }
        }
        // A copy's branch is new, and has written nothing.
        if checkpoint_branch != description.branch {
            next_sequence = 0;
        }

        // The files the index refers to, and those it doesn't, which are removed: what commits
        // that weren't durable wrote, or that were removed once the commit after them was.
        let mut live = BTreeMap::<u64, u64>::new();
        let mut own = HashSet::new();
        let mut stored = 0;
        for (extent, entry) in &extents {
            stored += entry.len;
            match &entry.data {
                Data::Pieces(pieces) => {
                    for piece in pieces {
                        *live.entry(piece.segment).or_default() += piece.len;
                    }
                }
                Data::Own => {
                    own.insert(*extent);
                }
                Data::Remote => {}
            }
        }
        let mut segments = BTreeMap::new();
        let mut next_segment = 0;
        for name in dir.list(SEGMENTS)? {
            let Some(number) = parse_number(&name) else {
                bail!("{SEGMENTS}/{name} is no segment");
            };
            next_segment = next_segment.max(number + 1);
            let path = segment_path(number);
            match live.get(&number) {
                Some(&live) => {
                    let len = dir.open(&path, true)?.size()?;
                    segments.insert(number, Segment { file: None, len, live, writers: 0 });
                }
                None => dir.remove(&path).with_context(|| format!("removing {path}"))?,
            }
        }
        if let Some(missing) = live.keys().find(|number| !segments.contains_key(number)) {
            bail!("segment {missing:#x} is missing");
        }
        let mut owned = HashSet::new();
        for name in dir.list(EXTENTS)? {
            let extent =
                parse_extent_name(&name).ok_or_else(|| anyhow!("{EXTENTS}/{name} is no extent"))?;
            if own.contains(&extent) {
                owned.insert(extent);
            } else {
                dir.remove(&extent_path(extent))?;
            }
        }
        if let Some(missing) = own.iter().find(|extent| !owned.contains(extent)) {
            bail!("extent {missing:?}'s file is missing");
        }

        // Writes go to a new segment, past whatever the last one holds.
        let open_segment = next_segment;
        let file = Arc::new(dir.create(&segment_path(open_segment), true)?);
        segments.insert(open_segment, Segment { file: Some(file), len: 0, live: 0, writers: 0 });

        // A new index file, with a checkpoint of what was recovered, which replaces the others.
        let index_number = index_numbers.first().map_or(0, |number| number + 1);
        let checkpoint = Checkpoint {
            epoch,
            branch: description.branch,
            next_sequence,
            root: root.clone(),
            extents: extents.iter().map(|(extent, entry)| (*extent, entry.clone())).collect(),
        };
        let bytes = encode(&Record::Checkpoint(checkpoint));
        let index_file = Arc::new(dir.create(&index_path(index_number), false)?);
        index_file.write_at(&bytes, 0)?;
        index_file.sync()?;
        dir.sync_dir(SEGMENTS)?;
        dir.sync_dir(INDEX)?;
        dir.barrier()?;
        for number in index_numbers {
            dir.remove(&index_path(number))?;
        }

        Ok(Self {
            dir,
            description,
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
                index: IndexFile {
                    number: index_number,
                    file: index_file,
                    len: bytes.len() as u64,
                    deltas: 0,
                },
                open_files: 1,
                failed: false,
                uploads: None,
            }),
            committing: Mutex::new(()),
            pending: Mutex::new(VecDeque::new()),
            flushing: Mutex::new(()),
        })
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.dir
    }

    pub fn description(&self) -> Description {
        self.description
    }

    fn state(&self) -> Result<MutexGuard<'_, State<D::File>>, Error> {
        let state = self.state.lock().unwrap();
        ensure!(!state.failed, "the extent store failed");
        Ok(state)
    }

    fn check_io(&self, offset: u64, len: u64) -> Result<(), Error> {
        let block_size = self.description.block_size as u64;
        if !offset.is_multiple_of(block_size) || !len.is_multiple_of(block_size) || len == 0 {
            bail!(Unaligned { offset, len });
        }
        Ok(())
    }

    /// Segment `number`'s file, opened if it isn't.
    fn segment_file(&self, state: &mut State<D::File>, number: u64) -> Result<Arc<D::File>, Error> {
        let segment = state.segments.get(&number).ok_or_else(|| anyhow!("no segment {number}"))?;
        if let Some(file) = &segment.file {
            return Ok(file.clone());
        }
        let file = Arc::new(self.dir.open(&segment_path(number), true)?);
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
        let file = Arc::new(self.dir.open(&extent_path(extent), true)?);
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
        if extent.branch != self.description.branch {
            bail!(OtherBranch(extent));
        }
        let mut state = self.state()?;
        let current = state.extents.get(&extent).map(|entry| entry.len);
        if current.is_some() && !state.open.contains_key(&extent) {
            bail!(SealedExtent(extent));
        }
        let end = offset + len;
        let grows = end.saturating_sub(current.unwrap_or(0));
        if state.stored + state.reserved + grows > self.description.capacity {
            bail!(NoSpace);
        }
        let placement = match state.open.get(&extent) {
            Some(placement) => placement.clone(),
            None => {
                let placement = if end >= self.options.large_extent {
                    let path = extent_path(extent);
                    let file =
                        self.dir.create(&path, true).with_context(|| format!("creating {path}"))?;
                    state.dirty_dirs.insert(EXTENTS);
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
                state.dirty_own.insert(extent, write.file.clone());
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
        let mut state = self.state()?;
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
                state.doomed.push(extent_path(extent));
            }
            Data::Remote => {}
        }
    }

    /// fxdax: holds `extent`, sealed, of `len` bytes, whose data is elsewhere: reads of it fail
    /// with [`RemoteData`], and the store's user reads it from there. A branch kept in object
    /// storage, opened where its data is not, holds its extents so.
    pub fn adopt(&self, extent: ExtentId, len: u64) -> Result<(), Error> {
        let mut state = self.state()?;
        ensure!(!state.extents.contains_key(&extent), "the store holds {extent:?}");
        state.extents.insert(extent, Extent { len, data: Data::Remote });
        state.stored += len;
        if extent.branch == self.description.branch {
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
        let mut state = self.state()?;
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
            uploads.committed.push_back(committed);
        }
        let mut syncs: Vec<Arc<D::File>> =
            std::mem::take(&mut state.dirty_own).into_values().collect();
        for number in std::mem::take(&mut state.dirty_segments) {
            syncs.push(self.segment_file(state, number)?);
        }
        let dirs = std::mem::take(&mut state.dirty_dirs);
        let mut doomed = std::mem::take(&mut state.doomed);
        // The segments whose extents are all freed or moved, by this commit, if not before.
        // Writes go to the open one, and to one just filled, until they finish.
        let dead: Vec<u64> = state
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
            doomed.push(segment_path(number));
        }
        // Once the open segment is full, writes go to a new one. Writes finishing in the old one
        // belong to the next commit, as do the new one's directory entry.
        if state.segments[&state.open_segment].len >= self.options.segment_size {
            let number = state.open_segment + 1;
            let file = Arc::new(self.dir.create(&segment_path(number), true)?);
            state
                .segments
                .insert(number, Segment { file: Some(file), len: 0, live: 0, writers: 0 });
            state.open_files += 1;
            state.open_segment = number;
            state.dirty_dirs.insert(SEGMENTS);
        }
        let checkpoint = state.index.deltas >= self.options.checkpoint_after;
        let record = encode(&if checkpoint {
            Record::Checkpoint(Checkpoint {
                epoch,
                branch: self.description.branch,
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
            let file = match self.dir.create(&index_path(number), false) {
                Ok(file) => Arc::new(file),
                Err(error) => {
                    state.failed = true;
                    return Err(Error::from(error).context("committing the extent store"));
                }
            };
            doomed.push(index_path(state.index.number));
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
                self.dir.sync_dir(dir).with_context(|| format!("syncing {dir}"))?;
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
                self.dir.sync_dir(INDEX)?;
            }
            self.dir.barrier()?;
            Ok(())
        };
        if let Err(error) = durable() {
            self.state.lock().unwrap().failed = true;
            return Err(error.context("committing the extent store"));
        }
        if let Some(uploads) = &mut self.state.lock().unwrap().uploads {
            uploads.durable = uploads.durable.max(pending.last().unwrap().epoch);
        }
        for path in pending.into_iter().flat_map(|pending| pending.doomed) {
            if let Err(err) = self.dir.remove(&path)
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

    /// Cleans the segments whose extents' data takes less than `live_fraction` of them: copies
    /// the data of their extents that are sealed to the open segment, and commits, which removes
    /// them.
    pub fn clean(&self, live_fraction: f64) -> Result<Cleaned, Error> {
        let committing = self.committing.lock().unwrap();
        let (victims, moves) = {
            let state = self.state()?;
            let victims: BTreeMap<u64, u64> = state
                .segments
                .iter()
                .filter(|(number, segment)| {
                    **number != state.open_segment
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
        let capacity = self.description.capacity;
        Ok(Info {
            capacity,
            used: state.stored,
            available: capacity.saturating_sub(state.stored + state.reserved),
            next_sequence: state.next_sequence,
        })
    }

    pub fn usage(&self) -> Result<Usage, Error> {
        let state = self.state()?;
        Ok(Usage {
            segments: state.segments.len() as u64,
            segment_bytes: state.segments.values().map(|segment| segment.len).sum(),
            live_bytes: state.segments.values().map(|segment| segment.live).sum(),
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
        if self.flush_pending()
            && let Err(err) = self.flush()
        {
            log::error!("extent store: making the last commits durable: {err:#}");
        }
    }
}

/// A part of an extent: its start and end, and the segment and offset of its data, or `None` where
/// nothing was written.
type Span = (u64, u64, Option<(u64, u64)>);

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
        let piece =
            |segment, offset, extent_offset, len| Piece { segment, offset, extent_offset, len };
        // Nothing written.
        assert_eq!(paint(&[], 0, 8), vec![(0, 8, None)]);
        // A piece in the middle, and one over its end.
        let pieces = [piece(1, 100, 2, 4), piece(2, 200, 5, 2)];
        assert_eq!(
            paint(&pieces, 0, 8),
            vec![(0, 2, None), (2, 5, Some((1, 100))), (5, 7, Some((2, 200))), (7, 8, None)]
        );
        // A later piece within an earlier one splits it.
        let pieces = [piece(1, 100, 0, 8), piece(2, 200, 3, 2)];
        assert_eq!(
            paint(&pieces, 1, 7),
            vec![(1, 3, Some((1, 101))), (3, 5, Some((2, 200))), (5, 7, Some((1, 105)))]
        );
    }
}
