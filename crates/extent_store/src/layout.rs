//! What a store keeps in its directory. See the crate's docs.

use anyhow::{Context as _, Error, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use storage_device::ExtentId;

/// The first line of a store's description: its format.
pub(crate) const FORMAT: &str = "fxdax extent store 2";

/// The store's description, its `store` file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    /// The size of the blocks extents are read and written in.
    pub block_size: u32,
    /// The most bytes the store's extents may hold.
    pub capacity: u64,
    /// The branch whose extents the store's user mints.
    pub branch: u64,
}

impl Description {
    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        let mut lines = text.lines();
        ensure!(lines.next() == Some(FORMAT), "not an extent store of this format");
        let mut fields = HashMap::new();
        for line in lines {
            let (key, value) = line.split_once(' ').ok_or_else(|| anyhow!("bad line {line:?}"))?;
            fields.insert(key, value.parse::<u64>().with_context(|| format!("bad {key}"))?);
        }
        let field = |key| fields.get(key).copied().ok_or_else(|| anyhow!("no {key}"));
        Ok(Self {
            block_size: field("block_size")?.try_into().context("bad block_size")?,
            capacity: field("capacity")?,
            branch: field("branch")?,
        })
    }

    pub(crate) fn text(&self) -> String {
        let Self { block_size, capacity, branch } = self;
        format!("{FORMAT}\nblock_size {block_size}\ncapacity {capacity}\nbranch {branch}\n")
    }
}

/// The description's file.
pub(crate) const DESCRIPTION: &str = "store";
/// The directory of segments.
pub(crate) const SEGMENTS: &str = "segments";
/// The directory of extents stored on their own.
pub(crate) const EXTENTS: &str = "extents";
/// The directory of the index's files.
pub(crate) const INDEX: &str = "index";

/// The path of segment `number`.
pub(crate) fn segment_path(number: u64) -> String {
    format!("{SEGMENTS}/{number:016x}")
}

/// The path of the file of `extent`, stored on its own.
pub(crate) fn extent_path(extent: ExtentId) -> String {
    format!("{EXTENTS}/{:016x}.{:016x}", extent.sequence, extent.branch)
}

/// The path of the index's file `number`.
pub(crate) fn index_path(number: u64) -> String {
    format!("{INDEX}/{number:016x}")
}

/// The number in a segment's or an index file's name.
pub(crate) fn parse_number(name: &str) -> Option<u64> {
    (name.len() == 16).then(|| u64::from_str_radix(name, 16).ok()).flatten()
}

/// The extent whose own file has the name `name`.
pub(crate) fn parse_extent_name(name: &str) -> Option<ExtentId> {
    let (sequence, branch) = name.split_once('.')?;
    Some(ExtentId::new(parse_number(branch)?, parse_number(sequence)?))
}

/// `len` bytes of an extent's data, from `extent_offset` in the extent, at `offset` in segment
/// `segment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Piece {
    pub segment: u64,
    pub offset: u64,
    pub extent_offset: u64,
    pub len: u64,
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

/// The index as of an epoch.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    pub epoch: u64,
    /// The branch whose next sequence number the index keeps: the store's, unless the store is a
    /// copy, whose branch is new.
    pub branch: u64,
    pub next_sequence: u64,
    pub root: Vec<u8>,
    pub extents: Vec<(ExtentId, Extent)>,
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
    /// The extent's data was copied to the pieces, which replace its others: cleaning.
    Move { extent: ExtentId, pieces: Vec<Piece> },
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

/// A record of an index file, which holds a checkpoint and the deltas of the epochs after it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Record {
    Checkpoint(Checkpoint),
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
                    piece: Piece { segment: 3, offset: 4096, extent_offset: 0, len: 8192 },
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
                Record::Checkpoint(checkpoint) => checkpoint.epoch,
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

    #[test]
    fn descriptions() {
        let description = Description { block_size: 4096, capacity: 1 << 30, branch: u64::MAX };
        assert_eq!(Description::parse(&description.text()).unwrap(), description);
        assert!(Description::parse("fxdax extent store 1\nblock_size 4096\n").is_err());
        assert!(Description::parse(&format!("{FORMAT}\nblock_size 4096\n")).is_err());
    }

    #[test]
    fn names() {
        let extent = ExtentId::new(0xb, 0x5a);
        assert_eq!(extent_path(extent), "extents/000000000000005a.000000000000000b");
        assert_eq!(parse_extent_name(&extent_path(extent)["extents/".len()..]), Some(extent));
        assert_eq!(parse_number(&segment_path(77)["segments/".len()..]), Some(77));
        assert_eq!(parse_number("77"), None);
    }
}
