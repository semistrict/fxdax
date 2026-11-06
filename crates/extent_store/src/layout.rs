// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What a store keeps in its directory. See the crate's docs.

use crate::dir::Dir;
use anyhow::{Context as _, Error, anyhow, ensure};
#[cfg(test)]
use extent_frame::segment::Contents;
use extent_frame::segment::Location;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use storage_device::ExtentId;

/// The first line of a store's description: its format. fxdax: 6 names the bucket a host's store
/// keeps its branches in (TASK-165.6); 7 places an extent whose data is in the bucket by its
/// location there (TASK-165.8), and holds one branch, no branch records (TASK-165.7); 8 has no
/// index: its branch is the bucket's, its write-ahead log and the open segment's log of its
/// commits, and its description names its capacity until a commit sets one, and its placement's
/// fencing token, if placed (TASK-165.9). A store of another format is refused ([`Format`]):
/// nothing reads, converts or upgrades it.
pub(crate) const FORMAT: &str = "fxdax extent store 8";

/// fxdax: the error of a store whose description is of another format than [`FORMAT`], an older
/// one included: refused, as nothing reads it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a store of format {found:?}, not {FORMAT:?}: refused")]
pub struct Format {
    /// The description's first line.
    pub found: String,
}

/// A store, as its user sees it: the size of the blocks extents are read and written in, the most
/// bytes its extents may hold, and its branch, whose extents its user mints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub block_size: u32,
    pub capacity: u64,
    pub branch: u64,
}

/// The store's description, its `store` file: its format, block size, and branch; and fxdax: the
/// URL of the bucket its branch is kept in, which every store in a host's directory names
/// (`create_host`, TASK-165.6), and a store of memory's need not; the capacity its branch has
/// until a commit sets one, as a branch made for it, or taken into it, says none; and the fencing
/// token of its placement, if it was placed (TASK-39.7), which takes the branch's lease
/// (TASK-165.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreFile {
    pub block_size: u32,
    pub branch: u64,
    pub bucket: Option<String>,
    pub capacity: u64,
    pub token: Option<u64>,
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
        let found = text.lines().next().unwrap_or_default();
        if found != FORMAT {
            return Err(Format { found: found.to_owned() }.into());
        }
        let fields = fields(text, FORMAT)?;
        let token = fields.contains_key("token").then(|| number(&fields, "token")).transpose()?;
        Ok(Self {
            block_size: number(&fields, "block_size")?.try_into().context("bad block_size")?,
            branch: number(&fields, "branch")?,
            bucket: fields.get("bucket").cloned(),
            capacity: number(&fields, "capacity")?,
            token,
        })
    }

    pub(crate) fn text(&self) -> String {
        let Self { block_size, branch, bucket, capacity, token } = self;
        let mut text = format!("{FORMAT}\nblock_size {block_size}\nbranch {branch}\n");
        text += &format!("capacity {capacity}\n");
        if let Some(url) = bucket {
            text += &format!("bucket {url}\n");
        }
        if let Some(token) = token {
            text += &format!("token {token}\n");
        }
        text
    }
}

/// The description's file.
pub(crate) const DESCRIPTION: &str = "store";
/// The directory of segments, a directory for each branch's.
pub(crate) const SEGMENTS: &str = "segments";
/// The directory of extents stored on their own, a directory for each branch's.
pub(crate) const EXTENTS: &str = "extents";
/// fxdax: the directory of the open segments' logs, a directory for each branch's: the commits each
/// holds, appended and synced by the flushes that make them durable (TASK-165.9).
pub(crate) const LOGS: &str = "logs";
/// The directory of open segments' files, raw, a directory for each branch's.
pub(crate) const OPEN_SEGMENTS: &str = "open-segments";
/// The directory of the files of open extents stored on their own, raw, a directory for each
/// branch's.
pub(crate) const OPEN_EXTENTS: &str = "open-extents";

/// The directory of `branch`'s files of `kind` (`SEGMENTS`, `EXTENTS`, `LOGS`, ...).
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

