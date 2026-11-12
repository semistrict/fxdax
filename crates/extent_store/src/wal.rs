// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a store's write-ahead log of its branch (doc-14, TASK-165.9): what the bucket doesn't
//! have yet of the branch, on local disk, and what recovering it from there takes.
//!
//! - **Its segments**, closed and not yet acknowledged in the bucket: each framed with its footer,
//!   `segments/<branch>/<n>`, the object it is to be in the bucket, and the extents its footer
//!   names stored on their own, `extents/<branch>/<sequence>`. They are framed in the order they
//!   closed, one after another, and made durable together, those framed meanwhile
//!   (`segments::make_durable`, TASK-310): a crash may leave any of those being made durable
//!   whole and any torn, a later one whole and an earlier one not; their raw files and logs stay
//!   until they are durable, and recovery checks each whose raw file or log is there (below).
//! - **Each segment's log** of the commits made in it, `logs/<branch>/<n>`, beside its raw data,
//!   `open-segments/<branch>/<n>`: a record of each commit's contents, as its footer will fold
//!   them (`extent_frame::segment::Contents`), each hashed, appended and synced by the flush that
//!   makes the commit durable, whether the segment is open, or closed since and being framed: a
//!   flush waits for no framing. fxdax: a flush that seals little puts the runs its commits
//!   sealed in their records, and syncs its logs alone, one sync, as a database's write-ahead log
//!   holds its pages; recovery writes them into the raw data again. Others put the CRC32C of each
//!   run in the records, and sync the data and append the records at once: recovery stops at the
//!   first record whose runs the raw data doesn't hold, as at a torn record, a commit whose record
//!   was made durable before its data was not durable ([`LogRecord`], TASK-301). A flush of
//!   commits that stored extents on their own, which no record checks, syncs their data first,
//!   as before. Its raw data and log go once it is
//!   framed and the commits it names are durable, its framed file their record then.
//! - **Its base**, `wal`: the branch's commit its first segment goes on from, which the store
//!   writes, durably, before the segments it acknowledged leave it ([`BaseFile`]): a segment a
//!   crash brings back, its removal not durable, whose last commit is the base's or before, is
//!   acknowledged, and goes again.
//!
//! Recovering it ([`recover`]) keeps each segment whose framed file is whole, and closes again
//! each whose framed file is not, left open, or its framing or its entry's sync stopped by a
//! crash, from its raw data and the records of its log that are whole, which are the commits made
//! durable, at least; fxdax: and, after the first segment not whole, each whose raw data and log
//! are still there, whole or not, as segments are made durable several at once (TASK-310), what
//! it keeps ending at a log refusing a record's runs, at a segment holding no durable commit, or at
//! a log not following the last commit kept (TASK-334); framed files left being made durable,
//! `framing-` ones, it removes: a footer
//! of their contents folded, as the segment's closing would have; one whose log holds none held no
//! durable commit, and goes. A log may lack its segment's last
//! commits, those not durable, as flushes append records in their commits' order: the next segment
//! holding commits then follows commits it lacks, which no later segment holds durable, and those
//! after it go, durably, before it is closed again. The store's commits are numbered one after
//! another, and the segments a crash left torn are all of the last opening's, which numbered them
//! alike: one closed again is whole if the next segment holding commits holds the next commit first
//! (TASK-165.9's crash rounds, cutting openings, and its Shuttle tests, a crash bringing back a
//! segment's raw data without its log). The footers recovered are
//! stamped as they were, or provisionally for those closed again: opening the store puts the log
//! after the branch's end by its commits (`extent_remote::Wal::rebase`), and stamps them anew.

use crate::dir::{Dir, File, not_found, read_file};
use crate::framed::{read_frame, read_whole};
use crate::layout::{
    FRAMING, LOGS, OPEN_SEGMENTS, SEGMENTS, SegmentId, branch_dir, decode, encode, extent_path,
    list_numbers, log_path, open_segment_path, segment_path,
};
use crate::segments::{fold, footer_last, segment_footer};
use anyhow::{Context as _, Error};
use extent_frame::segment::{Branch, Chain, Contents, Footer, epoch_of, writer_of};
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
        crate::written::wrote(crate::written::Kind::Log, record.len());
        file.sync().context("syncing the WAL's base")?;
        self.serial = serial;
        Ok(())
    }
}

/// fxdax: a commit's record in its segment's log: its contents, and what makes the runs its
/// extents were sealed in durable with it (TASK-301): the runs themselves, for a flush that seals
/// little, which then syncs its log alone, as a database's write-ahead log holds its pages; or the
/// CRC32C of each, which recovery checks against the segment's raw data, for one whose data it
/// syncs beside its records. Records of logs written before either are their contents alone,
/// which nothing checks: their flushes synced the data first.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LogRecord {
    pub contents: Contents,
    pub runs: Runs,
}

/// What a [`LogRecord`] holds of the runs its commit sealed, in the order its contents name them
/// (`Contents::runs`: fxdax: those of the commit it goes on with too, TASK-92.41).
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Runs {
    /// Nothing: its data was synced before it.
    Synced,
    /// The CRC32C of each.
    Checked(Vec<u32>),
    /// Their bytes, one after another.
    Held(Bytes),
}

/// fxdax: bytes, which bincode writes as a length and the bytes, not a sequence of each byte's
/// value, as it would a `Vec<u8>`: a record holding a commit's runs encodes as fast as it copies.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Bytes(pub Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Bytes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Bytes, E> {
                Ok(Bytes(bytes.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Bytes, E> {
                Ok(Bytes(bytes))
            }
        }
        deserializer.deserialize_byte_buf(Visitor)
    }
}

impl LogRecord {
    /// The record of `contents`, made in a segment whose raw data `raw` is: the runs it sealed
    /// read back, and held, if `hold`, or checksummed.
    pub fn of<F: File>(contents: Contents, raw: &F, hold: bool) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        let mut crcs = Vec::with_capacity(contents.extents.len());
        for &(extent, offset, len) in contents.runs() {
            let start = bytes.len();
            bytes.resize(start + len as usize, 0);
            raw.read_cached(&mut bytes[start..], offset)
                .with_context(|| format!("reading {extent:?}'s run back"))?;
            if !hold {
                crcs.push(extent_frame::crc(&bytes[start..]));
                bytes.truncate(start);
            }
        }
        let runs = if hold { Runs::Held(Bytes(bytes)) } else { Runs::Checked(crcs) };
        Ok(Self { contents, runs })
    }

    /// The bytes of the runs `contents` names.
    pub fn run_bytes(contents: &Contents) -> u64 {
        contents.runs().map(|&(_, _, len)| len).sum()
    }

    /// Makes `raw`, its segment's raw data, hold the runs it names, as its commit wrote them:
    /// those it holds, written into it; whether it does, those it checks, or those synced.
    fn restore(&self, raw: &mut Vec<u8>) -> bool {
        let runs =
            self.contents.runs().map(|&(_, offset, len)| offset as usize..(offset + len) as usize);
        match &self.runs {
            Runs::Synced => true,
            Runs::Checked(crcs) => {
                crcs.len() == self.contents.runs().count()
                    && runs.zip(crcs).all(|(run, &crc)| {
                        raw.get(run).is_some_and(|run| extent_frame::crc(run) == crc)
                    })
            }
            Runs::Held(Bytes(bytes)) => {
                let mut at = 0;
                for run in runs {
                    if raw.len() < run.end {
                        raw.resize(run.end, 0);
                    }
                    let Some(held) = bytes.get(at..at + run.len()) else { return false };
                    raw[run.clone()].copy_from_slice(held);
                    at += run.len();
                }
                at == bytes.len()
            }
        }
    }

    /// The records of `bytes`, `segment`'s log, that are whole: those of any kind, after its
    /// header, salted as it says ([`LOG_MAGIC`]), as a log reused does, whose records from before
    /// are torn then; none if its header isn't `segment`'s ([`log_salt`]).
    fn decode(bytes: &[u8], segment: SegmentId) -> Vec<Self> {
        let Some(salt) = log_salt(bytes, segment) else { return Vec::new() };
        let bytes = bytes.get(LOG_RECORDS as usize..).unwrap_or_default();
        crate::layout::decode_with(bytes, salt, |payload| {
            bincode::deserialize::<Self>(payload).ok().or_else(|| {
                let contents = bincode::deserialize::<Contents>(payload).ok()?;
                Some(Self { contents, runs: Runs::Synced })
            })
        })
        .0
    }
}

