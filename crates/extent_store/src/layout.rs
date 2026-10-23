// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What a store keeps in its directory. See the crate's docs.

use crate::dir::Dir;
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use storage_device::ExtentId;

/// The first line of a store's description: its format.
pub(crate) const FORMAT: &str = "fxdax extent store 4";

/// A branch of a store, as its user sees it: the size of the blocks extents are read and written
/// in, which is the store's, the most bytes the branch's extents may hold, and the branch, whose
/// extents its user mints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub block_size: u32,
    pub capacity: u64,
    pub branch: u64,
}

/// The store's description, its `store` file: its format, block size, and first branch, which
/// opening the store, rather than one of its branches, opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoreFile {
    pub block_size: u32,
    pub first: u64,
}

/// The fields of a text of lines of `key value`, after its first, which is `first`.
fn fields(text: &str, first: &str) -> Result<HashMap<String, String>, Error> {
    let mut lines = text.lines();
    ensure!(lines.next() == Some(first), "not {first:?}");
    let mut fields = HashMap::new();
    for line in lines {
        let (key, value) = line.split_once(' ').ok_or_else(|| anyhow!("bad line {line:?}"))?;
        fields.insert(key.to_owned(), value.to_owned());
    }
    Ok(fields)
}

fn number(fields: &HashMap<String, String>, key: &str) -> Result<u64, Error> {
    let value = fields.get(key).ok_or_else(|| anyhow!("no {key}"))?;
    value.parse().with_context(|| format!("bad {key}"))
}

impl StoreFile {
    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        let fields = fields(text, FORMAT).context("not an extent store of this format")?;
        Ok(Self {
            block_size: number(&fields, "block_size")?.try_into().context("bad block_size")?,
            first: number(&fields, "branch")?,
        })
    }

    pub(crate) fn text(&self) -> String {
        format!("{FORMAT}\nblock_size {}\nbranch {}\n", self.block_size, self.first)
    }
}

/// Where a branch was forked from: its parent, and the parent's epoch, next sequence number and
/// index file as of the fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    pub parent: u64,
    pub epoch: u64,
    pub next_sequence: u64,
    pub index: u64,
}

/// A branch's record, its newest `branches/<branch>.<version>` file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BranchRecord {
    pub capacity: u64,
    pub origin: Option<Origin>,
    /// Whether its state, a fork's, is in its own index yet, or still its parent's at the fork.
    pub resolved: bool,
    /// Whether its index, a fork's, holds what it changed over its parent's state at the fork,
    /// rather than all it holds (TASK-31); forks before it was, opened, hold all.
    pub layered: bool,
    pub deleted: bool,
}

/// The first line of a branch's record.
const RECORD: &str = "fxdax extent store branch";

impl BranchRecord {
    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        let fields = fields(text, RECORD)?;
        let origin = match fields.get("parent") {
            None => None,
            Some(parent) => {
                let numbers: Vec<u64> = parent
                    .split(' ')
                    .map(|number| number.parse().context("bad parent"))
                    .collect::<Result<_, _>>()?;
                let [parent, epoch, next_sequence, index] = numbers[..] else {
                    bail!("bad parent {parent:?}");
                };
                Some(Origin { parent, epoch, next_sequence, index })
            }
        };
        let flag = |key| Ok::<_, Error>(number(&fields, key)? != 0);
        ensure!(fields.contains_key("end"), "a torn record");
        Ok(Self {
            capacity: number(&fields, "capacity")?,
            origin,
            resolved: flag("resolved")?,
            // Not in records written before forks were layered.
            layered: fields.contains_key("layered") && flag("layered")?,
            deleted: flag("deleted")?,
        })
    }

    /// Where it was forked from, if its state is over its parent's at the fork: a fork's not yet
    /// opened, or whose index holds what it changed over it.
    pub(crate) fn over_parent(&self) -> Option<Origin> {
        self.origin.filter(|_| self.layered || !self.resolved)
    }

    pub(crate) fn text(&self) -> String {
        let parent = self.origin.map_or(String::new(), |origin| {
            let Origin { parent, epoch, next_sequence, index } = origin;
            format!("parent {parent} {epoch} {next_sequence} {index}\n")
        });
        format!(
            "{RECORD}\ncapacity {}\n{parent}resolved {}\nlayered {}\ndeleted {}\nend .\n",
            self.capacity, self.resolved as u8, self.layered as u8, self.deleted as u8
        )
    }
}

