// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's index (see the crate's docs): its state recovered from its newest checkpoint and the
//! deltas after it, or rebuilt from that checkpoint and the footers of its closed segments after
//! it; and its states as of its forks, which they inherited, read from the index files it keeps
//! for them.

use crate::dir::{Dir, File as _, read_file};
use crate::framed::read_frame;
use crate::layers::{Extents, Layer};
use crate::layout::{
    Change, Checkpoint, Data, Delta, Extent, INDEX, Piece, Record, SEGMENTS, SegmentId, branch_dir,
    decode, encode, index_path, segment_path,
};
use crate::lineage::list_numbers;
use crate::segments::{footer_last, segment_footer};
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
        let (extents, gone) = self.extents.overlay(origin);
        Checkpoint {
            epoch: self.epoch,
            next_sequence: self.next_sequence,
            root: self.root.clone(),
            segment: self.segment,
            extents,
            gone,
        }
    }
}

/// `branch`'s state as of its last commit that is durable: its newest index file whose checkpoint
/// is whole, with the deltas after it that are whole and follow on; over `base`, the layer its
/// index is over, if any.
pub(crate) fn recover<D: Dir>(
    dir: &D,
    branch: u64,
    base: Option<Arc<Layer>>,
) -> Result<Recovered, Error> {
    let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
    for &number in numbers.iter().rev() {
        let file = dir.open(&index_path(branch, number), false)?;
        let mut records = decode(&read_file(&file)?).0.into_iter();
        if let Some(Record::Checkpoint(table)) = records.next() {
            let mut recovered = Recovered::of(Checkpoint::of_table(&table)?, base.clone());
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
    let file = dir.open(&index_path(branch, number), false)?;
    let mut records = decode(&read_file(&file)?).0.into_iter();
    let Some(Record::Checkpoint(table)) = records.next() else {
        bail!("branch {branch:#x}'s index file {number:#x} holds no checkpoint");
    };
    let origin = base.clone();
    let mut recovered = Recovered::of(Checkpoint::of_table(&table)?, base);
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
        let Some(Record::Delta(delta)) = records.next() else {
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

/// `branch`'s state from its newest checkpoint and the footers of its closed segments after it:
/// the segment open as of the checkpoint, and each closed after it that follows on; over `base`,
/// the layer its index is over, if any.
pub(crate) fn rebuild<D: Dir>(
    dir: &D,
    branch: u64,
    base: Option<Arc<Layer>>,
) -> Result<Recovered, Error> {
    let numbers = list_numbers(dir, &branch_dir(INDEX, branch))?;
    let mut state = None;
    for &number in numbers.iter().rev() {
        let file = dir.open(&index_path(branch, number), false)?;
        if let Some(Record::Checkpoint(table)) = decode(&read_file(&file)?).0.into_iter().next() {
            state = Some(Recovered::of(Checkpoint::of_table(&table)?, base));
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