/// fxdax: what a segment's log starts with: its header, a block, its salt after this, then its
/// records, each salted with it, so that a log reused, a spare, holding records of another
/// segment's after them, reads as holding its own alone (TASK-301): a log made ahead of its
/// records is overwritten as it is appended to, a sync then syncing the records alone, not the
/// log's size and blocks too, a third of the time on a cloud disk. The header is synced before any
/// record is appended: a log whose new header a crash lost would read its old one's records.
/// fxdax: after the salt, the segment the log is of, the branch's then the number: a spare is
/// renamed a log, and was renamed a spare, neither rename durable until a flush syncs the
/// directory of logs, so a crash may keep it under its last segment's name, holding the new
/// one's header and records, which an opening took for the last one's commits (TASK-341, found by
/// TASK-310's crash rounds, round 81); a log whose header names another segment holds none of
/// its own.
pub(crate) const LOG_MAGIC: &[u8; 8] = b"FXLOG\0\0\x02";

/// fxdax: where a segment's log's records start, after its header.
pub(crate) const LOG_RECORDS: u64 = 4096;

/// fxdax: the bytes of a log's header ([`LOG_MAGIC`]) that say anything.
pub(crate) const LOG_HEADER: usize = LOG_MAGIC.len() + 3 * 8;

/// fxdax: the header of `segment`'s log, its records salted with `salt` ([`LOG_MAGIC`]).
pub(crate) fn log_header(salt: u64, segment: SegmentId) -> Vec<u8> {
    let mut header = vec![0; LOG_RECORDS as usize];
    let fields = [salt, segment.branch, segment.number].map(u64::to_le_bytes);
    header[..LOG_MAGIC.len()].copy_from_slice(LOG_MAGIC);
    header[LOG_MAGIC.len()..LOG_HEADER].copy_from_slice(fields.as_flattened());
    header
}

/// fxdax: the salt of the records of `log`, `segment`'s log's bytes, as its header says, if it
/// has one, and it is `segment`'s.
pub(crate) fn log_salt(log: &[u8], segment: SegmentId) -> Option<u64> {
    let fields = log.strip_prefix(LOG_MAGIC)?.get(..LOG_HEADER - LOG_MAGIC.len())?;
    let [salt, branch, number] =
        [0, 1, 2].map(|at| u64::from_le_bytes(fields[at * 8..][..8].try_into().unwrap()));
    (SegmentId { branch, number } == segment).then_some(salt)
}

/// fxdax: a salt for the records of a log of `segment`'s made in `lifetime`, a value of the
/// opening of the branch that made it ([`crate::segments::Segments::open`]): none another log
/// had, as a branch whose segments all left numbers its next from 0 again, its epoch past theirs;
/// and none 0, the salt of logs before they had headers.
pub(crate) fn new_salt(lifetime: u64, segment: SegmentId) -> u64 {
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(&lifetime.to_le_bytes());
    bytes[8..16].copy_from_slice(&segment.branch.to_le_bytes());
    bytes[16..].copy_from_slice(&segment.number.to_le_bytes());
    rapidhash::rapidhash(&bytes) | 1
}

/// The log of the commits made in a segment ([`log_path`]), open, or closed since: where its next
/// record goes, which the flushes append, one at a time, in their commits' order.
pub(crate) struct SegmentLog<F> {
    file: Arc<F>,
    end: u64,
    salt: u64,
}

impl<F: File> SegmentLog<F> {
    /// A segment's log, `file`, holding no records yet, after its header ([`log_header`]), made or
    /// reused with it, its records salted with `salt`.
    pub fn new(file: Arc<F>, salt: u64) -> Self {
        Self { file, end: LOG_RECORDS, salt }
    }

    /// The salt of its records.
    pub fn salt(&self) -> u64 {
        self.salt
    }