/// The description's file.
pub(crate) const DESCRIPTION: &str = "store";
/// The directory of branches' records.
pub(crate) const BRANCHES: &str = "branches";
/// The directory of segments, a directory for each branch's.
pub(crate) const SEGMENTS: &str = "segments";
/// The directory of extents stored on their own, a directory for each branch's.
pub(crate) const EXTENTS: &str = "extents";
/// The directory of the index's files, a directory for each branch's.
pub(crate) const INDEX: &str = "index";
/// The directory of open segments' files, raw, a directory for each branch's.
pub(crate) const OPEN_SEGMENTS: &str = "open-segments";
/// The directory of the files of open extents stored on their own, raw, a directory for each
/// branch's.
pub(crate) const OPEN_EXTENTS: &str = "open-extents";

/// The directory of `branch`'s files of `kind` (`SEGMENTS`, `EXTENTS`, `INDEX`).
pub(crate) fn branch_dir(kind: &'static str, branch: u64) -> String {
    format!("{kind}/{branch:016x}")
}

/// A segment: the branch that wrote it, and its number among the branch's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct SegmentId {
    pub branch: u64,
    pub number: u64,
}

/// The path of a segment.
pub(crate) fn segment_path(segment: SegmentId) -> String {
    format!("{}/{:016x}", branch_dir(SEGMENTS, segment.branch), segment.number)
}

/// The path of the raw file of a segment while it is open.
pub(crate) fn open_segment_path(segment: SegmentId) -> String {
    format!("{}/{:016x}", branch_dir(OPEN_SEGMENTS, segment.branch), segment.number)
}

/// The path of the raw file of `extent`, stored on its own, while it is open.
pub(crate) fn open_extent_path(extent: ExtentId) -> String {
    format!("{}/{:016x}", branch_dir(OPEN_EXTENTS, extent.branch), extent.sequence)
}

/// The path of the file of `extent`, stored on its own, in its branch's directory.
pub(crate) fn extent_path(extent: ExtentId) -> String {
    format!("{}/{:016x}", branch_dir(EXTENTS, extent.branch), extent.sequence)
}

/// The path of `branch`'s index file `number`.
pub(crate) fn index_path(branch: u64, number: u64) -> String {
    format!("{}/{number:016x}", branch_dir(INDEX, branch))
}

/// The path of `branch`'s record's `version`.
pub(crate) fn record_path(branch: u64, version: u64) -> String {
    format!("{BRANCHES}/{branch:016x}.{version:016x}")
}

/// The number in a file's name: a segment's, an index file's, an extent's sequence number, or a
/// branch's directory's.
pub(crate) fn parse_number(name: &str) -> Option<u64> {
    (name.len() == 16).then(|| u64::from_str_radix(name, 16).ok()).flatten()
}

/// The branch and version of a record's file's name.
pub(crate) fn parse_record_name(name: &str) -> Option<(u64, u64)> {
    let (branch, version) = name.split_once('.')?;
    Some((parse_number(branch)?, parse_number(version)?))
}

/// The numbers of the files in `path`, in order.
pub(crate) fn list_numbers<D: Dir>(dir: &D, path: &str) -> Result<Vec<u64>, Error> {
    let mut numbers = Vec::new();
    for name in dir.list(path)? {
        numbers.push(parse_number(&name).ok_or_else(|| anyhow!("{path}/{name} is no file"))?);
    }
    numbers.sort_unstable();
    Ok(numbers)
}

/// Creates `branch`'s directories of segments, extents and index files, where it has none.
pub(crate) fn create_branch_dirs<D: Dir>(dir: &D, branch: u64) -> Result<(), Error> {
    for kind in [SEGMENTS, EXTENTS, INDEX, OPEN_SEGMENTS, OPEN_EXTENTS] {
        let path = branch_dir(kind, branch);
        match dir.create_dir(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            created => created.with_context(|| format!("creating {path}"))?,
        }
    }
    Ok(())
}

