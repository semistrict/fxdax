// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a store's write-ahead log of its branch (doc-14, TASK-165.9): what the bucket doesn't
//! have yet of the branch, on local disk, and what recovering it from there takes.
//!
//! - **Its segments**, closed and not yet acknowledged in the bucket: each framed with its footer,
//!   `segments/<branch>/<n>`, the object it is to be in the bucket, and the extents its footer
//!   names stored on their own, `extents/<branch>/<sequence>`. They are framed in the order they
//!   closed, one after another, so that a segment whose framed file is whole follows segments
//!   whose framed files are whole too.
//! - **The open segment's log** of the commits sealed in it, `logs/<branch>/<n>`, beside its raw
//!   data, `open-segments/<branch>/<n>`: a record of each commit's contents, as its footer will
//!   fold them (`extent_frame::segment::Contents`), each hashed, appended and synced by the flush
//!   that makes the commit durable, after the data it names. A segment's closing makes its framed
//!   file the record of its commits; its raw data and log go once the commits it names are durable.
//! - **Its base**, `wal`: the branch's commit its first segment goes on from, which the store
//!   writes, durably, before the segments it acknowledged leave it ([`BaseFile`]): a segment a
//!   crash brings back, its removal not durable, whose last commit is the base's or before, is
//!   acknowledged, and goes again.
//!
//! Recovering it ([`recover`]) keeps each segment whose framed file is whole, to the first whose
//! framed file is not, left open, or its framing or its entry's sync stopped by a crash, which it
//! closes again from its raw data and the records of its log that are whole, which are the commits
//! made durable, at least: a footer of their contents folded, as the segment's closing would have;
//! one whose log holds none held no durable commit, and goes. A flush makes no commit durable
//! before every segment closed before it is framed, and its framed file's entry durable, so that a
//! segment after one not whole holds no durable commit, though its framed file be whole: it goes,
//! durably, before the one before is closed again, whose footer lacks the commit that closed it,
//! which the one after followed (TASK-165.9's crash rounds, cutting openings). The footers recovered
//! are stamped as they were, or provisionally for those closed again: opening the store puts the
//! log after the branch's end by its commits (`extent_remote::Wal::rebase`), and stamps them anew.

use crate::dir::{Dir, File, not_found, read_file};
use crate::framed::{Aligned, read_frame, read_whole};
use crate::layout::{
    LOGS, OPEN_SEGMENTS, SEGMENTS, SegmentId, branch_dir, decode, encode, extent_path,
    list_numbers, log_path, open_segment_path, segment_path,
};
use crate::segments::{fold, footer_last, segment_footer};
use anyhow::{Context as _, Error, anyhow};
use extent_frame::segment::{Chain, Contents, Footer, epoch_of, writer_of};
use extent_remote::wal::Fenced;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// The file of the WAL's base.
pub(crate) const BASE: &str = "wal";

/// The bytes of each of the base file's two slots: a record each, written whole or torn alone.
const SLOT: u64 = 4096;

/// A slot of the base file: the base, and the serial number of its writing, which the later
/// writing's exceeds.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Slot {
    serial: u64,
    base: Option<u64>,
}

/// The WAL's base, kept durably in two slots of [`BASE`]: each writing writes the slot the latest
/// is not in, and syncs it, so that a writing a crash tears leaves the one before. Read, it is the
/// whole slot of the higher serial number; none, before the first writing. Opened for each
/// writing, which comes with an acknowledgement, rather than held open: a host serves hundreds
/// of stores, forks most of them, which acknowledge little.
pub(crate) struct BaseFile {
    made: bool,
    serial: u64,
}

impl BaseFile {
    /// The base file of the store in `dir`, and the base it holds.
    pub fn read<D: Dir>(dir: &D) -> Result<(Self, Option<u64>), Error> {
        let file = match dir.open(BASE, false) {
            Ok(file) => file,
            Err(err) if not_found(&err) => return Ok((Self { made: false, serial: 0 }, None)),
            Err(err) => return Err(Error::new(err).context("opening the WAL's base")),
        };
        let bytes = read_file(&file).context("reading the WAL's base")?;
        let slots = bytes.chunks(SLOT as usize).filter_map(|slot| decode::<Slot>(slot).0.pop());
        let latest = slots.max_by_key(|slot| slot.serial);
        let (serial, base) = latest.map_or((0, None), |slot| (slot.serial, slot.base));
        Ok((Self { made: true, serial }, base))
    }