/// fxdax: the path of the log of the commits of a segment, while it is open (TASK-165.9).
pub(crate) fn log_path(segment: SegmentId) -> String {
    format!("{}/{:016x}", branch_dir(LOGS, segment.branch), segment.number)
}

/// The number in a file's name: a segment's, a log's, an extent's sequence number, or a branch's
/// directory's.
pub(crate) fn parse_number(name: &str) -> Option<u64> {
    (name.len() == 16).then(|| u64::from_str_radix(name, 16).ok()).flatten()
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

/// The directories of a store's files, each a directory for each branch's.
pub(crate) const KINDS: [&str; 5] = [SEGMENTS, EXTENTS, LOGS, OPEN_SEGMENTS, OPEN_EXTENTS];

/// Creates `branch`'s directories of segments, extents, logs and open files, where it has none.
pub(crate) fn create_branch_dirs<D: Dir>(dir: &D, branch: u64) -> Result<(), Error> {
    for kind in KINDS {
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
/// written last, or a file of its own; or, fxdax, in the bucket, at its location there, which the
/// store's user reads it from (TASK-165.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Data {
    Pieces(Vec<Piece>),
    Own,
    Remote(Location),
}

impl Data {
    /// fxdax: if in the bucket, named at `to`, where the collector moved it, if it is named on its
    /// way there (`extent_remote::layout::further`, TASK-195).
    pub fn follow(&mut self, to: &Location) {
        if let Data::Remote(at) = self
            && extent_remote::layout::further(&at.key, &to.key)
        {
            *at = to.clone();
        }
    }
}

/// An extent: its length, the end of its furthest write, and where its data is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extent {
    pub len: u64,
    pub data: Data,
}

/// A branch's extents, by their IDs.
pub(crate) type Extents = HashMap<ExtentId, Extent>;

/// A file of extents' data.
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
            Data::Remote(_) => {}
        }
    }
    files
}

/// A change a commit makes to the branch, which the next commit records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// A piece of the extent's data was written to a segment.
    Write { extent: ExtentId, piece: Piece },
    /// The extent's own file was written, which it holds `len` bytes of now.
    WriteOwn { extent: ExtentId, len: u64 },
    /// The extent was freed.
    Free(ExtentId),
    /// The extent's data was copied to the pieces, which replace its others: sealing it in one
    /// run.
    Move { extent: ExtentId, pieces: Vec<Piece> },
    /// The extent's data was copied to its own file, sealing it at its length.
    MoveOwn { extent: ExtentId },
}

/// fxdax: of `changes`, an epoch's, the extents it freed that a commit held: but those written in
/// it, open, which their frees let go of before any commit held them (TASK-79.1). Those of them
/// that are the branch's own it spends, as the store spends them as it frees them (TASK-79.6);
/// and a footer frees them alone, so that what its readers spend of it, all its frees, is that
/// too (TASK-160.15).
pub(crate) fn held_frees(changes: &[Change]) -> HashSet<ExtentId> {
    let (mut written, mut freed) = (HashSet::new(), HashSet::new());
    for change in changes {
        match change {
            Change::Write { extent, .. } | Change::WriteOwn { extent, .. } => {
                written.insert(*extent);
            }
            Change::Free(extent) if !written.contains(extent) => {
                freed.insert(*extent);
            }
            _ => {}
        }
    }
    freed
}

/// Where an extent's data is, as a store's user sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// One run in a segment of a branch's, at an offset: the extent is sealed.
    Run { branch: u64, number: u64, offset: u64 },
    /// Its own file.
    Own,
    /// fxdax: in the bucket, at its location there.
    Remote(Location),
    /// Pieces, as it was written: the extent is open.
    Pieces,
}

/// A record's header: its payload's length, and its hash.
const HEADER: usize = 4 + 8;

/// The seed of records' hashes.
const SEED: u64 = 0x6678_6461_7832_0001;

fn hash(payload: &[u8]) -> u64 {
    rapidhash::rapidhash_seeded(payload, SEED ^ payload.len() as u64)
}