/// `len` bytes of an extent's data, from `extent_offset` in the extent, at `offset` in segment
/// `segment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Piece {
    pub segment: SegmentId,
    pub offset: u64,
    pub extent_offset: u64,
    pub len: u64,
}

impl Piece {
    /// fxdax: this piece and `other` as one, if they are one run: one right after the other in
    /// the extent, and so in the segment, whichever comes first.
    fn joined(&self, other: &Piece) -> Option<Piece> {
        let (first, second) =
            if self.extent_offset < other.extent_offset { (self, other) } else { (other, self) };
        let run = first.segment == second.segment
            && first.extent_offset + first.len == second.extent_offset
            && first.offset + first.len == second.offset;
        run.then(|| Piece { len: first.len + second.len, ..*first })
    }
}

/// Adds `piece`, written last, to `pieces`, an extent's, joined to the last while they are one
/// run: they hold none of each other's bytes, so that which is painted over which does not matter.
/// fxdax: an extent written in runs one after another in a segment, as the guest's device splits a
/// write larger than its most I/O, is so one piece, which its commit seals where it is, rather than
/// reading it back and writing it again (TASK-92.1).
pub(crate) fn add_piece(pieces: &mut Vec<Piece>, mut piece: Piece) {
    while let Some(joined) = pieces.last().and_then(|last| last.joined(&piece)) {
        pieces.pop();
        piece = joined;
    }
    pieces.push(piece);
}

/// Where an extent's data is: pieces in segments, the later of overlapping pieces holding the data
/// written last, or a file of its own; or, fxdax, elsewhere: in object storage, which the store's
/// user reads it from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Data {
    Pieces(Vec<Piece>),
    Own,
    Remote,
}

/// An extent: its length, the end of its furthest write, and where its data is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extent {
    pub len: u64,
    pub data: Data,
}

/// A file of extents' data, which the branches holding extents in it claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum FileKey {
    Segment(SegmentId),
    Own(ExtentId),
}

impl FileKey {
    /// Its paths: framed, and raw, while it is open.
    pub(crate) fn paths(&self) -> [String; 2] {
        match self {
            Self::Segment(segment) => [segment_path(*segment), open_segment_path(*segment)],
            Self::Own(extent) => [extent_path(*extent), open_extent_path(*extent)],
        }
    }

    /// fxdax: the branch that made it, among whose files it is.
    pub(crate) fn branch(&self) -> u64 {
        match self {
            Self::Segment(segment) => segment.branch,
            Self::Own(extent) => extent.branch,
        }
    }
}

/// The files `extents`' data is in.
pub(crate) fn files_of<'a>(
    extents: impl IntoIterator<Item = (&'a ExtentId, &'a Extent)>,
) -> BTreeSet<FileKey> {
    let mut files = BTreeSet::new();
    for (extent, entry) in extents {
        match &entry.data {
            Data::Pieces(pieces) => {
                files.extend(pieces.iter().map(|piece| FileKey::Segment(piece.segment)));
            }
            Data::Own => {
                files.insert(FileKey::Own(*extent));
            }
            Data::Remote => {}
        }
    }
    files
}

/// A branch's index as of an epoch, whose extents are all sealed, and the number of the segment
/// open then, whose footer, and those of the segments closed after it, follow it. A fork's, over
/// its parent's state at the fork, holds the extents it holds otherwise, and those of its parent's
/// it holds no longer, `gone` (TASK-31).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checkpoint {
    pub epoch: u64,
    pub next_sequence: u64,
    pub root: Vec<u8>,
    pub segment: u64,
    pub extents: Vec<(ExtentId, Extent)>,
    pub gone: Vec<ExtentId>,
}

/// A checkpoint's state but for its extents, its table's footer holds.
#[derive(Serialize, Deserialize)]
struct Rest {
    epoch: u64,
    next_sequence: u64,
    root: Vec<u8>,
    segment: u64,
}

/// The key of an extent's place in a checkpoint's table, if its data is elsewhere.
const REMOTE: &str = "remote";
/// The key of a row of a fork's checkpoint's table naming an extent of its parent's it holds no
/// longer.
const GONE: &str = "gone";