    /// Writes `base`, durably, in `dir`: the file made, and its entry made durable, first if it
    /// isn't there.
    pub fn write<D: Dir>(&mut self, dir: &D, base: Option<u64>) -> Result<(), Error> {
        let file = match self.made {
            true => dir.open(BASE, false).context("opening the WAL's base")?,
            false => {
                let file = dir.create(BASE, false).context("making the WAL's base")?;
                dir.sync_dir("")?;
                self.made = true;
                file
            }
        };
        let serial = self.serial + 1;
        let record = encode(&Slot { serial, base });
        assert!(record.len() as u64 <= SLOT, "a base's record fits its slot");
        file.write_at(&record, (serial % 2) * SLOT).context("writing the WAL's base")?;
        file.sync().context("syncing the WAL's base")?;
        self.serial = serial;
        Ok(())
    }
}

/// The log of the commits of an open segment ([`log_path`]): where its next record goes, which the
/// flushes append, one at a time, in their commits' order.
pub(crate) struct SegmentLog<F> {
    pub segment: SegmentId,
    file: Arc<F>,
    end: u64,
}

impl<F: File> SegmentLog<F> {
    /// The log of `segment`, `file`, empty.
    pub fn new(segment: SegmentId, file: Arc<F>) -> Self {
        Self { segment, file, end: 0 }
    }

    /// Appends `records`, encoded, after those before, and syncs them.
    pub fn append<'a>(&mut self, records: impl IntoIterator<Item = &'a [u8]>) -> Result<(), Error> {
        let start = self.end;
        for record in records {
            self.file.write_at(record, self.end)?;
            self.end += record.len() as u64;
        }
        if self.end > start {
            self.file.sync()?;
        }
        Ok(())
    }
}

/// A store's WAL as its directory holds it, recovered ([`recover`]): its base; its segments,
/// footers by their numbers, in order; and the number of the next segment, past every one the
/// directory held.
pub(crate) struct Recovered {
    pub base: Option<u64>,
    pub base_file: BaseFile,
    pub segments: Vec<(Footer, u64)>,
    pub next: u64,
}