    /// Appends `records`, encoded, after those before, and syncs them. fxdax: keeping the log's
    /// pages in the host's page cache, as the open segment's raw file keeps its (TASK-301): a log is
    /// appended to and synced again and again, and its next record's append would read its last
    /// page back from the disk first.
    pub fn append<'a>(&mut self, records: impl IntoIterator<Item = &'a [u8]>) -> Result<(), Error> {
        let start = self.end;
        for record in records {
            self.file.write_at(record, self.end)?;
            crate::written::wrote(crate::written::Kind::Log, record.len());
            self.end += record.len() as u64;
        }
        if self.end > start {
            self.file.sync_cached()?;
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
/// is whole, in order, and each whose framed file is not that its log closes again, holding
/// commits, to the last durable commit; and nothing else of segments: raw data, logs and framed
/// files torn, closed again or holding nothing durable, go.
pub(crate) fn recover<D: Dir>(dir: &D, branch: Branch) -> Result<Recovered, Error> {
    let (base_file, base) = BaseFile::read(dir)?;
    let listed = |kind| -> Result<BTreeSet<u64>, Error> {
        Ok(list_numbers(dir, &branch_dir(kind, branch.id))?.into_iter().collect())
    };
    // fxdax: framed files a crash stopped being made durable, named their own: their segments are
    // closed again from their raw files and logs, which stay until they are (TASK-310).
    for name in dir.list(&branch_dir(SEGMENTS, branch.id))? {
        if name.starts_with(FRAMING) {
            remove(dir, &format!("{}/{name}", branch_dir(SEGMENTS, branch.id)))?;
        }
    }
    let (framed, raw, logs) = (listed(SEGMENTS)?, listed(OPEN_SEGMENTS)?, listed(LOGS)?);
    let numbers: BTreeSet<u64> = framed.iter().chain(&raw).chain(&logs).copied().collect();
    let left = |number| raw.contains(&number) || logs.contains(&number);
    // fxdax: the logs that are their segments' own, their headers naming them: one renamed a spare
    // and taken as another segment's log, which a crash brought back under its old name, holds none
    // of its segment's records, and says nothing of whether its segment was made durable: a
    // segment the base acknowledged, its files' removal undone, its log a spare taken since, read as
    // one past the base whose raw file and log are both there, broke the run of segments framed
    // whole, those after it closed again from their raw files and logs, which they hold no more,
    // their commits lost (TASK-369, the Shuttle rounds, with the next segment's log prepared from a
    // spare as soon as it opens).
    let mut own = BTreeSet::new();
    for &number in &logs {
        if own_log(dir, SegmentId { branch: branch.id, number })? {
            own.insert(number);
        }
    }
    // What a segment holds is in its framed file, durably, or nothing durable is: its raw data and
    // log go.
    let settled = |number| -> Result<(), Error> {
        if left(number) {
            let segment = SegmentId { branch: branch.id, number };
            dir.sync_dir(&branch_dir(SEGMENTS, branch.id))?;
            dir.barrier()?;
            remove(dir, &open_segment_path(segment))?;
            remove(dir, &log_path(segment))?;
        }
        Ok(())
    };
    // A log's commits the base acknowledged are the bucket's: its framed file left, its raw data
    // and log a crash brought back, it holds nothing of the log's. fxdax: a commit's record going
    // on with it past the segment, the commit's (TASK-92.41).
    let after_base = |records: &[Contents]| {
        let last = records.iter().rev().find_map(last_commit);
        last.is_some_and(|last| base.is_none_or(|base| epoch_of(last) > epoch_of(base)))
    };
    // A segment holding nothing goes, its framed file's removal durable before its raw data's and
    // log's, as its framing was before they went: a framed file a crash kept without them, which
    // the next recovery takes as framed whole, unchecked, is whole (TASK-318).
    let gone = |number| -> Result<(), Error> {
        let segment = SegmentId { branch: branch.id, number };
        remove(dir, &segment_path(segment))?;
        if framed.contains(&number) && left(number) {
            dir.sync_dir(&branch_dir(SEGMENTS, branch.id))?;
            dir.barrier()?;
        }
        remove(dir, &open_segment_path(segment))?;
        remove(dir, &log_path(segment))
    };
    // What each segment holds, in order. fxdax: its framed file, whole, only while every segment
    // before it is framed whole, or holds only commits the base acknowledged; after the first that
    // isn't, each whose raw file and log are both still there is closed again from them, as a
    // torn one is (TASK-310). Segments are made durable several at once, a later one's framed file
    // whole while an earlier one's is not, its commits not durable then, which its log lacks: the
    // later footer, its record of commits after those, or of the rest of a commit spanning both,
    // read whole, would hold the branch past what the earlier one lost (TASK-310's crash rounds).
    // Their logs hold every commit a flush made durable, which is all a crash must keep. One whose
    // log is gone, or its raw file, was durable, its commits too, as they go only then: its
    // making durable made their entries durable before its framed file's
    // (`Segments::unsynced_entries`).
    let mut found = Vec::new();
    let mut broken = false;
    // fxdax: the segments whose logs hold a record whose runs their raw data doesn't hold: its
    // commit's data not durable, the commit isn't, nor those after it (`log_records`); and those
    // which end what is durable so, holding nothing durable, their framed file not whole, nor any
    // record: one whose raw file or log is there, past the segments the base acknowledged, made
    // as the next commits' flushes would have synced its entries, which no durable commit's did
    // (TASK-334). A segment the base acknowledged, its files' removal undone, comes before those
    // past it.
    let mut refusing = BTreeSet::new();
    let mut ending = BTreeSet::new();
    let mut past_base = base.is_none();
    for number in numbers {
        let segment = SegmentId { branch: branch.id, number };
        let again = broken && raw.contains(&number) && own.contains(&number);
        let footer = match framed.contains(&number) && !again {
            true => whole_footer(dir, segment, left(number))?,
            false => None,
        };
        let (records, data) = match (&footer, raw.contains(&number)) {
            (None, true) => {
                let (records, data, refused) = log_records(dir, segment)?;
                if refused {
                    refusing.insert(number);
                }
                (records, data)
            }
            _ => (Vec::new(), Vec::new()),
        };
        // Not whole, and not one the base acknowledged, whose removal a crash undid: one whose
        // raw file or log is gone was durable, as they go only then.
        let lost = raw.contains(&number) && own.contains(&number);
        broken |= footer.is_none() && lost && (records.is_empty() || after_base(&records));
        let holds_nothing = past_base && records.is_empty();
        if footer.is_none() && left(number) && (refusing.contains(&number) || holds_nothing) {
            ending.insert(number);
        }
        past_base |= after_base(&records)
            || footer
                .as_ref()
                .is_some_and(|footer| base.is_none_or(|base| footer_last(footer) > epoch_of(base)));
        found.push(match footer {
            Some(footer) => (number, Found::Whole(footer)),
            None if after_base(&records) => (number, Found::Torn(records, data)),
            None => (number, Found::Nothing),
        });
    }
    // fxdax: whether the base acknowledged a segment's commits, its whole.
    let acknowledged =
        |footer: &Footer| base.is_some_and(|base| footer_last(footer) <= epoch_of(base));
    // fxdax: whether the commit the segment found at `at` goes on with past it, `commit`, ends:
    // the segments after it, one after another, each going on with it, to one ending it
    // (TASK-92.41). A flush appends a commit's records to its segments' logs in their order, the
    // one ending it last, but checks of their runs against the raw data may stop at any of them.
    let ended = |at: usize, commit: u64| -> bool {
        let mut number = found[at].0;
        for (next, found) in &found[at + 1..] {
            if *next != number + 1 {
                return false;
            }
            number = *next;
            match found.goes_on(commit) {
                Some(true) => return true,
                Some(false) => {}
                None => return false,
            }
        }
        false
    };
    // Each segment, in order: whole, framed; or, its framed file not whole, closed again from its
    // log, which holds the records of the commits made in it that a flush made durable, at least;
    // or holding no durable commit, its files going. One closed again whose log lacks its last
    // commits, the next segment holding commits not following its last, is the last: those after
    // it go, durably, first (see the module's docs). fxdax: and one going on with a commit that no
    // segment after it ends is the last, closed again without it, from its raw data and log, which
    // stay until the commit is durable: the commit is not.
    let mut segments: Vec<(Footer, u64)> = Vec::new();
    for (at, (number, found_here)) in found.iter().enumerate() {
        let number = *number;
        let segment = SegmentId { branch: branch.id, number };
        let continues = |record: &Contents| record.continued.is_some();
        // fxdax: those from `from` on go, durably, before any is closed again: those after it
        // first, then it, whose files say why they go, a log refusing a record or a commit past
        // the last kept, which a crash between its files' removals and theirs would leave gone,
        // and theirs kept (TASK-334's opening rounds).
        let cut = |from: usize| -> Result<(), Error> {
            let synced = || -> Result<(), Error> {
                for kind in [SEGMENTS, OPEN_SEGMENTS, LOGS] {
                    dir.sync_dir(&branch_dir(kind, branch.id))?;
                }
                dir.barrier().map_err(Error::from)
            };
            for (number, _) in found[from + 1..].iter().rev() {
                gone(*number)?;
            }
            synced()?;
            gone(found[from].0)?;
            synced()
        };
        let (mut records, data, unended) = match found_here {
            // One the base acknowledged, whose files' removal a crash undid, goes below: its
            // commit is the bucket's, whole, whatever is left of the segments after it.
            Found::Whole(footer) => match &footer.contents.continued {
                Some(continued) if !acknowledged(footer) && !ended(at, continued.commit) => {
                    let (records, data, _) = log_records(dir, segment)?;
                    (records, data, true)
                }
                _ => {
                    settled(number)?;
                    segments.push((footer.clone(), number));
                    continue;
                }
            },
            // fxdax: one whose log refused a record, or holds none, its raw file and log there,
            // holds the first commit not durable, if any: it and those after go (TASK-334): a
            // commit spanning it and the next, its record in the next's log whole, its runs there
            // durable, its record here lost or refused, read whole, would be read without them.
            Found::Nothing if ending.contains(&number) => {
                cut(at)?;
                break;
            }
            Found::Nothing => {
                gone(number)?;
                continue;
            }
            Found::Torn(records, data) => {
                // fxdax: its log goes on from the last commit kept, the base's or a segment's, or
                // it and those after go, durably: a flush appends its commits' records to the logs
                // of their segments at once, and a crash may keep a later commit's record, in this
                // one's log, and lose an earlier one's, in the log of a segment before, which went,
                // holding nothing durable (TASK-334, found by TASK-310's Shuttle crash rounds).
                let kept = segments.last().map(|(footer, _)| footer_last(footer));
                let kept = kept.max(base.map(epoch_of));
                if let (Some(first), Some(kept)) = (first_epoch(records), kept)
                    && first > kept + 1
                {
                    cut(at)?;
                    break;
                }
                let continued = records.last().and_then(|record| record.continued.as_ref());
                let unended = continued.is_some_and(|continued| !ended(at, continued.commit));
                (records.clone(), data.clone(), unended)
            }
        };
        let goes_on = !unended && records.last().is_some_and(continues);
        if unended && records.last().is_some_and(continues) {
            records.pop();
        }
        let last = records.iter().rev().find_map(|record| record.commits.last()).copied();
        let next = found[at + 1..].iter().find_map(|(_, found)| found.first_commit());
        let follows = next.is_some_and(|next| last.is_some_and(|last| next == epoch_of(last) + 1));
        // fxdax: and not if its log refused a record, its commit not durable (TASK-334).
        let whole = !refusing.contains(&number) && (goes_on || (!unended && follows));
        if !whole && at + 1 < found.len() {
            cut(at + 1)?;
        }
        if !after_base(&records) {
            gone(number)?;
            break;
        }
        if framed.contains(&number) {
            remove(dir, &segment_path(segment))?;
        }
        let previous = segments.last().map(|(footer, _)| footer);
        let footer = close_again(dir, branch, segment, previous, &records, &data)?;
        settled(number)?;
        segments.push((footer, number));
        if !whole {
            break;
        }
    }
    // Those acknowledged by the base, whose files' removal a crash undid, are the bucket's: they
    // go again.
    let (gone, segments): (Vec<_>, Vec<_>) =
        segments.into_iter().partition(|(footer, _)| acknowledged(footer));
    for (footer, number) in gone {
        remove(dir, &segment_path(SegmentId { branch: branch.id, number }))?;
        for (extent, _) in footer.own() {
            remove(dir, &extent_path(extent))?;
        }
    }
    let next = (framed.iter().chain(&raw).chain(&logs)).max().map_or(0, |last| last + 1);
    Ok(Recovered { base, base_file, segments, next })
}

/// The footer of `segment`'s framed file, if it is whole: checked whole, data and all, if `left`,
/// its raw data and log still there, as its framing may have stopped part way; else whole, its
/// framing having made it durable before they went, or not whole, a crash cutting short its
/// removal, as holding nothing durable, after its raw data and log went (TASK-165.9). Synced, if
/// checked.
fn whole_footer<D: Dir>(dir: &D, segment: SegmentId, left: bool) -> Result<Option<Footer>, Error> {
    let path = segment_path(segment);
    let file = dir.open(&path, true)?;
    let footer = match read_frame(&file)? {
        Some((_, footer)) => segment_footer(&footer).ok().flatten(),
        None => None,
    };
    if !left {
        return Ok(footer);
    }
    let whole = footer.is_some() && extent_frame::data(&read_whole(&file)?).is_ok();
    if whole {
        file.sync()?;
    }
    Ok(footer.filter(|_| whole))
}

/// fxdax: whether `segment`'s log is its own: there, its header naming it (`LOG_MAGIC`).
fn own_log<D: Dir>(dir: &D, segment: SegmentId) -> Result<bool, Error> {
    let log = match dir.open(&log_path(segment), false) {
        Ok(log) => log,
        Err(err) if not_found(&err) => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if log.size()? < LOG_HEADER as u64 {
        return Ok(false);
    }
    let mut header = [0; LOG_HEADER];
    log.read_cached(&mut header, 0)?;
    Ok(log_salt(&header, segment).is_some())
}

/// fxdax: the store's epoch of the first commit `records`, a segment's log's, hold: the first a
/// record ends, or goes on with (TASK-92.41).
fn first_epoch(records: &[Contents]) -> Option<u64> {
    let first = records.first()?;
    let commit =
        first.commits.first().or(first.continued.as_ref().map(|continued| &continued.commit));
    commit.copied().map(epoch_of)
}

/// What recovery finds of a segment: its framed file whole; or not, and the records of its log,
/// holding commits after the WAL's base, and its raw data, to close it again from; or nothing
/// durable.
enum Found {
    Whole(Footer),
    Torn(Vec<Contents>, Vec<u8>),
    Nothing,
}

impl Found {
    /// fxdax: whether it ends `commit`, which the segment before it went on with, or goes on with
    /// it, ending none: `Some(true)` or `Some(false)`; none if neither (TASK-92.41). Its first
    /// record, of a log, ends it, or goes on with it.
    fn goes_on(&self, commit: u64) -> Option<bool> {
        let first = match self {
            Found::Whole(footer) => &footer.contents,
            Found::Torn(records, _) => records.first()?,
            Found::Nothing => return None,
        };
        match (first.ends(commit), first.continues(commit)) {
            (true, _) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        }
    }

    /// The store's epoch of the first commit it holds, if any.
    fn first_commit(&self) -> Option<u64> {
        let first = match self {
            Found::Whole(footer) => footer.contents.commits.first(),
            Found::Torn(records, _) => records.iter().find_map(|record| record.commits.first()),
            Found::Nothing => None,
        };
        first.copied().map(epoch_of)
    }
}

/// The whole records of the log of `segment`, the commits it holds, up to the first whose runs
/// its raw data doesn't hold ([`LogRecord`]), and its raw data, with the runs the records hold
/// written into it; none if it has no log. fxdax: and whether there is such a record, whose
/// commit is then not durable, nor those after it: a flush makes a commit durable only once its
/// record and its runs' data both are (TASK-334).
fn log_records<D: Dir>(
    dir: &D,
    segment: SegmentId,
) -> Result<(Vec<Contents>, Vec<u8>, bool), Error> {
    let records = match dir.open(&log_path(segment), false) {
        Ok(log) => LogRecord::decode(&read_file(&log)?, segment),
        Err(err) if not_found(&err) => return Ok((Vec::new(), Vec::new(), false)),
        Err(err) => return Err(err.into()),
    };
    let raw = dir.open(&open_segment_path(segment), true)?;
    let mut data = vec![0; raw.size()? as usize];
    raw.read_cached(&mut data, 0)?;
    let whole = records.iter().take_while(|record| record.restore(&mut data)).count();
    let refused = whole < records.len();
    let records = records.into_iter().take(whole).map(|record| record.contents).collect();
    Ok((records, data, refused))
}

/// Closes `segment` again, left open, or framed part way, from its raw data, `data`, and the
/// records of its log that are whole, which hold a commit at least: framed with a footer of their
/// contents folded, stamped provisionally as `branch`'s, after `last`, the footer before it, if
/// any, and made durable. Returns its footer.
fn close_again<D: Dir>(
    dir: &D,
    branch: Branch,
    segment: SegmentId,
    last: Option<&Footer>,
    records: &[Contents],
    data: &[u8],
) -> Result<Footer, Error> {
    let first = records.first().and_then(last_commit);
    let first = first.context("a log closed again holds a commit")?;
    let mut chain = match last {
        Some(last) => {
            let after = Some((last.seq, last.epoch));
            Chain::new(branch, last.epoch, last.seq + 1, after, last.contents.next_sequence)
        }
        None => Chain::new(branch, writer_of(first), 0, None, 0),
    };
    let footer = chain.close(fold(segment.branch, records));
    let path = segment_path(segment);
    remove(dir, &path)?;
    let file = dir.create(&path, true)?;
    // fxdax: to the end of its last run: past it, nothing the records name, as a framing in place
    // cut short leaves the raw file with its tail, torn (TASK-310).
    let end = footer.contents.runs().map(|&(_, offset, len)| offset + len).max().unwrap_or(0);
    let data = &data[..(end as usize).min(data.len())];
    let framed = extent_frame::framed(data, &footer.encode());
    file.write_at(&framed, 0)?;
    crate::written::wrote(crate::written::Kind::Framed, framed.len());
    file.sync()?;
    Ok(footer)
}

/// fxdax: the last commit `record`, a log's record, holds of: the one it goes on with past its
/// segment, if any, or its last (TASK-92.41).
fn last_commit(record: &Contents) -> Option<u64> {
    let continued = record.continued.as_ref().map(|continued| continued.commit);
    continued.or(record.commits.last().copied())
}

/// Removes the file at `path` in `dir`, unless it is gone.
fn remove<D: Dir>(dir: &D, path: &str) -> Result<(), Error> {
    match dir.remove(path) {
        Err(err) if !not_found(&err) => Err(Error::new(err).context(format!("removing {path}"))),
        _ => Ok(()),
    }
}

/// fxdax: what putting the queue after a branch's end found ([`Queue::rebase`]): the numbers of
/// its segments landed; the chain after the rest, stamped anew; and the first of the rest, by its
/// number, with its footer as it landed, if it ends the branch's last commit and goes on with the
/// next.
pub(crate) type Rebased = (Vec<u64>, Chain, Option<(u64, Footer)>);

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
    /// fxdax: those acknowledged that go on with a commit the next end, held until the one ending
    /// it is acknowledged too ([`Queue::acknowledge`], TASK-338).
    held: Vec<Queued>,
}

impl Queue {
    /// A queue of `segments`, after `base`.
    pub fn new(base: Option<u64>, segments: impl IntoIterator<Item = Queued>) -> Self {
        Self { segments: segments.into_iter().collect(), base, held: Vec::new() }
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
            let last = footer_last(&queued.footer);
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
    /// and the chain after the last of them, which the open segment's footer goes on from. fxdax:
    /// and the first of the rest, by its number and its footer before, if it landed, ending the
    /// branch's last commit, and goes on with the next ([`extent_remote::Rebased::ended`],
    /// TASK-345). `Diverged` if another writer's commits moved the branch past it.
    pub fn rebase(&mut self, fenced: &Fenced) -> Result<Rebased, extent_remote::Error> {
        let queued = self.segments.iter().map(|queued| (queued.footer.clone(), queued.number));
        let mut wal = extent_remote::Wal::of(self.base, queued);
        let extent_remote::Rebased { landed, ended } = wal.rebase(fenced)?;
        let landed = landed.into_iter().map(|(_, number)| number).collect();
        let first = wal.segments().next().map(|(_, number)| *number);
        let ended = ended.map(|footer| (first.expect("the segment ending the branch"), footer));
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
        Ok((landed, chain, ended))
    }

    /// fxdax: the commits of its first segment, through `base`, landed, though the segment goes
    /// on with the next commit, stamped again with that alone (TASK-345): the queue goes on from
    /// them.
    pub fn landed_ending(&mut self, base: Option<u64>) {
        self.base = base;
    }

    /// The first segment, if any.
    pub fn first(&self) -> Option<&Queued> {
        self.segments.front()
    }

    /// Segment `number`, the first, is acknowledged: it leaves the queue, which goes on from its
    /// last commit. Returns the segments whose acknowledgements are whole now, which their store
    /// may let go of: it, and those held before it; none if it goes on with a commit, which it is
    /// held for, until the segment ending that commit is acknowledged (TASK-338). Let go of before,
    /// its files leaving for the host's cache, the base not yet past the commit, a crash left the
    /// segments before and after it, the commit's runs in it missing, and an opening closed the
    /// commit's first segment again without it, losing it and those after, durable. None if it is
    /// not the first.
    pub fn acknowledge(&mut self, number: u64) -> Option<Vec<Queued>> {
        if self.segments.front().is_none_or(|queued| queued.number != number) {
            return None;
        }
        let queued = self.segments.pop_front().unwrap();
        self.base = queued.footer.contents.commits.last().copied().or(self.base);
        let continues = queued.footer.contents.continued.is_some();
        self.held.push(queued);
        Some(if continues { Vec::new() } else { std::mem::take(&mut self.held) })
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
    use extent_frame::segment::{Continued, tag};
    use storage_device::ExtentId;

    const BRANCH: extent_remote::layout::Branch = extent_remote::layout::test_branch(7);

    fn segment(number: u64) -> SegmentId {
        SegmentId { branch: BRANCH.id, number }
    }

    /// A commit of epoch `epoch`, sealing `extent` at `offset`, of a block.
    fn commit(epoch: u64, extent: u64, offset: u64) -> Contents {
        Contents {
            commits: vec![tag(2, epoch)],
            extents: vec![(ExtentId::new(BRANCH.id, extent), offset, 4096)],
            next_sequence: extent + 1,
            ..Contents::default()
        }
    }

    /// Writes segment `number`'s raw data, `blocks` blocks, and its log, of `records`, as an open
    /// segment's flushes leave them.
    fn left_open(dir: &MemDir, number: u64, blocks: u64, records: &[Contents]) {
        let raw = dir.create(&open_segment_path(segment(number)), true).unwrap();
        raw.write_at(&vec![number as u8; (blocks * 4096) as usize], 0).unwrap();
        logged(dir, number, number, records);
    }

    /// Writes segment `number`'s log as segment `of`'s log, of `records`, its header and records
    /// `of`'s: a spare reused for `of` that a crash left under `number`'s name.
    fn logged(dir: &MemDir, number: u64, of: u64, records: &[Contents]) {
        let log = Arc::new(dir.create(&log_path(segment(number)), false).unwrap());
        let salt = new_salt(0, segment(of));
        log.write_at(&log_header(salt, segment(of)), 0).unwrap();
        let mut log = SegmentLog::new(log, salt);
        let records = records.iter().map(|contents| {
            let record = LogRecord { contents: contents.clone(), runs: Runs::Synced };
            crate::layout::encode_salted(&record, salt)
        });
        let records: Vec<Vec<u8>> = records.collect();
        log.append(records.iter().map(Vec::as_slice)).unwrap();
    }

    fn files(dir: &MemDir, kind: &'static str) -> Vec<u64> {
        list_numbers(dir, &branch_dir(kind, BRANCH.id)).unwrap()
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
        create_branch_dirs(&dir, BRANCH.id).unwrap();
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
            [(ExtentId::new(BRANCH.id, 10), 0, 4096), (ExtentId::new(BRANCH.id, 11), 4096, 4096)];
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
        create_branch_dirs(&dir, BRANCH.id).unwrap();
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
        create_branch_dirs(&dir, BRANCH.id).unwrap();
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

    /// A segment whose framed file a crash tore, its raw data and log there, the log holding no
    /// durable commit, goes, its framed file's removal durable before theirs: a crash keeping the
    /// framed file without them would have the next recovery take it as framed whole, unchecked,
    /// its parts failing their CRCs as they are read (TASK-318).
    #[test]
    fn segments_holding_nothing_go_framed_files_first() {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH.id).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        left_open(&dir, 1, 1, &[commit(2, 2, 0)]);
        let framed = recover(&dir, BRANCH).unwrap().segments;
        assert_eq!(framed.iter().map(|(_, number)| *number).collect::<Vec<_>>(), [0, 1]);
        // Segment 1's framed file torn, its trailer, table and footer whole but not its data; its
        // raw data and log back, the log holding no record durable.
        let torn = dir.open(&segment_path(segment(1)), true).unwrap();
        torn.write_at(&[!1], 0).unwrap();
        torn.sync().unwrap();
        left_open(&dir, 1, 1, &[]);
        for kind in [SEGMENTS, OPEN_SEGMENTS, LOGS] {
            dir.sync_dir(&branch_dir(kind, BRANCH.id)).unwrap();
        }
        assert_eq!(recover(&dir, BRANCH).unwrap().segments, framed[..1]);
        // The opening makes its next open segment, whose raw data and log it makes durable.
        for kind in [OPEN_SEGMENTS, LOGS] {
            dir.sync_dir(&branch_dir(kind, BRANCH.id)).unwrap();
        }
        for seed in 0..64 {
            let recovered = recover(&dir.crash(seed), BRANCH).unwrap();
            assert_eq!(recovered.segments, framed[..1], "seed {seed}");
        }
    }

    /// fxdax: the record of commit `epoch` going on past its segment, sealing `extent` there at
    /// `offset`, of a block: a commit spanning segments (TASK-92.41).
    fn continuing(epoch: u64, extent: u64, offset: u64) -> Contents {
        let extents = vec![(ExtentId::new(BRANCH.id, extent), offset, 4096)];
        let continued = Some(extent_frame::segment::Continued { commit: tag(2, epoch), extents });
        Contents { continued, ..Contents::default() }
    }

    /// fxdax: a commit spanning segments whose records, each segment's part of it, are whole, the
    /// last ending it, is kept, each segment closed again with its part, going on with it, but the
    /// last; one whose last segment's log lacks its record, ending it, is not: the first closes
    /// again with the commits before it alone, and those after go (TASK-92.41).
    #[test]
    fn commits_spanning_segments_are_kept_whole_or_not_at_all() {
        let ended = |dir: &MemDir| {
            create_branch_dirs(dir, BRANCH.id).unwrap();
            left_open(dir, 0, 2, &[commit(1, 1, 0), continuing(2, 2, 4096)]);
            left_open(dir, 1, 1, &[continuing(2, 3, 0)]);
        };
        let dir = MemDir::new();
        ended(&dir);
        left_open(&dir, 2, 1, &[commit(2, 4, 0), commit(3, 5, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap().segments;
        let place = |(footer, number): &(Footer, u64)| {
            let continued = footer.contents.continued.as_ref().map(|continued| continued.commit);
            (*number, footer.contents.commits.clone(), continued)
        };
        let places: Vec<_> = recovered.iter().map(place).collect();
        let expected = [
            (0, vec![tag(2, 1)], Some(tag(2, 2))),
            (1, vec![], Some(tag(2, 2))),
            (2, vec![tag(2, 3)], None),
        ];
        assert_eq!(places, expected);
        assert_eq!(recover(&dir, BRANCH).unwrap().segments, recovered, "recovered again");

        let dir = MemDir::new();
        ended(&dir);
        left_open(&dir, 2, 1, &[]);
        let recovered = recover(&dir, BRANCH).unwrap().segments;
        assert_eq!(recovered.iter().map(place).collect::<Vec<_>>(), [(0, vec![tag(2, 1)], None)]);
        assert_eq!(files(&dir, SEGMENTS), [0]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
        // Framed whole, going on with the commit, its raw data and log there: the same.
        let dir = MemDir::new();
        ended(&dir);
        left_open(&dir, 2, 1, &[commit(2, 4, 0)]);
        let framed = recover(&dir, BRANCH).unwrap().segments;
        assert_eq!(framed.len(), 3);
        left_open(&dir, 0, 2, &[commit(1, 1, 0), continuing(2, 2, 4096)]);
        left_open(&dir, 1, 1, &[continuing(2, 3, 0)]);
        dir.remove(&segment_path(segment(2))).unwrap();
        left_open(&dir, 2, 1, &[]);
        let recovered = recover(&dir, BRANCH).unwrap().segments;
        assert_eq!(recovered.iter().map(place).collect::<Vec<_>>(), [(0, vec![tag(2, 1)], None)]);
    }

    /// fxdax: a segment going on with a commit the base acknowledged, its raw data gone, its framed
    /// file's removal, as it left for the host's cache, and its log's undone by a crash, the segment
    /// ending the commit gone: it goes, its commit the bucket's, whole, not taken for one going on
    /// with a commit no segment ends, whose raw data it would close it again from (found by the
    /// cut openings of the kept crash rounds, TASK-92.41).
    #[test]
    fn segments_going_on_with_acknowledged_commits_go() {
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH.id).unwrap();
        left_open(&dir, 0, 2, &[commit(1, 1, 0), continuing(2, 2, 4096)]);
        left_open(&dir, 1, 1, &[commit(2, 3, 0)]);
        assert_eq!(recover(&dir, BRANCH).unwrap().segments.len(), 2);
        BaseFile::read(&dir).unwrap().0.write(&dir, Some(tag(2, 2))).unwrap();
        dir.remove(&segment_path(segment(1))).unwrap();
        // Its log's removal undone too, not its raw data's.
        left_open(&dir, 0, 2, &[commit(1, 1, 0), continuing(2, 2, 4096)]);
        dir.remove(&open_segment_path(segment(0))).unwrap();
        let recovered = recover(&dir, BRANCH).unwrap();
        assert_eq!(recovered.segments, []);
        assert_eq!(files(&dir, SEGMENTS), Vec::<u64>::new());
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// Segments 0 to 2 framed whole, from logs of commits 1 and 2, 3, and 4: a flush appends its
    /// commits' records to the logs of the segments they were made in, being framed or open, so
    /// that each segment left torn closes again (TASK-165.9).
    fn framed_three(dir: &MemDir) -> Vec<(Footer, u64)> {
        create_branch_dirs(dir, BRANCH.id).unwrap();
        left_open(dir, 0, 1, &[commit(1, 1, 0), commit(2, 2, 0)]);
        left_open(dir, 1, 1, &[commit(3, 3, 0)]);
        left_open(dir, 2, 1, &[commit(4, 4, 0)]);
        let recovered = recover(dir, BRANCH).unwrap().segments;
        let last: Vec<u64> = recovered.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [2, 3, 4]);
        recovered
    }

    /// A segment whose framed file isn't whole, its log holding each of its commits, closes again,
    /// and the segments after it, holding the commits after its last, are kept: though a crash
    /// brought their raw data back without their logs, their removals not durable, which doesn't
    /// say whether their commits were durable (found by Shuttle, TASK-165.9).
    #[test]
    fn segments_after_one_closed_again_are_kept_if_they_follow_it() {
        let dir = MemDir::new();
        let framed = framed_three(&dir);
        dir.remove(&segment_path(segment(0))).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0), commit(2, 2, 0)]);
        for number in [1, 2] {
            let raw = dir.create(&open_segment_path(segment(number)), true).unwrap();
            raw.write_at(&[number as u8; 4096], 0).unwrap();
        }
        let recovered = recover(&dir, BRANCH).unwrap();
        let numbers: Vec<u64> = recovered.segments.iter().map(|(_, number)| *number).collect();
        assert_eq!(numbers, [0, 1, 2]);
        assert_eq!(recovered.segments[1..], framed[1..]);
        assert_eq!(footer_last(&recovered.segments[0].0), 2);
        assert_eq!(files(&dir, SEGMENTS), [0, 1, 2]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: after a segment that holds nothing durable, its framed file lost, a segment framed
    /// whole whose raw data and log are still there is closed again from them, not read from its
    /// framed file: segments are made durable several at once, and a crash may keep a later one's
    /// framed file and lose an earlier one's, its commits not durable, its log lacking them, the
    /// later footer then a record of commits after those it lost (TASK-310's Shuttle crash
    /// rounds, round 137). Its log lacking its commits too, it goes.
    #[test]
    fn segments_framed_after_one_lost_close_again_from_their_logs() {
        let dir = MemDir::new();
        framed_three(&dir);
        dir.remove(&segment_path(segment(1))).unwrap();
        left_open(&dir, 1, 1, &[]);
        left_open(&dir, 2, 1, &[]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let numbers: Vec<u64> = recovered.segments.iter().map(|(_, number)| *number).collect();
        assert_eq!(numbers, [0]);
        assert_eq!(footer_last(&recovered.segments[0].0), 2);
        assert_eq!(files(&dir, SEGMENTS), [0]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: a segment left open whose log's first commit isn't the next after the last kept goes,
    /// and those after it: a flush appends its commits' records to their segments' logs at once,
    /// and a crash may keep a later commit's record and lose an earlier one's in the log before,
    /// whose segment then holds nothing durable, and goes (TASK-310's Shuttle crash rounds, round
    /// 20).
    #[test]
    fn logs_following_a_commit_lost_go() {
        let dir = MemDir::new();
        framed_three(&dir);
        left_open(&dir, 3, 1, &[]);
        left_open(&dir, 4, 1, &[commit(6, 6, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let last: Vec<u64> =
            recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [2, 3, 4]);
        assert_eq!(files(&dir, SEGMENTS), [0, 1, 2]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: a commit spanning two segments left open, its record in the second's log whole and
    /// its runs there durable, but its record in the first's checking runs the first's raw data
    /// doesn't hold, isn't durable: both go, the commit with them, though the second's log alone
    /// reads it whole (TASK-334, found by TASK-310's bucket crash rounds: a flush syncs a commit's
    /// runs' data at once with its records, and a crash before both are durable kept the records
    /// and the second's data, not the first's).
    /// The segments [`framed_three`] leaves, then segment 3, filled since commit 4, going on with
    /// commit 5 in its run of extent 5, its record checking a run its raw data, a crash's, doesn't
    /// hold; and segment 4, the open one, ending it in its run of extent 6.
    fn refusing_two(dir: &MemDir) {
        framed_three(dir);
        left_open(dir, 3, 1, &[]);
        let continued = Contents {
            continued: Some(Continued {
                commit: tag(2, 5),
                extents: vec![(ExtentId::new(BRANCH.id, 5), 0, 4096)],
            }),
            ..Contents::default()
        };
        let crc = extent_frame::crc(&[0xee; 4096]);
        let record = LogRecord { contents: continued, runs: Runs::Checked(vec![crc]) };
        let log = Arc::new(dir.open(&log_path(segment(3)), false).unwrap());
        let salt = new_salt(0, segment(3));
        let encoded = crate::layout::encode_salted(&record, salt);
        SegmentLog::new(log, salt).append([encoded.as_slice()]).unwrap();
        left_open(dir, 4, 1, &[commit(5, 6, 0)]);
    }

    #[test]
    fn commits_whose_runs_a_log_refuses_go() {
        let dir = MemDir::new();
        refusing_two(&dir);
        let recovered = recover(&dir, BRANCH).unwrap();
        let last: Vec<u64> =
            recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [2, 3, 4]);
        assert_eq!(files(&dir, SEGMENTS), [0, 1, 2]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: so too after a recovery stopped after each of its operations, as by a crash, which
    /// may keep any of what it removed: segment 4 goes, durably, before segment 3, whose log says
    /// why both go; removed first, a crash kept segment 3's raw file without its log, which then
    /// read as gone durable, and segment 4's commit was kept, past the lost one (TASK-334, found by
    /// TASK-310's opening rounds, round 28).
    #[test]
    fn commits_whose_runs_a_log_refuses_go_whatever_a_recovery_stopped_left() {
        for operations in 0.. {
            let dir = MemDir::serial();
            refusing_two(&dir);
            dir.fail_after(Some(operations));
            let stopped = recover(&dir, BRANCH).is_err();
            dir.fail_after(None);
            for seed in 0..8 {
                let recovered = recover(&dir.crash(seed), BRANCH).unwrap();
                let last: Vec<u64> =
                    recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
                assert_eq!(last, [2, 3, 4], "stopped after {operations} operations, seed {seed}");
            }
            if !stopped {
                break;
            }
        }
    }

    /// fxdax: a segment the base acknowledged, its raw file brought back by a crash without its log
    /// or framed file, breaks no run of whole framed files: the segments after it are read from
    /// theirs, though their raw files and logs are still there, their commits durable (TASK-310).
    #[test]
    fn segments_acknowledged_brought_back_break_no_whole_run() {
        let dir = MemDir::new();
        let framed = framed_three(&dir);
        let (mut base, _) = BaseFile::read(&dir).unwrap();
        base.write(&dir, Some(tag(2, 2))).unwrap();
        dir.remove(&segment_path(segment(0))).unwrap();
        dir.create(&open_segment_path(segment(0)), true).unwrap();
        // Segment 2's commit after its framing, its raw file and log not gone yet.
        left_open(&dir, 2, 1, &[]);
        let recovered = recover(&dir, BRANCH).unwrap();
        assert_eq!(recovered.segments, framed[1..]);
    }

    /// fxdax: a log whose header is another segment's holds none of its segment's commits: a spare,
    /// renamed from segment 3's log, then renamed segment 6's, neither rename durable, its header
    /// and records then 6's, which a crash kept under 3's name. Read as 3's, its commits were kept
    /// with segment 3's raw data, which holds none of their runs (TASK-341, TASK-310's crash
    /// rounds, round 81).
    #[test]
    fn logs_of_other_segments_hold_none_of_their_names() {
        let dir = MemDir::new();
        framed_three(&dir);
        let raw = dir.create(&open_segment_path(segment(3)), true).unwrap();
        raw.write_at(&[3; 4096], 0).unwrap();
        logged(&dir, 3, 6, &[commit(5, 5, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let last: Vec<u64> =
            recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [2, 3, 4]);
        assert_eq!(files(&dir, SEGMENTS), [0, 1, 2]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: a segment the base acknowledged, its framed file gone, its raw file and log back, a
    /// crash undoing their removal, its log a spare taken since as another segment's, breaks no run
    /// of segments framed whole: those after it are read framed, though their raw files and logs are
    /// back too, their logs spares taken since. Its log taken for its own, holding no record, it
    /// broke the run, and the segments after it were closed again from logs holding none of their
    /// records, commits the uploader had lost (TASK-369, the Shuttle rounds, round
    /// writer_closer_and_uploader_randomly's: "uploaded commit 3 before durable").
    #[test]
    fn logs_taken_since_break_no_run_of_whole_segments() {
        let dir = MemDir::new();
        framed_three(&dir);
        let (mut base, _) = BaseFile::read(&dir).unwrap();
        base.write(&dir, Some(tag(2, 2))).unwrap();
        dir.remove(&segment_path(segment(0))).unwrap();
        for (number, of) in [(0, 9), (1, 8)] {
            let raw = dir.create(&open_segment_path(segment(number)), true).unwrap();
            raw.write_at(&[number as u8; 4096], 0).unwrap();
            logged(&dir, number, of, &[]);
        }
        let recovered = recover(&dir, BRANCH).unwrap();
        let last: Vec<u64> =
            recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [3, 4]);
        assert_eq!(files(&dir, SEGMENTS), [1, 2]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// fxdax: a segment left open whose log goes on from the base's commit is kept, though the
    /// segments the base acknowledged that a crash brought back end before it: the segment of the
    /// base's commit left for the host's cache, those before it its files' removal undone
    /// (TASK-334).
    #[test]
    fn logs_following_the_base_are_kept() {
        let dir = MemDir::new();
        framed_three(&dir);
        let (mut base, _) = BaseFile::read(&dir).unwrap();
        base.write(&dir, Some(tag(2, 4))).unwrap();
        dir.remove(&segment_path(segment(2))).unwrap();
        left_open(&dir, 3, 1, &[commit(5, 5, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let last: Vec<u64> =
            recovered.segments.iter().map(|(footer, _)| footer_last(footer)).collect();
        assert_eq!(last, [5]);
        assert_eq!(files(&dir, SEGMENTS), [3]);
    }

    /// A segment whose framed file isn't whole, its log lacking its last commit, not durable,
    /// closes again with the commits it holds, and the segments after it go, though whole: they
    /// follow the commit it lacks (TASK-165.9).
    #[test]
    fn segments_after_a_log_lacking_its_last_commits_go() {
        let dir = MemDir::new();
        framed_three(&dir);
        dir.remove(&segment_path(segment(0))).unwrap();
        left_open(&dir, 0, 1, &[commit(1, 1, 0)]);
        let recovered = recover(&dir, BRANCH).unwrap();
        let [(footer, 0)] = &recovered.segments[..] else {
            panic!("recovered {:?}", recovered.segments)
        };
        assert_eq!(footer.contents.commits, [tag(2, 1)]);
        assert_eq!(files(&dir, SEGMENTS), [0]);
        assert_eq!((files(&dir, OPEN_SEGMENTS), files(&dir, LOGS)), (vec![], vec![]));
    }

    /// A segment whose framed file isn't whole, its raw data and log gone, as a crash part way
    /// through removing it leaves, it holding nothing durable, goes, rather than refusing the
    /// store: its raw data and log go otherwise only once it is framed whole (found by the crash
    /// rounds cutting openings, TASK-165.9).
    #[test]
    fn segments_torn_without_raw_data_or_log_go() {
        let dir = MemDir::new();
        let framed = framed_three(&dir);
        let torn = dir.create(&segment_path(segment(3)), true).unwrap();
        torn.write_at(&[9; 100], 0).unwrap();
        let recovered = recover(&dir, BRANCH).unwrap();
        assert_eq!(recovered.segments, framed);
        assert_eq!(files(&dir, SEGMENTS), [0, 1, 2]);
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
        let seqs = |group: Vec<Queued>| group.iter().map(|q| q.footer.seq).collect::<Vec<_>>();
        assert_eq!(queue.acknowledge(1).map(seqs), Some(vec![1]));
        assert_eq!((queue.base(), queue.bytes()), (Some(tag(2, 5)), 200));
    }

    /// fxdax: acknowledged, the segments going on with a commit are held until the one ending it
    /// is acknowledged too, then given together, the base past the commit: let go one at a time, a
    /// crash between left a hole in the commit's segments here, which recovery read as the commit
    /// unended (TASK-338, TASK-310's crash rounds, round 65).
    #[test]
    fn queues_hold_acknowledged_segments_going_on_with_a_commit() {
        let footer = |seq, commits: Vec<u64>, continued: Option<u64>| Footer {
            branch: BRANCH,
            seq,
            epoch: 2,
            after: None,
            contents: Contents {
                commits: commits.into_iter().map(|epoch| tag(2, epoch)).collect(),
                continued: continued
                    .map(|epoch| Continued { commit: tag(2, epoch), extents: Vec::new() }),
                ..Contents::default()
            },
        };
        let mut queue = Queue::new(Some(tag(1, 4)), []);
        // 1 ends commit 5; 2 ends 6 and goes on with 7, which 3 goes on with, and 4 ends; 5 ends 8.
        queue.push(1, footer(1, vec![5], None), 100);
        queue.push(2, footer(2, vec![6], Some(7)), 100);
        queue.push(3, footer(3, vec![], Some(7)), 100);
        queue.push(4, footer(4, vec![7], None), 100);
        queue.push(5, footer(5, vec![8], None), 100);
        let numbers = |group: Vec<Queued>| group.iter().map(|q| q.number).collect::<Vec<_>>();
        assert_eq!(queue.acknowledge(1).map(numbers), Some(vec![1]));
        assert_eq!(queue.acknowledge(2).map(numbers), Some(vec![]));
        assert_eq!(queue.base(), Some(tag(2, 6)));
        assert_eq!(queue.acknowledge(3).map(numbers), Some(vec![]));
        assert_eq!(queue.acknowledge(4).map(numbers), Some(vec![2, 3, 4]));
        assert_eq!(queue.base(), Some(tag(2, 7)));
    }
}