impl Checkpoint {
    /// Its table, in doc-7's format, as checkpoints in object storage are (`extent_frame::table`):
    /// each extent in one run of a segment, or a file of its own, or elsewhere.
    pub(crate) fn table(&self) -> Result<Vec<u8>, Error> {
        let mut entries = Vec::with_capacity(self.extents.len());
        for (extent, entry) in &self.extents {
            let (key, offset) = match &entry.data {
                Data::Pieces(pieces) => match pieces[..] {
                    [piece] if piece.extent_offset == 0 && piece.len == entry.len => {
                        (segment_path(piece.segment), piece.offset)
                    }
                    _ => bail!("{extent:?} is not sealed in one run"),
                },
                Data::Own => (extent_path(*extent), 0),
                Data::Remote => (REMOTE.to_owned(), 0),
            };
            entries.push(((extent.branch, extent.sequence), key, offset, entry.len));
        }
        for extent in &self.gone {
            entries.push(((extent.branch, extent.sequence), GONE.to_owned(), 0, 0));
        }
        entries.sort_by_key(|(id, ..)| *id);
        let rest = Rest {
            epoch: self.epoch,
            next_sequence: self.next_sequence,
            root: self.root.clone(),
            segment: self.segment,
        };
        let rest = bincode::serialize(&rest).expect("checkpoints serialize");
        let rows = entries.iter().map(|(id, key, offset, len)| (*id, key.as_str(), *offset, *len));
        Ok(extent_frame::table::encode(rows, rest))
    }

    /// The checkpoint whose table is `bytes`.
    pub(crate) fn of_table(bytes: &[u8]) -> Result<Self, Error> {
        let (footer, rows) = extent_frame::table::decode(bytes)?;
        let Rest { epoch, next_sequence, root, segment } =
            bincode::deserialize(&footer.rest).context("a checkpoint's state")?;
        let (mut extents, mut gone) = (Vec::with_capacity(rows.len()), Vec::new());
        for row in rows {
            let extent = ExtentId::new(row.id.0, row.id.1);
            if row.key == GONE {
                gone.push(extent);
                continue;
            }
            let data = if row.key == REMOTE {
                Data::Remote
            } else if row.key == extent_path(extent) {
                Data::Own
            } else {
                let segment = (row.key.strip_prefix(SEGMENTS).and_then(|rest| {
                    let (branch, number) = rest.strip_prefix('/')?.split_once('/')?;
                    Some(SegmentId { branch: parse_number(branch)?, number: parse_number(number)? })
                }))
                .ok_or_else(|| anyhow!("a checkpoint names {:?}", row.key))?;
                Data::Pieces(vec![Piece {
                    segment,
                    offset: row.offset,
                    extent_offset: 0,
                    len: row.len,
                }])
            };
            extents.push((extent, Extent { len: row.len, data }));
        }
        Ok(Self { epoch, next_sequence, root, segment, extents, gone })
    }
}

/// A change an epoch made to the index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Change {
    /// A piece of the extent's data was written to a segment.
    Write { extent: ExtentId, piece: Piece },
    /// The extent's own file was written, which it holds `len` bytes of now.
    WriteOwn { extent: ExtentId, len: u64 },
    /// The extent was freed.
    Free(ExtentId),
    /// The extent's data was copied to the pieces, which replace its others: cleaning, or sealing
    /// it in one run.
    Move { extent: ExtentId, pieces: Vec<Piece> },
    /// The extent's data was copied to its own file, sealing it at its length.
    MoveOwn { extent: ExtentId },
    /// fxdax: the store holds the extent, sealed, of `len` bytes, whose data is elsewhere.
    Adopt { extent: ExtentId, len: u64 },
}

/// An epoch's changes to the index, which a commit records.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Delta {
    pub epoch: u64,
    pub next_sequence: u64,
    /// The root the commit set, if it set one.
    pub root: Option<Vec<u8>>,
    pub changes: Vec<Change>,
}

/// Where an extent's data is, as a store's user sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// One run in a segment of a branch's, at an offset: the extent is sealed.
    Run { branch: u64, number: u64, offset: u64 },
    /// Its own file.
    Own,
    /// Elsewhere: in object storage.
    Remote,
    /// Pieces, as it was written: the extent is open.
    Pieces,
}