/// Recovers the WAL of `branch` in `dir` (see the module's docs): each segment whose framed file
/// is whole, in order, to the first whose framed file is not, which its log closes again if it
/// holds commits; and nothing else of segments: raw data, logs and framed files torn, closed again
/// or holding nothing durable, go.
pub(crate) fn recover<D: Dir>(dir: &D, branch: u64) -> Result<Recovered, Error> {
    let (base_file, base) = BaseFile::read(dir)?;
    let listed = |kind| -> Result<BTreeSet<u64>, Error> {
        Ok(list_numbers(dir, &branch_dir(kind, branch))?.into_iter().collect())
    };
    let (framed, raw, logs) = (listed(SEGMENTS)?, listed(OPEN_SEGMENTS)?, listed(LOGS)?);
    let numbers: BTreeSet<u64> = framed.iter().chain(&raw).chain(&logs).copied().collect();
    let left = |number| raw.contains(&number) || logs.contains(&number);
    // The segments whose framed files are whole, in order, to the first whose framed file is not
    // and whose log holds commits: those between whose logs hold none held no durable commit, set
    // aside or closed for want of room, and go.
    let mut whole: Vec<(Footer, u64)> = Vec::new();
    let mut numbers = numbers.into_iter();
    let mut torn = None;
    for number in numbers.by_ref() {
        let segment = SegmentId { branch, number };
        let footer = match framed.contains(&number) {
            true => whole_footer(dir, segment, left(number))?,
            false => None,
        };
        // A log's commits the base acknowledged are the bucket's: its framed file left, its raw
        // data and log a crash brought back, it holds nothing of the log's.
        let after_base = |records: &[Contents]| {
            let last = records.iter().rev().find_map(|record| record.commits.last().copied());
            last.is_some_and(|last| base.is_none_or(|base| epoch_of(last) > epoch_of(base)))
        };
        match footer {
            Some(footer) => whole.push((footer, number)),
            None if raw.contains(&number) && after_base(&log_records(dir, segment)?) => {
                torn = Some(number);
                break;
            }
            None => {
                for path in [segment_path(segment), open_segment_path(segment), log_path(segment)] {
                    remove(dir, &path)?;
                }
            }
        }
    }
    // Those after it hold no durable commit (see the module's docs): they go first, durably, so
    // that no crash finds one after it closed again, which the closing commit it lacks would leave
    // a gap before.
    let after: Vec<u64> = numbers.collect();
    if !after.is_empty() {
        for number in after {
            let segment = SegmentId { branch, number };
            for path in [segment_path(segment), open_segment_path(segment), log_path(segment)] {
                remove(dir, &path)?;
            }
        }
        for kind in [SEGMENTS, OPEN_SEGMENTS, LOGS] {
            dir.sync_dir(&branch_dir(kind, branch))?;
        }
        dir.barrier()?;
    }
    // What a segment holds is in its framed file, durably, or nothing durable is: its raw data and
    // log go.
    let settled = |number| -> Result<(), Error> {
        if left(number) {
            let segment = SegmentId { branch, number };
            dir.sync_dir(&branch_dir(SEGMENTS, branch))?;
            dir.barrier()?;
            remove(dir, &open_segment_path(segment))?;
            remove(dir, &log_path(segment))?;
        }
        Ok(())
    };
    for (_, number) in &whole {
        settled(*number)?;
    }
    let mut segments = whole;
    // The first not whole whose log holds commits, closed again from it.
    if let Some(number) = torn {
        let segment = SegmentId { branch, number };
        if framed.contains(&number) {
            remove(dir, &segment_path(segment))?;
        }
        let last = segments.last().map(|(footer, _)| footer);
        let footer = close_again(dir, segment, last)?;
        settled(number)?;
        segments.push((footer, number));
    }
    // Those acknowledged by the base, whose files' removal a crash undid, are the bucket's: they
    // go again.
    let acknowledged = base.map(epoch_of);
    let (gone, segments): (Vec<_>, Vec<_>) = segments
        .into_iter()
        .partition(|(footer, _)| acknowledged.is_some_and(|base| footer_last(footer) <= base));
    for (footer, number) in gone {
        remove(dir, &segment_path(SegmentId { branch, number }))?;
        for (extent, _) in footer.own() {
            remove(dir, &extent_path(extent))?;
        }
    }
    let next = (framed.iter().chain(&raw).chain(&logs)).max().map_or(0, |last| last + 1);
    Ok(Recovered { base, base_file, segments, next })
}

/// The footer of `segment`'s framed file, if it is whole: checked whole, data and all, if `left`,
/// its raw data and log still there, as its framing may have stopped part way; else whole, its
/// framing having made it durable before they went. Synced, if checked.
fn whole_footer<D: Dir>(dir: &D, segment: SegmentId, left: bool) -> Result<Option<Footer>, Error> {
    let path = segment_path(segment);
    let file = dir.open(&path, true)?;
    let footer = match read_frame(&file)? {
        Some((_, footer)) => segment_footer(&footer).ok().flatten(),
        None => None,
    };
    if !left {
        return footer.map(Some).ok_or_else(|| anyhow!("{path} is not a whole framed file"));
    }
    let whole = footer.is_some() && extent_frame::data(&read_whole(&file)?).is_ok();
    if whole {
        file.sync()?;
    }
    Ok(footer.filter(|_| whole))
}

/// The whole records of the log of `segment`, the commits it holds; none if it has no log.
fn log_records<D: Dir>(dir: &D, segment: SegmentId) -> Result<Vec<Contents>, Error> {
    match dir.open(&log_path(segment), false) {
        Ok(log) => Ok(decode(&read_file(&log)?).0),
        Err(err) if not_found(&err) => Ok(Vec::new()),
        Err(err) => Err(err.into()),
    }
}

