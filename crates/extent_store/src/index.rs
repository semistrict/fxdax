// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's index (see the crate's docs): its state recovered from its newest checkpoint and the
//! deltas after it, or rebuilt from that checkpoint and the footers of its closed segments after
//! it; its states as of its forks, which they inherited, read from the index files it keeps for
//! them; and the log its open store appends its commits' records to ([`IndexLog`]), whose
//! checkpoints replace the files before, but for those forks need.

use crate::dir::{Dir, File, read_file};
use crate::framed::read_frame;
use crate::layers::{Extents, Layer};
use crate::layout::{
    Change, Checkpoint, Data, Delta, Extent, FileKey, INDEX, Piece, Record, SEGMENTS, SegmentId,
    branch_dir, create_branch_dirs, decode, encode, index_path, list_numbers, segment_path,
};
use crate::segments::{Reaped, footer_last, segment_footer};
use crate::store::Options;
use anyhow::{Error, anyhow, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Applies `change`, recovering it from an index.
fn apply(extents: &mut Extents, change: Change) {
    match change {
        Change::Write { extent, piece } => {
            let entry = extents
                .get_or_insert_with(extent, || Extent { len: 0, data: Data::Pieces(Vec::new()) });
            entry.len = entry.len.max(piece.extent_offset + piece.len);
            if let Data::Pieces(pieces) = &mut entry.data {
                pieces.push(piece);
            }
        }
        Change::WriteOwn { extent, len } => {
            let entry = extents.get_or_insert_with(extent, || Extent { len: 0, data: Data::Own });
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

impl Checkpoint {
    /// The checkpoint of a branch as of commit `epoch`: its next sequence number, its root, the
    /// segment open, and its extents, `extents`, as the index of a branch over `origin` holds
    /// them: what they hold over `origin`, or, with none, all they hold.
    pub(crate) fn of(
        epoch: u64,
        next_sequence: u64,
        root: &[u8],
        segment: u64,
        extents: &Extents,
        origin: Option<&Arc<Layer>>,
    ) -> Self {
        let (extents, gone) = extents.overlay(origin);
        Self { epoch, next_sequence, root: root.to_vec(), segment, extents, gone }
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
    pub extents: Extents,
}

impl Recovered {
    /// The state a checkpoint holds, over `base`, the layer the branch's index is over, if any.
    fn of(checkpoint: Checkpoint, base: Option<Arc<Layer>>) -> Self {
        let mut extents = Extents::over(base);
        for (extent, entry) in checkpoint.extents {
            extents.insert(extent, entry);
        }
        for extent in checkpoint.gone {
            extents.remove(&extent);
        }
        Self {
            epoch: checkpoint.epoch,
            next_sequence: checkpoint.next_sequence,
            root: checkpoint.root,
            segment: checkpoint.segment,
            extents,
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

    /// Its checkpoint, as the index of a branch over `origin` holds it: what it holds over
    /// `origin`, or, with none, all it holds.
    fn checkpoint(&self, origin: Option<&Arc<Layer>>) -> Checkpoint {
        Checkpoint::of(
            self.epoch,
            self.next_sequence,
            &self.root,
            self.segment,
            &self.extents,
            origin,
        )
    }
}

/// What `branch`'s index file `number` holds, if it begins with a whole checkpoint: the
/// checkpoint, and the deltas after it that are whole, in order.
fn read_index<D: Dir>(
    dir: &D,
    branch: u64,
    number: u64,
) -> Result<Option<(Checkpoint, Vec<Delta>)>, Error> {
    let file = dir.open(&index_path(branch, number), false)?;
    let mut records = decode(&read_file(&file)?).0.into_iter();
    let Some(Record::Checkpoint(table)) = records.next() else { return Ok(None) };
    let deltas = (records)
        .map_while(|record| match record {
            Record::Delta(delta) => Some(delta),
            Record::Checkpoint(_) => None,
        })
        .collect();
    Ok(Some((Checkpoint::of_table(&table)?, deltas)))
}

/// What `branch`'s newest index file holding a whole checkpoint holds ([`read_index`]): the files
/// after it hold none, torn by a crash.
fn newest_checkpoint<D: Dir>(dir: &D, branch: u64) -> Result<(Checkpoint, Vec<Delta>), Error> {
    let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
    for &number in numbers.iter().rev() {
        if let Some(index) = read_index(dir, branch, number)? {
            return Ok(index);
        }
        log::warn!("extent store: branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    }
    bail!("branch {branch:#x} has no checkpoint")
}

/// `branch`'s state as of its last commit that is durable: its newest index file whose checkpoint
/// is whole, with the deltas after it that are whole and follow on; over `base`, the layer its
/// index is over, if any.
pub(crate) fn recover<D: Dir>(
    dir: &D,
    branch: u64,
    base: Option<Arc<Layer>>,
) -> Result<Recovered, Error> {
    let (checkpoint, deltas) = newest_checkpoint(dir, branch)?;
    let mut recovered = Recovered::of(checkpoint, base);
    for delta in deltas {
        if !recovered.apply(delta) {
            break;
        }
    }
    Ok(recovered)
}

/// `branch`'s state as of each of `epochs`, from its index file `number`, which holds them, over
/// `base`, the layer its index is over, if any: what forks of it at those points inherited. Layers,
/// each over the one before, which share what did not change between them: read once, for all
/// the forks of the branch there (TASK-31).
pub(crate) fn layers_at<D: Dir>(
    dir: &D,
    branch: u64,
    number: u64,
    epochs: &BTreeSet<u64>,
    base: Option<Arc<Layer>>,
) -> Result<BTreeMap<u64, Arc<Layer>>, Error> {
    let Some((checkpoint, deltas)) = read_index(dir, branch, number)? else {
        bail!("branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    };
    let mut deltas = deltas.into_iter();
    let origin = base.clone();
    let mut recovered = Recovered::of(checkpoint, base);
    let mut layers = BTreeMap::new();
    let mut wanted = epochs.iter().copied().peekable();
    while let Some(&epoch) = wanted.peek() {
        ensure!(
            epoch >= recovered.epoch,
            "branch {branch:#x}'s index file {number:#x} begins after epoch {epoch}"
        );
        if epoch == recovered.epoch {
            let layer = recovered.extents.freeze(origin.as_ref(), &recovered.root);
            layers.insert(epoch, layer);
            wanted.next();
            continue;
        }
        let Some(delta) = deltas.next() else {
            bail!("branch {branch:#x}'s index file {number:#x} ends before epoch {epoch}");
        };
        ensure!(
            recovered.apply(delta),
            "branch {branch:#x}'s index file {number:#x} skips an epoch before {epoch}"
        );
    }
    Ok(layers)
}

/// Writes `branch`'s index file `number`, of a checkpoint of `state`, durably, and returns it: of
/// what it holds over `origin`, the layer its index is over, if any, or of all it holds.
pub(crate) fn write_checkpoint<D: Dir>(
    dir: &D,
    branch: u64,
    number: u64,
    state: &Recovered,
    origin: Option<&Arc<Layer>>,
) -> Result<(D::File, u64), Error> {
    let bytes = encode(&Record::Checkpoint(state.checkpoint(origin).table()?));
    let file = dir.create(&index_path(branch, number), false)?;
    file.write_at(&bytes, 0)?;
    file.sync()?;
    dir.sync_dir(&branch_dir(INDEX, branch))?;
    Ok((file, bytes.len() as u64))
}

/// The state of `branch`, a fork of the state `origin` at its first opening, and its first index:
/// a branch of its own, which has minted nothing, committed nothing, and changed nothing over
/// `origin`, durably.
pub(crate) fn start_fork<D: Dir>(
    dir: &D,
    branch: u64,
    origin: &Arc<Layer>,
) -> Result<Recovered, Error> {
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
    Ok(state)
}

/// `branch`'s state from its newest checkpoint and the footers of its closed segments after it:
/// the segment open as of the checkpoint, and each closed after it that follows on; over `base`,
/// the layer its index is over, if any.
pub(crate) fn rebuild<D: Dir>(
    dir: &D,
    branch: u64,
    base: Option<Arc<Layer>>,
) -> Result<Recovered, Error> {
    let (checkpoint, _) = newest_checkpoint(dir, branch)?;
    let mut state = Recovered::of(checkpoint, base);
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

/// The index an open branch appends its commits' records to: a delta each, in its current file,
/// or a checkpoint, in a new file, once the deltas are many, the closed segments only their
/// footers keep take a segment's room, or the branch wants one. Every index file of the branch's
/// but the current is retained, as forks need it, or is to go.
pub(crate) struct IndexLog<F> {
    branch: u64,
    /// The current file, its number, its bytes, and the bytes of deltas it holds.
    number: u64,
    file: Arc<F>,
    len: u64,
    deltas: u64,
    /// The branch's index files replaced, which forks not yet opened need still.
    retained: BTreeSet<u64>,
    /// The epoch of its newest checkpoint.
    checkpointed: u64,
    /// Whether the next record is a checkpoint: the branch adopted extents, which footers don't
    /// name.
    wanted: bool,
    /// The branch's closed segments no extent is in any more, with the last epochs their footers
    /// cover and their sizes, which go once a checkpoint covers them.
    uncovered: Vec<(SegmentId, u64, u64)>,
}

/// A commit's record, to be written at `offset` in `file`, the index's, by the flush that makes the
/// commit durable: a checkpoint, beginning a new file, if `new_file`.
pub(crate) struct IndexWrite<F> {
    record: Vec<u8>,
    file: Arc<F>,
    offset: u64,
    new_file: bool,
}

/// A commit's record, appended ([`IndexLog::append`]), and what it made unreferenced, if it is a
/// checkpoint: the index files it replaced, but those forks need, which go once it is durable;
/// and the closed segments it covers the footers of, which no extent is in.
pub(crate) struct Appended<F> {
    pub write: IndexWrite<F>,
    pub replaced: Vec<String>,
    pub covered: Vec<FileKey>,
}

impl<F: File> IndexLog<F> {
    /// `branch`'s index as its opening starts it: a new file, of a checkpoint of `state`, of what
    /// it holds over `origin`, the layer its index is over, if any, which replaces the files before
    /// once durable, but for those forks need, which `pinned` says.
    pub fn open<D: Dir<File = F>>(
        dir: &D,
        branch: u64,
        state: &Recovered,
        origin: Option<&Arc<Layer>>,
        pinned: impl FnOnce() -> BTreeSet<u64>,
    ) -> Result<Self, Error> {
        let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
        let number = numbers.last().map_or(0, |number| number + 1);
        let (file, len) = write_checkpoint(dir, branch, number, state, origin)?;
        dir.barrier()?;
        let mut index = Self {
            branch,
            number,
            file: Arc::new(file),
            len,
            deltas: 0,
            retained: BTreeSet::new(),
            checkpointed: state.epoch,
            wanted: false,
            uncovered: Vec::new(),
        };
        for path in index.replace(numbers, &pinned()) {
            dir.remove(&path)?;
        }
        Ok(index)
    }

    /// The number of its current file.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// Has the next record be a checkpoint: the branch adopted extents, which footers don't name.
    pub fn want_checkpoint(&mut self) {
        self.wanted = true;
    }

    /// Lets the segments `reaped` go, which no extent is in any more, and returns those that go
    /// now: those the branch closed since it was opened go once a checkpoint covers their footers.
    pub fn let_go(&mut self, reaped: Vec<Reaped>) -> Vec<FileKey> {
        let mut released = Vec::new();
        for Reaped { number, last, len } in reaped {
            match last {
                Some(last) if last > self.checkpointed => self.uncovered.push((number, last, len)),
                _ => released.push(FileKey::Segment(number)),
            }
        }
        released
    }

    /// Appends a commit's record: its delta, `delta`, or, if one is due, as `options` say, a
    /// checkpoint of the branch's state as of it, `checkpoint()`, in a new file in `dir`, which
    /// replaces the files before once the commit is durable, but for those forks need, which
    /// `pinned` says.
    pub fn append<D: Dir<File = F>>(
        &mut self,
        dir: &D,
        delta: Delta,
        checkpoint: impl FnOnce() -> Checkpoint,
        options: &Options,
        pinned: impl FnOnce() -> BTreeSet<u64>,
    ) -> Result<Appended<F>, Error> {
        // A checkpoint once the index holds enough deltas, or the closed segments only footers need
        // take a segment's room, or the branch wants one: which any answers (TASK-84).
        let uncovered: u64 = self.uncovered.iter().map(|(_, _, size)| size).sum();
        let wanted = std::mem::take(&mut self.wanted);
        let due =
            wanted || self.deltas >= options.checkpoint_after || uncovered >= options.segment_size;
        if !due {
            // After the records before it.
            let record = encode(&Record::Delta(delta));
            let offset = self.len;
            self.len += record.len() as u64;
            self.deltas += record.len() as u64;
            let write = IndexWrite { record, file: self.file.clone(), offset, new_file: false };
            return Ok(Appended { write, replaced: Vec::new(), covered: Vec::new() });
        }
        let record = encode(&Record::Checkpoint(checkpoint().table()?));
        // A new file, which replaces the last once this is durable, but for what forks need.
        let number = self.number + 1;
        let file = Arc::new(dir.create(&index_path(self.branch, number), false)?);
        let mut numbers = std::mem::take(&mut self.retained);
        numbers.insert(self.number);
        let replaced = self.replace(numbers, &pinned());
        (self.number, self.file, self.len, self.deltas) =
            (number, file.clone(), record.len() as u64, 0);
        // The closed segments its checkpoint covers no longer need their footers.
        self.checkpointed = delta.epoch;
        let (covered, uncovered): (Vec<_>, Vec<_>) = std::mem::take(&mut self.uncovered)
            .into_iter()
            .partition(|(_, last, _)| *last <= delta.epoch);
        self.uncovered = uncovered;
        let covered = covered.into_iter().map(|(number, ..)| FileKey::Segment(number)).collect();
        let write = IndexWrite { record, file, offset: 0, new_file: true };
        Ok(Appended { write, replaced, covered })
    }

    /// Replaces the branch's index files `numbers`, by the current: retains those forks need,
    /// `pinned`, and returns the paths of the others, which are to go.
    fn replace(
        &mut self,
        numbers: impl IntoIterator<Item = u64>,
        pinned: &BTreeSet<u64>,
    ) -> Vec<String> {
        let (retained, gone): (BTreeSet<u64>, BTreeSet<u64>) =
            numbers.into_iter().partition(|number| pinned.contains(number));
        self.retained = retained;
        gone.into_iter().map(|number| index_path(self.branch, number)).collect()
    }
}

/// Writes `writes`, commits' records in `branch`'s index in `dir`, in order, durably but for the
/// barrier: the records, then the files they are in, each synced once, then, if a record began a
/// file, the branch's directory of index files.
pub(crate) fn write_records<'a, D: Dir>(
    dir: &D,
    branch: u64,
    writes: impl IntoIterator<Item = &'a IndexWrite<D::File>>,
) -> Result<(), Error> {
    let mut files: Vec<&Arc<D::File>> = Vec::new();
    let mut new_file = false;
    for write in writes {
        write.file.write_at(&write.record, write.offset)?;
        if !files.iter().any(|file| Arc::ptr_eq(file, &write.file)) {
            files.push(&write.file);
        }
        new_file |= write.new_file;
    }
    for file in files {
        file.sync()?;
    }
    if new_file {
        dir.sync_dir(&branch_dir(INDEX, branch))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemDir, MemFile};

    const BRANCH: u64 = 7;

    /// Checkpoints after `checkpoint_after` bytes of deltas, or 64 KiB of segments' footers.
    fn options(checkpoint_after: u64) -> Options {
        Options { checkpoint_after, segment_size: 64 << 10, ..Options::default() }
    }

    /// A branch's state as of `epoch`, holding nothing.
    fn state(epoch: u64) -> Recovered {
        Recovered { epoch, ..Recovered::default() }
    }

    fn delta(epoch: u64) -> Delta {
        Delta { epoch, next_sequence: 0, root: None, changes: Vec::new() }
    }

    fn checkpoint(epoch: u64) -> impl FnOnce() -> Checkpoint {
        move || state(epoch).checkpoint(None)
    }

    /// The bytes of the record of `record`.
    fn len(record: Record) -> u64 {
        encode(&record).len() as u64
    }

    fn checkpoint_len(epoch: u64) -> u64 {
        len(Record::Checkpoint(checkpoint(epoch)().table().unwrap()))
    }

    fn segment(number: u64) -> SegmentId {
        SegmentId { branch: BRANCH, number }
    }

    /// A directory holding the branch's index files `numbers`, each a checkpoint.
    fn holding(numbers: &[u64]) -> MemDir {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        for &number in numbers {
            write_checkpoint(&dir, BRANCH, number, &state(number), None).unwrap();
        }
        dir
    }

    fn numbers(dir: &MemDir) -> Vec<u64> {
        list_numbers(dir, &branch_dir(INDEX, BRANCH)).unwrap()
    }

    /// Appends commit `epoch`'s record, and writes it, as its flush does.
    fn commit(
        index: &mut IndexLog<MemFile>,
        dir: &MemDir,
        epoch: u64,
        options: &Options,
    ) -> Appended<MemFile> {
        let appended =
            index.append(dir, delta(epoch), checkpoint(epoch), options, BTreeSet::new).unwrap();
        write_records(dir, BRANCH, [&appended.write]).unwrap();
        appended
    }

    /// Opening the index replaces its files, at once, but those forks need, which it retains; a
    /// checkpoint replaces those and the file before it, once durable, but those forks still need.
    #[test]
    fn replaced_index_files_are_retained_while_forks_need_them() {
        let dir = holding(&[0, 1, 2]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(5), None, || [1].into()).unwrap();
        assert_eq!((index.number(), numbers(&dir)), (3, vec![1, 3]));
        let options = options(0);
        let appended =
            index.append(&dir, delta(6), checkpoint(6), &options, || [1].into()).unwrap();
        assert_eq!((index.number(), appended.replaced), (4, vec![index_path(BRANCH, 3)]));
        let appended =
            index.append(&dir, delta(7), checkpoint(7), &options, BTreeSet::new).unwrap();
        let replaced = vec![index_path(BRANCH, 1), index_path(BRANCH, 4)];
        assert_eq!((index.number(), appended.replaced), (5, replaced));
    }

    /// Commits' deltas follow each other in the current file until they take
    /// `Options::checkpoint_after`: then a checkpoint begins a new one.
    #[test]
    fn deltas_append_until_a_checkpoint_is_due() {
        let dir = holding(&[]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(0), None, BTreeSet::new).unwrap();
        let delta_len = len(Record::Delta(delta(1)));
        let options = options(2 * delta_len);
        let places: Vec<_> = (1..=4)
            .map(|epoch| {
                let write = commit(&mut index, &dir, epoch, &options).write;
                (index.number(), write.offset, write.new_file)
            })
            .collect();
        let (first, second) = (checkpoint_len(0), checkpoint_len(3));
        assert_eq!(
            places,
            [(0, first, false), (0, first + delta_len, false), (1, 0, true), (1, second, false)]
        );
        assert_eq!(recover(&dir, BRANCH, None).unwrap().epoch, 4);
    }

    /// The segments reaped that the branch closed after its newest checkpoint stay, their footers
    /// its history, until a checkpoint covers them: one comes once they take a segment's room.
    #[test]
    fn reaped_segments_go_once_a_checkpoint_covers_their_footers() {
        let dir = holding(&[]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(2), None, BTreeSet::new).unwrap();
        let reaped = |number, last| Reaped { number: segment(number), last, len: 40 << 10 };
        // Closed by an earlier opening, and before the checkpoint it opened with.
        let released = index.let_go(vec![reaped(0, None), reaped(1, Some(2)), reaped(2, Some(3))]);
        assert_eq!(released, [FileKey::Segment(segment(0)), FileKey::Segment(segment(1))]);
        let options = options(1 << 20);
        let appended = commit(&mut index, &dir, 3, &options);
        assert_eq!((appended.write.new_file, appended.covered), (false, vec![]));
        assert_eq!(index.let_go(vec![reaped(3, Some(4))]), []);
        let appended = commit(&mut index, &dir, 4, &options);
        let covered = vec![FileKey::Segment(segment(2)), FileKey::Segment(segment(3))];
        assert_eq!((appended.write.new_file, appended.covered), (true, covered));
        // Covered, they go at once.
        assert_eq!(index.let_go(vec![reaped(4, Some(4))]), [FileKey::Segment(segment(4))]);
    }

    /// A branch wanting a checkpoint has its next commit's record be one, and the next a delta.
    #[test]
    fn a_wish_for_a_checkpoint_is_answered() {
        let dir = holding(&[]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(0), None, BTreeSet::new).unwrap();
        let options = options(1 << 20);
        index.want_checkpoint();
        let new_files: Vec<_> =
            (1..=2).map(|epoch| commit(&mut index, &dir, epoch, &options).write.new_file).collect();
        assert_eq!(new_files, [true, false]);
    }

    /// A checkpoint, whatever called for it, answers a wish for one: the deltas being many, the
    /// next record after it is a delta (TASK-84).
    #[test]
    fn a_checkpoint_due_answers_a_wish_for_one() {
        let dir = holding(&[]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(0), None, BTreeSet::new).unwrap();
        let options = options(len(Record::Delta(delta(1))));
        commit(&mut index, &dir, 1, &options);
        index.want_checkpoint();
        let new_files: Vec<_> =
            (2..=3).map(|epoch| commit(&mut index, &dir, epoch, &options).write.new_file).collect();
        assert_eq!(new_files, [true, false]);
    }

    /// Recovery starts from the newest index file whose checkpoint is whole, with the whole deltas
    /// after it; rebuilding, from that checkpoint, with the footers after it.
    #[test]
    fn recovery_and_rebuilding_start_from_the_newest_whole_checkpoint() {
        let dir = holding(&[]);
        let mut index = IndexLog::open(&dir, BRANCH, &state(1), None, BTreeSet::new).unwrap();
        let options = options(1 << 20);
        // File 0: checkpoint 1, and deltas 2 and 3. File 1: checkpoint 4, delta 5, a torn record.
        for epoch in 2..=3 {
            commit(&mut index, &dir, epoch, &options);
        }
        index.want_checkpoint();
        commit(&mut index, &dir, 4, &options);
        let write = commit(&mut index, &dir, 5, &options).write;
        write.file.write_at(&[1; 7], write.offset + write.record.len() as u64).unwrap();
        // File 2: begun by a checkpoint not yet written.
        dir.create(&index_path(BRANCH, 2), false).unwrap();
        assert_eq!(numbers(&dir), [0, 1, 2]);
        assert_eq!(recover(&dir, BRANCH, None).unwrap().epoch, 5);
        assert_eq!(rebuild(&dir, BRANCH, None).unwrap().epoch, 4);
    }
}