/// A record of an index file, which holds a checkpoint, its table ([`Checkpoint::table`]), and the
/// deltas of the epochs after it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Record {
    Checkpoint(Vec<u8>),
    Delta(Delta),
}

/// A record's header: its payload's length, and its hash.
const HEADER: usize = 4 + 8;

/// The seed of records' hashes.
const SEED: u64 = 0x6678_6461_7832_0001;

fn hash(payload: &[u8]) -> u64 {
    rapidhash::rapidhash_seeded(payload, SEED ^ payload.len() as u64)
}

/// `record`, as an index file holds it.
pub(crate) fn encode(record: &Record) -> Vec<u8> {
    let payload = bincode::serialize(record).expect("records serialize");
    let len = u32::try_from(payload.len()).expect("a record is under 4 GiB");
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&hash(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// The records at the start of `bytes` that are whole and intact, and how many bytes they take.
pub(crate) fn decode(bytes: &[u8]) -> (Vec<Record>, usize) {
    let mut records = Vec::new();
    let mut at = 0;
    while let Some(header) = bytes.get(at..at + HEADER) {
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let expected = u64::from_le_bytes(header[4..].try_into().unwrap());
        let Some(payload) = bytes.get(at + HEADER..at + HEADER + len) else { break };
        if len == 0 || hash(payload) != expected {
            break;
        }
        let Ok(record) = bincode::deserialize(payload) else { break };
        records.push(record);
        at += HEADER + len;
    }
    (records, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(epoch: u64) -> Record {
        Record::Delta(Delta {
            epoch,
            next_sequence: 9,
            root: Some(b"root".to_vec()),
            changes: vec![
                Change::Write {
                    extent: ExtentId::new(1, 2),
                    piece: Piece {
                        segment: SegmentId { branch: 1, number: 3 },
                        offset: 4096,
                        extent_offset: 0,
                        len: 8192,
                    },
                },
                Change::Free(ExtentId::new(1, 1)),
            ],
        })
    }

    fn epochs(records: &[Record]) -> Vec<u64> {
        records
            .iter()
            .map(|record| match record {
                Record::Delta(delta) => delta.epoch,
                Record::Checkpoint(table) => Checkpoint::of_table(table).unwrap().epoch,
            })
            .collect()
    }

    #[test]
    fn records_decode_until_one_is_torn() {
        let bytes = [encode(&delta(1)), encode(&delta(2)), encode(&delta(3))].concat();
        let (records, len) = decode(&bytes);
        assert_eq!(epochs(&records), vec![1, 2, 3]);
        assert_eq!(len, bytes.len());

        let second = encode(&delta(1)).len();
        // Torn in the third record, or a byte of it changed.
        let (records, len) = decode(&bytes[..bytes.len() - 1]);
        assert_eq!((epochs(&records), len), (vec![1, 2], 2 * second));
        let mut changed = bytes.clone();
        changed[2 * second + HEADER + 1] ^= 1;
        assert_eq!(epochs(&decode(&changed).0), vec![1, 2]);
        // Zeros past the records, as a file that grew holds.
        let zeros = [bytes.clone(), vec![0; 4096]].concat();
        assert_eq!(decode(&zeros), (decode(&bytes).0, bytes.len()));
    }

    /// fxdax: pieces one right after another, in the extent and so in the segment, are one,
    /// whichever was written first; those apart in either, holding each other's bytes, or in
    /// other segments, are not (TASK-92.1).
    #[test]
    fn pieces_of_one_run_are_one() {
        let piece = |number, offset, extent_offset, len| Piece {
            segment: SegmentId { branch: 1, number },
            offset,
            extent_offset,
            len,
        };
        let added = |written: &[Piece]| {
            let mut pieces = Vec::new();
            for &piece in written {
                add_piece(&mut pieces, piece);
            }
            pieces
        };
        let one = [piece(0, 0, 0, 12)];
        assert_eq!(added(&[piece(0, 0, 0, 8), piece(0, 8, 8, 4)]), one);
        assert_eq!(added(&[piece(0, 8, 8, 4), piece(0, 0, 0, 8)]), one);
        // The last two joined, then the first to them.
        let three = [piece(0, 8, 8, 4), piece(0, 0, 0, 4), piece(0, 4, 4, 4)];
        assert_eq!(added(&three), one);
        for apart in [
            [piece(0, 0, 0, 8), piece(0, 12, 8, 4)],
            [piece(0, 0, 0, 8), piece(0, 8, 12, 4)],
            [piece(0, 0, 0, 8), piece(0, 8, 4, 8)],
            [piece(0, 0, 0, 8), piece(1, 8, 8, 4)],
        ] {
            assert_eq!(added(&apart), apart);
        }
    }

    /// Checkpoints are tables, in doc-7's format, of sealed extents.
    #[test]
    fn checkpoints_are_tables() {
        let run = |number, offset, len| Extent {
            len,
            data: Data::Pieces(vec![Piece {
                segment: SegmentId { branch: 1, number },
                offset,
                extent_offset: 0,
                len,
            }]),
        };
        let checkpoint = Checkpoint {
            epoch: 9,
            next_sequence: 7,
            root: b"root".to_vec(),
            segment: 4,
            extents: vec![
                (ExtentId::new(1, 2), run(3, 4096, 8192)),
                (ExtentId::new(1, 5), Extent { len: 1 << 20, data: Data::Own }),
                (ExtentId::new(2, 1), Extent { len: 4096, data: Data::Remote }),
            ],
            gone: vec![ExtentId::new(1, 3)],
        };
        let table = checkpoint.table().unwrap();
        assert_eq!(Checkpoint::of_table(&table).unwrap(), checkpoint);
        assert_eq!(extent_frame::Frame::of(&table).unwrap().data_len, 4 * 36);
        let open = Extent { len: 8192, data: Data::Pieces(vec![]) };
        let unsealed = Checkpoint { extents: vec![(ExtentId::new(1, 9), open)], ..checkpoint };
        assert!(unsealed.table().is_err());
    }

    #[test]
    fn descriptions() {
        let file = StoreFile { block_size: 4096, first: u64::MAX };
        assert_eq!(StoreFile::parse(&file.text()).unwrap(), file);
        assert!(StoreFile::parse("fxdax extent store 2\nblock_size 4096\nbranch 1\n").is_err());
        assert!(StoreFile::parse(&format!("{FORMAT}\nblock_size 4096\n")).is_err());

        let origin = Origin { parent: 3, epoch: 9, next_sequence: 12, index: 2 };
        for record in [
            BranchRecord {
                capacity: 1 << 30,
                origin: None,
                resolved: true,
                layered: false,
                deleted: false,
            },
            BranchRecord {
                capacity: 7,
                origin: Some(origin),
                resolved: false,
                layered: true,
                deleted: true,
            },
        ] {
            assert_eq!(BranchRecord::parse(&record.text()).unwrap(), record);
            let text = record.text();
            // Torn.
            assert!(BranchRecord::parse(&text[..text.len() - 3]).is_err());
        }
        // Written before forks were layered: a fork opened holds all it holds.
        let old = "fxdax extent store branch\ncapacity 7\nparent 3 9 12 2\nresolved 1\ndeleted 0\nend .\n";
        let record = BranchRecord::parse(old).unwrap();
        assert!(!record.layered && record.over_parent().is_none());
    }

    #[test]
    fn names() {
        let extent = ExtentId::new(0xb, 0x5a);
        assert_eq!(extent_path(extent), "extents/000000000000000b/000000000000005a");
        let segment = SegmentId { branch: 0xb, number: 77 };
        assert_eq!(segment_path(segment), "segments/000000000000000b/000000000000004d");
        assert_eq!(index_path(0xb, 2), "index/000000000000000b/0000000000000002");
        assert_eq!(record_path(0xb, 1), "branches/000000000000000b.0000000000000001");
        assert_eq!(parse_record_name("000000000000000b.0000000000000001"), Some((0xb, 1)));
        assert_eq!(parse_number("000000000000004d"), Some(77));
        assert_eq!(parse_number("77"), None);
    }
}