/// Closes `segment` again, left open, or framed part way, from its raw data and the records of its
/// log that are whole, which hold a commit at least: framed with a footer of their contents folded,
/// stamped provisionally, after `last`, the footer before it, if any, and made durable. Returns
/// its footer.
fn close_again<D: Dir>(
    dir: &D,
    segment: SegmentId,
    last: Option<&Footer>,
) -> Result<Footer, Error> {
    let records = log_records(dir, segment)?;
    let first = records.first().context("a log closed again holds a commit")?;
    let mut chain = match last {
        Some(last) => {
            let after = Some((last.seq, last.epoch));
            Chain::new(segment.branch, last.epoch, last.seq + 1, after, last.contents.next_sequence)
        }
        None => Chain::new(segment.branch, writer_of(first.commits[0]), 0, None, 0),
    };
    let footer = chain.close(fold(segment.branch, &records));
    let raw = dir.open(&open_segment_path(segment), true)?;
    let mut data = Aligned::new(raw.size()? as usize);
    raw.read_cached(data.as_mut_slice(), 0)?;
    let path = segment_path(segment);
    remove(dir, &path)?;
    let file = dir.create(&path, true)?;
    file.write_at(&extent_frame::framed(data.as_slice(), &footer.encode()), 0)?;
    file.sync()?;
    Ok(footer)
}

/// Removes the file at `path` in `dir`, unless it is gone.
fn remove<D: Dir>(dir: &D, path: &str) -> Result<(), Error> {
    match dir.remove(path) {
        Err(err) if !not_found(&err) => Err(Error::new(err).context(format!("removing {path}"))),
        _ => Ok(()),
    }
}

/// A segment of the WAL, closed, and not yet acknowledged: its number, its footer, stamped by its
/// writer's chain, and, if its writer's chain stamped it anew after a takeover, which its framed
/// file says not, so ([`Queue`]); the bytes of its data and of the extents stored on their own it
/// names; and whether the uploader took it.
pub(crate) struct Queued {
    pub number: u64,
    pub footer: Footer,
    pub restamped: bool,
    pub bytes: u64,
    taken: bool,
}

/// The WAL's segments not yet acknowledged, in order, which the uploader takes once their
/// commits are durable and they are framed, and acknowledges in order; and its base, the branch's
/// commit before the first, which acknowledging moves on.
pub(crate) struct Queue {
    segments: VecDeque<Queued>,
    base: Option<u64>,
}

impl Queue {
    /// A queue of `segments`, after `base`.
    pub fn new(base: Option<u64>, segments: impl IntoIterator<Item = Queued>) -> Self {
        Self { segments: segments.into_iter().collect(), base }
    }

    /// Segment `number`, closed, whose footer is `footer`, holding `bytes` of data with the
    /// extents stored on their own its footer names, as its writer's chain stamped it as it closed.
    pub fn push(&mut self, number: u64, footer: Footer, bytes: u64) {
        self.segments.push_back(Queued::new(number, footer, false, bytes));
    }

    /// The WAL's base: the branch's commit before its first segment.
    pub fn base(&self) -> Option<u64> {
        self.base
    }

    /// The bytes of the segments not yet acknowledged: what the uploader's lag counts.
    pub fn bytes(&self) -> u64 {
        self.segments.iter().map(|queued| queued.bytes).sum()
    }

    /// Takes the segments not yet taken, in order, `limit` of them at most: up to the first of
    /// commits past `durable`, the last commit durable, or `framing` says is being framed, whose
    /// framed file is what is uploaded.
    pub fn take(
        &mut self,
        durable: u64,
        framing: impl Fn(u64) -> bool,
        limit: usize,
    ) -> Vec<&Queued> {
        let mut taken = Vec::new();
        for queued in self.segments.iter_mut().filter(|queued| !queued.taken) {
            let last = queued.footer.contents.commits.last().map_or(0, |&tag| epoch_of(tag));
            if taken.len() == limit || last > durable || framing(queued.number) {
                break;
            }
            queued.taken = true;
            taken.push(&*queued);
        }
        taken
    }

    /// Puts the queue after the branch's end, as a writer taking the branch again found it,
    /// `fenced`, by commits (`extent_remote::Wal::rebase`): returns the numbers of its segments
    /// that landed, the first, to be acknowledged; the rest are stamped anew, to be taken again,
    /// and the chain after the last of them, which the open segment's footer goes on from.
    /// `Diverged` if another writer's commits moved the branch past it.
    pub fn rebase(&mut self, fenced: &Fenced) -> Result<(Vec<u64>, Chain), extent_remote::Error> {
        let queued = self.segments.iter().map(|queued| (queued.footer.clone(), queued.number));
        let mut wal = extent_remote::Wal::of(self.base, queued);
        let landed = wal.rebase(fenced)?.into_iter().map(|(_, number)| number).collect();
        let mut chain = fenced.chain;
        let stamped: BTreeMap<u64, &Footer> =
            wal.segments().map(|(footer, number)| (*number, footer)).collect();
        for queued in &mut self.segments {
            let Some(&footer) = stamped.get(&queued.number) else { continue };
            chain = chain.admit(footer).expect("a log rebased follows the writer's chain");
            queued.restamped |= *footer != queued.footer;
            queued.footer = footer.clone();
            queued.taken = false;
        }
        Ok((landed, chain))
    }