/// `record`, as a log holds it: its header, then its payload.
pub(crate) fn encode<R: Serialize>(record: &R) -> Vec<u8> {
    let payload = bincode::serialize(record).expect("records serialize");
    let len = u32::try_from(payload.len()).expect("a record is under 4 GiB");
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&hash(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// The records at the start of `bytes` that are whole and intact, and how many bytes they take.
pub(crate) fn decode<R: serde::de::DeserializeOwned>(bytes: &[u8]) -> (Vec<R>, usize) {
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

    /// The record of a commit of `epoch`'s, as an open segment's log holds it (TASK-165.9).
    fn commit(epoch: u64) -> Contents {
        Contents {
            commits: vec![extent_frame::segment::tag(3, epoch)],
            extents: vec![(ExtentId::new(1, 2), 4096, 8192)],
            frees: vec![ExtentId::new(1, 1)],
            root: Some(b"root".to_vec()),
            next_sequence: 9,
            capacity: Some(1 << 20),
            ..Contents::default()
        }
    }

    fn epochs(records: &[Contents]) -> Vec<u64> {
        let last = |record: &Contents| extent_frame::segment::epoch_of(record.commits[0]);
        records.iter().map(last).collect()
    }

    #[test]
    fn records_decode_until_one_is_torn() {
        let bytes = [encode(&commit(1)), encode(&commit(2)), encode(&commit(3))].concat();
        let (records, len) = decode::<Contents>(&bytes);
        assert_eq!(epochs(&records), vec![1, 2, 3]);
        assert_eq!(len, bytes.len());

        let second = encode(&commit(1)).len();
        // Torn in the third record, or a byte of it changed.
        let (records, len) = decode::<Contents>(&bytes[..bytes.len() - 1]);
        assert_eq!((epochs(&records), len), (vec![1, 2], 2 * second));
        let mut changed = bytes.clone();
        changed[2 * second + HEADER + 1] ^= 1;
        assert_eq!(epochs(&decode::<Contents>(&changed).0), vec![1, 2]);
        // Zeros past the records, as a file that grew holds.
        let zeros = [bytes.clone(), vec![0; 4096]].concat();
        assert_eq!(decode::<Contents>(&zeros), (decode(&bytes).0, bytes.len()));
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

    #[test]
    fn descriptions() {
        let file = StoreFile {
            block_size: 4096,
            branch: u64::MAX,
            bucket: None,
            capacity: 1,
            token: None,
        };
        assert_eq!(StoreFile::parse(&file.text()).unwrap(), file);
        // fxdax: a host's store names its bucket (TASK-165.6), and a placed one its placement's
        // token (TASK-165.9).
        let bucket = Some("file:///images/a/bucket".to_owned());
        let kept = StoreFile { bucket, token: Some(7), ..file.clone() };
        assert_eq!(StoreFile::parse(&kept.text()).unwrap(), kept);
        // fxdax: older formats are refused, by their format (TASK-165.6), the index's last
        // (TASK-165.9).
        for older in ["fxdax extent store 2", "fxdax extent store 6", "fxdax extent store 7"] {
            let refused = StoreFile::parse(&format!("{older}\nblock_size 4096\nbranch 1\n"));
            let found = refused.unwrap_err().downcast::<Format>().unwrap().found;
            assert_eq!(found, older);
        }
        let capacity = "capacity 4096\n";
        assert!(StoreFile::parse(&format!("{FORMAT}\nblock_size 4096\n{capacity}")).is_err());
        assert!(StoreFile::parse(&format!("{FORMAT}\nblock_size 4096\nbranch 1\n")).is_err());
    }

    #[test]
    fn names() {
        let extent = ExtentId::new(0xb, 0x5a);
        assert_eq!(extent_path(extent), "extents/000000000000000b/000000000000005a");
        let segment = SegmentId { branch: 0xb, number: 77 };
        assert_eq!(segment_path(segment), "segments/000000000000000b/000000000000004d");
        assert_eq!(log_path(segment), "logs/000000000000000b/000000000000004d");
        assert_eq!(parse_number("000000000000004d"), Some(77));
        assert_eq!(parse_number("77"), None);
    }
}