    /// The first segment, if any.
    pub fn first(&self) -> Option<&Queued> {
        self.segments.front()
    }

    /// Segment `number`, the first, is acknowledged: it leaves the queue, which goes on from its
    /// last commit. Returns it; none if it is not the first.
    pub fn acknowledge(&mut self, number: u64) -> Option<Queued> {
        if self.segments.front().is_none_or(|queued| queued.number != number) {
            return None;
        }
        let queued = self.segments.pop_front().unwrap();
        self.base = queued.footer.contents.commits.last().copied().or(self.base);
        Some(queued)
    }
}

impl Queued {
    pub fn new(number: u64, footer: Footer, restamped: bool, bytes: u64) -> Self {
        Self { number, footer, restamped, bytes, taken: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemDir;
    use crate::layout::create_branch_dirs;
    use extent_frame::segment::tag;
    use storage_device::ExtentId;

    const BRANCH: u64 = 7;

    fn segment(number: u64) -> SegmentId {
        SegmentId { branch: BRANCH, number }
    }

    /// A commit of epoch `epoch`, sealing `extent` at `offset`, of a block.
    fn commit(epoch: u64, extent: u64, offset: u64) -> Contents {
        Contents {
            commits: vec![tag(2, epoch)],
            extents: vec![(ExtentId::new(BRANCH, extent), offset, 4096)],
            next_sequence: extent + 1,
            ..Contents::default()
        }
    }

    /// Writes segment `number`'s raw data, `blocks` blocks, and its log, of `records`, as an open
    /// segment's flushes leave them.
    fn left_open(dir: &MemDir, number: u64, blocks: u64, records: &[Contents]) {
        let raw = dir.create(&open_segment_path(segment(number)), true).unwrap();
        raw.write_at(&vec![number as u8; (blocks * 4096) as usize], 0).unwrap();
        let log = Arc::new(dir.create(&log_path(segment(number)), false).unwrap());
        let mut log = SegmentLog::new(segment(number), log);
        let records: Vec<Vec<u8>> = records.iter().map(encode).collect();
        log.append(records.iter().map(Vec::as_slice)).unwrap();
    }

    fn files(dir: &MemDir, kind: &'static str) -> Vec<u64> {
        list_numbers(dir, &branch_dir(kind, BRANCH)).unwrap()
    }

    /// The base is the last written, whole: a writing torn leaves the one before; none before the
    /// first.
    #[test]
    fn bases_are_the_last_written_whole() {
        let dir = MemDir::new();
        let (mut file, base) = BaseFile::read(&dir).unwrap();
        assert_eq!(base, None);
        for written in [Some(3), None, Some(9)] {
            file.write(&dir, written).unwrap();
            assert_eq!(BaseFile::read(&dir).unwrap().1, written);
        }
        // The next writing, to the slot of `None`'s, torn.
        let raw = dir.open(BASE, false).unwrap();
        raw.write_at(&[0xff; 8], 0).unwrap();
        assert_eq!(BaseFile::read(&dir).unwrap().1, Some(9));
    }

    /// A segment left open closes again from its raw data and its log's whole records, their
    /// contents folded into its footer, after the segments framed before it; one whose log holds
    /// none holds nothing durable, and goes. Raw data and logs go.
    #[test]
    fn segments_left_open_close_again_from_their_logs() {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        left_open(&dir, 3, 2, &[commit(5, 10, 0), commit(6, 11, 4096)]);
        // A torn record after them.
        let log = dir.open(&log_path(segment(3)), false).unwrap();
        log.write_at(&[1; 7], log.size().unwrap()).unwrap();
        left_open(&dir, 4, 1, &[]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let [(footer, 3)] = &recovered.segments[..] else {
            panic!("recovered {:?}", recovered.segments)
        };
        assert_eq!(footer.contents.commits, [tag(2, 6)]);
        let extents =
            [(ExtentId::new(BRANCH, 10), 0, 4096), (ExtentId::new(BRANCH, 11), 4096, 4096)];
        assert_eq!(footer.contents.extents, extents);
        assert_eq!((footer.contents.next_sequence, recovered.next), (12, 5));
        assert_eq!(files(&dir, SEGMENTS), [3]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
        let object = read_whole(&dir.open(&segment_path(segment(3)), true).unwrap()).unwrap();
        assert_eq!(extent_frame::data(&object).unwrap(), vec![3; 2 * 4096]);
        // Recovered again, it holds the same.
        let again = recover(&dir, BRANCH).unwrap();
        assert_eq!(again.segments, recovered.segments);
    }

    /// A segment whose framed file is torn, its framing stopped, closes again from its log; one
    /// whose framed file is whole is kept, its raw data and log going.
    #[test]
    fn segments_framed_part_way_close_again() {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        let footer = recover(&dir, BRANCH).unwrap().segments.remove(0).0;
        // Framed whole, its raw data and log still there.
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        left_open(&dir, 1, 1, &[commit(2, 2, 0)]);
        let torn = dir.create(&segment_path(segment(1)), true).unwrap();
        torn.write_at(&[9; 100], 0).unwrap();
        let recovered = recover(&dir, BRANCH).unwrap();
        let numbers: Vec<u64> = recovered.segments.iter().map(|(_, number)| *number).collect();
        assert_eq!(numbers, [0, 1]);
        assert_eq!(recovered.segments[0].0, footer);
        assert_eq!(recovered.segments[1].0.after, Some((footer.seq, footer.epoch)));
        assert_eq!(recovered.segments[1].0.contents.commits, [tag(2, 2)]);
    }

    /// A segment the base acknowledged, its framed file gone, as acknowledging it removed it, but
    /// its raw data and log a crash brought back, holds nothing of the log's: the whole segment
    /// after it is kept, not taken to follow one closed again, and dropped with the durable
    /// commits it holds (found by the kept crash rounds, TASK-165.9).
    #[test]
    fn segments_acknowledged_brought_back_are_no_logs() {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        recover(&dir, BRANCH).unwrap();
        left_open(&dir, 1, 1, &[commit(2, 2, 0)]);
        let framed = recover(&dir, BRANCH).unwrap().segments;
        assert_eq!(framed.iter().map(|(_, number)| *number).collect::<Vec<_>>(), [0, 1]);
        // Segment 0 acknowledged: the base names its commit, and its framed file went; a crash
        // brought back its raw data and log, whose removals were not durable.
        BaseFile::read(&dir).unwrap().0.write(&dir, Some(tag(2, 1))).unwrap();
        dir.remove(&segment_path(segment(0))).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap();
        assert_eq!(recovered.segments, framed[1..]);
        assert_eq!(files(&dir, SEGMENTS), [1]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// The queue gives the uploader the segments that are framed, whose commits are durable, in
    /// order, each once, and is acknowledged in order, its base moving on.
    #[test]
    fn queues_give_what_may_upload_in_order() {
        let footer = |seq, epoch| Footer {
            branch: BRANCH,
            seq,
            epoch: 2,
            after: None,
            contents: Contents { commits: vec![tag(2, epoch)], ..Contents::default() },
        };
        let mut queue = Queue::new(Some(tag(1, 4)), []);
        for (number, epoch) in [(1, 5), (2, 6), (3, 7)] {
            queue.push(number, footer(number, epoch), 100);
        }
        assert_eq!(queue.bytes(), 300);
        let numbers = |taken: Vec<&Queued>| taken.iter().map(|q| q.number).collect::<Vec<_>>();
        assert_eq!(numbers(queue.take(7, |_| false, 1)), [1]);
        assert_eq!(numbers(queue.take(7, |number| number == 2, 9)), Vec::<u64>::new());
        assert_eq!(numbers(queue.take(6, |_| false, 9)), [2]);
        assert_eq!(numbers(queue.take(7, |_| false, 9)), [3]);
        assert_eq!(numbers(queue.take(7, |_| false, 9)), Vec::<u64>::new());
        assert!(queue.acknowledge(2).is_none());
        assert_eq!(queue.acknowledge(1).map(|queued| queued.footer.seq), Some(1));
        assert_eq!((queue.base(), queue.bytes()), (Some(tag(2, 5)), 200));
    }
}
