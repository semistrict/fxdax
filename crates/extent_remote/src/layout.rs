//! Where branches keep what, in a bucket, and what the objects hold (doc-7, Layout).
//!
//! Numbers in keys are fixed-width hex, so keys list in numeric order.

use crate::bucket::{self, Bucket, ByteRange};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

pub const BRANCHES: &str = "branches/";
pub const GC_LEASE: &str = "gc/lease";
const MOVES: &str = "moves/";

pub fn segments(branch: u64) -> String {
    format!("segments/{branch:016x}/")
}

/// Segment `seq` of `branch`: `seq` counts from 0 without gaps.
pub fn segment(branch: u64, seq: u64) -> String {
    format!("{}{seq:016x}", segments(branch))
}

pub fn extents(branch: u64) -> String {
    format!("extents/{branch:016x}/")
}

/// The object of an extent stored on its own, uploaded by the writer of epoch `epoch`.
pub fn extent(epoch: u64, extent: ExtentId) -> String {
    format!("{}{epoch:016x}.{:016x}", extents(extent.branch), extent.sequence)
}

pub fn checkpoints(branch: u64) -> String {
    format!("checkpoints/{branch:016x}/")
}

/// The checkpoint of `branch` as of its segment `seq`.
pub fn checkpoint(branch: u64, seq: u64) -> String {
    format!("{}{seq:016x}", checkpoints(branch))
}

pub fn record(branch: u64) -> String {
    format!("{BRANCHES}{branch:016x}")
}

pub fn lease(branch: u64) -> String {
    format!("leases/{branch:016x}")
}

/// What the collector made of the object at `key` when it compacted it: the extents of it that were
/// live, and where in it each is. Being one object, written whole, it is never there without them.
pub fn moves(key: &str) -> String {
    format!("{MOVES}{key}")
}

/// What kind of object a key names, and what its key says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    Segment { branch: u64, seq: u64 },
    Extent { branch: u64, epoch: u64, sequence: u64 },
    Checkpoint { branch: u64, seq: u64 },
    Record { branch: u64 },
    Lease { branch: u64 },
    GcLease,
    Moves { from: &'a str },
    Unknown,
}

fn hex(text: &str) -> Option<u64> {
    (text.len() == 16).then(|| u64::from_str_radix(text, 16).ok()).flatten()
}

fn pair(text: &str, separator: char) -> Option<(u64, u64)> {
    let (a, b) = text.split_once(separator)?;
    Some((hex(a)?, hex(b)?))
}

pub fn kind(key: &str) -> Kind<'_> {
    let parsed = || -> Option<Kind<'_>> {
        if key == GC_LEASE {
            return Some(Kind::GcLease);
        }
        if let Some(from) = key.strip_prefix(MOVES) {
            return Some(Kind::Moves { from });
        }
        if let Some(rest) = key.strip_prefix(BRANCHES) {
            return Some(Kind::Record { branch: hex(rest)? });
        }
        if let Some(rest) = key.strip_prefix("leases/") {
            return Some(Kind::Lease { branch: hex(rest)? });
        }
        if let Some(rest) = key.strip_prefix("segments/") {
            let (branch, seq) = pair(rest, '/')?;
            return Some(Kind::Segment { branch, seq });
        }
        if let Some(rest) = key.strip_prefix("checkpoints/") {
            let (branch, seq) = pair(rest, '/')?;
            return Some(Kind::Checkpoint { branch, seq });
        }
        if let Some(rest) = key.strip_prefix("extents/") {
            let (branch, rest) = rest.split_once('/')?;
            let (epoch, sequence) = pair(rest, '.')?;
            return Some(Kind::Extent { branch: hex(branch)?, epoch, sequence });
        }
        None
    };
    parsed().unwrap_or(Kind::Unknown)
}

/// Where an extent's bytes are: a range of an object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    pub key: String,
    pub offset: u64,
    pub len: u64,
}

/// A fork of a branch, made at the end of the commits of its segment `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    pub child: u64,
    pub at: u64,
}

/// What a segment holds after its data: the commits it makes. A segment with no data is a footer
/// alone: commits that only free extents or set the root, or forks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footer {
    pub branch: u64,
    pub seq: u64,
    /// The epoch of its writer's lease.
    pub epoch: u64,
    /// The segment it follows, by its number and epoch: the last of the branch before it that is
    /// not void. It is void unless that is the one before it that is not.
    pub after: Option<(u64, u64)>,
    /// Its commits, oldest first, by tags its writer gives them, unique to each commit.
    pub commits: Vec<u64>,
    /// The extents sealed in its commits that its data holds: at an offset, of a length.
    pub extents: Vec<(ExtentId, u64, u64)>,
    /// The extents sealed in its commits that are stored on their own, uploaded before it.
    pub own: Vec<(ExtentId, Location)>,
    /// The extents its commits free: the branch's, or inherited.
    pub frees: Vec<ExtentId>,
    /// The root, if its commits set one.
    pub root: Option<Vec<u8>>,
    /// The next sequence number of the branch's extents, as of its last commit.
    pub next_sequence: u64,
    /// Forks of the branch made at its end.
    pub forks: Vec<u64>,
}

/// A segment: its data, then its footer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub data: Vec<u8>,
    pub footer: Footer,
}

/// A branch's record: where it came from, and whether it was deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The branch it was forked from, and the segment of that branch's at whose end.
    pub origin: Option<Origin>,
    /// Whether it is a fork's, which a writer of its parent that is slow to record the fork could
    /// make again were it removed: so a deleted fork's record stays.
    pub forked: bool,
    pub deleted: bool,
}

/// Where a branch was forked from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub parent: u64,
    pub at: u64,
}

/// The holder of a lease, and its epoch: a branch's writer, or the garbage collector. A lease of
/// [`Lease::FINAL`] is a deleted branch's: no writer takes it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub epoch: u64,
    /// Who took it: unique to each taking, so that a write of it is recognised.
    pub holder: u64,
}

impl Lease {
    pub const FINAL: u64 = u64::MAX;
}

/// The footer of a compacted object's moves: where in the moves each extent the object held, live,
/// now is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moves {
    pub extents: Vec<(ExtentId, Location)>,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("objects serialize")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    bincode::deserialize(bytes).map_err(|error| crate::Error::Corrupt(error.to_string()))
}

/// Each this many bytes of an object's data, the last fewer, is a part, with its own CRC32C: what a
/// host fetches at once, and checks.
pub const PART: u64 = 1 << 20;

/// The bytes after an object's CRCs: the lengths of its data and its footer.
pub const TRAILER: u64 = 16;

/// The CRC32C of `bytes`.
pub fn crc(bytes: &[u8]) -> u32 {
    crc_fast::checksum(crc_fast::CrcAlgorithm::Crc32Iscsi, bytes) as u32
}

/// The bytes of an object of `data` and `footer`: the data, the footer, the CRC32Cs of the data's
/// parts, then the [`TRAILER`].
pub fn with_footer<T: Serialize>(data: &[u8], footer: &T) -> Vec<u8> {
    let footer = encode(footer);
    let mut bytes = data.to_vec();
    bytes.extend_from_slice(&footer);
    for part in data.chunks(PART as usize) {
        bytes.extend_from_slice(&crc(part).to_le_bytes());
    }
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(footer.len() as u64).to_le_bytes());
    bytes
}

/// Where the pieces of an object [`with_footer`] made are, as its trailer says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub data_len: u64,
    pub footer_len: u64,
}

impl Frame {
    /// The frame an object's last bytes, its [`TRAILER`] at their end, give.
    pub fn of_trailer(bytes: &[u8]) -> Result<Self, crate::Error> {
        let at = bytes.len().checked_sub(TRAILER as usize).ok_or_else(|| short("its trailer"))?;
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let frame = Self { data_len: u64_at(at), footer_len: u64_at(at + 8) };
        // Lengths so large that the object's would overflow are corrupt.
        frame.data_len.checked_add(frame.footer_len).ok_or_else(|| short("its lengths"))?;
        Ok(frame)
    }

    /// The frame of a whole object.
    pub fn of(bytes: &[u8]) -> Result<Self, crate::Error> {
        let frame = Self::of_trailer(bytes)?;
        if frame.size() != bytes.len() as u64 {
            return Err(short("what its trailer says"));
        }
        Ok(frame)
    }

    pub fn parts(&self) -> u64 {
        self.data_len.div_ceil(PART)
    }

    /// Where part `part` of the data is.
    pub fn part(&self, part: u64) -> std::ops::Range<u64> {
        (part * PART).min(self.data_len)..((part + 1) * PART).min(self.data_len)
    }

    pub fn footer(&self) -> std::ops::Range<u64> {
        self.data_len..self.data_len + self.footer_len
    }

    pub fn crcs(&self) -> std::ops::Range<u64> {
        let at = self.footer().end;
        at..at.saturating_add(4 * self.parts())
    }

    /// The object's length.
    pub fn size(&self) -> u64 {
        self.crcs().end.saturating_add(TRAILER)
    }

    /// The CRCs in `suffix`, the object's last bytes, which must hold them.
    pub fn crcs_in(&self, suffix: &[u8]) -> Result<Vec<u32>, crate::Error> {
        let at = (suffix.len() as u64 + self.crcs().start)
            .checked_sub(self.size())
            .ok_or_else(|| short("its CRCs"))? as usize;
        let crcs = &suffix[at..suffix.len() - TRAILER as usize];
        Ok(crcs.as_chunks::<4>().0.iter().map(|crc| u32::from_le_bytes(*crc)).collect())
    }
}

fn short(what: &str) -> crate::Error {
    crate::Error::Corrupt(format!("an object too short for {what}"))
}

/// The data of a whole object [`with_footer`] made, checked against its CRCs.
pub fn data(bytes: &[u8]) -> Result<&[u8], crate::Error> {
    let frame = Frame::of(bytes)?;
    let data = &bytes[..frame.data_len as usize];
    let crcs = frame.crcs_in(bytes)?;
    for (part, crc_of) in data.chunks(PART as usize).zip(crcs) {
        if crc(part) != crc_of {
            return Err(crate::Error::Corrupt("an object's data, by its CRCs".into()));
        }
    }
    Ok(data)
}

/// The footer of a whole object [`with_footer`] made.
pub fn footer<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    let frame = Frame::of(bytes)?;
    let footer = frame.footer();
    decode(&bytes[footer.start as usize..footer.end as usize])
}

/// The footer of the object [`with_footer`] made at `key`, if there is one: two ranged reads.
pub async fn read_footer<T: DeserializeOwned>(
    bucket: &impl Bucket,
    key: &str,
) -> Result<Option<T>, crate::Error> {
    let Some(trailer) = bucket::get_range(bucket, key, ByteRange::Suffix(TRAILER)).await else {
        return Ok(None);
    };
    let frame = Frame::of_trailer(&trailer)?;
    let Some(footer) = bucket::get_range(bucket, key, ByteRange::Bounded(frame.footer())).await
    else {
        return Ok(None);
    };
    if footer.len() as u64 != frame.footer_len {
        return Err(short("its footer"));
    }
    decode(&footer).map(Some)
}

impl Segment {
    pub fn encode(&self) -> Vec<u8> {
        with_footer(&self.data, &self.footer)
    }

    /// The footer of a segment's object.
    pub fn footer(bytes: &[u8]) -> Result<Footer, crate::Error> {
        footer(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        let id = ExtentId::new(3, 9);
        assert_eq!(kind(&segment(3, 7)), Kind::Segment { branch: 3, seq: 7 });
        assert_eq!(kind(&extent(2, id)), Kind::Extent { branch: 3, epoch: 2, sequence: 9 });
        assert_eq!(kind(&checkpoint(3, 7)), Kind::Checkpoint { branch: 3, seq: 7 });
        assert_eq!(kind(&record(3)), Kind::Record { branch: 3 });
        assert_eq!(kind(&lease(3)), Kind::Lease { branch: 3 });
        assert_eq!(kind(GC_LEASE), Kind::GcLease);
        let from = segment(3, 7);
        assert_eq!(kind(&moves(&from)), Kind::Moves { from: &from });
        assert_eq!(kind("segments/3/7"), Kind::Unknown);
    }

    #[test]
    fn segments_hold_their_footers() {
        let footer = Footer {
            branch: 1,
            seq: 2,
            epoch: 3,
            after: Some((1, 3)),
            commits: vec![4],
            extents: vec![(ExtentId::new(1, 5), 0, 3)],
            own: vec![],
            frees: vec![ExtentId::new(1, 1)],
            root: Some(b"root".to_vec()),
            next_sequence: 6,
            forks: vec![7],
        };
        let segment = Segment { data: b"abc".to_vec(), footer: footer.clone() };
        let bytes = segment.encode();
        assert_eq!(&bytes[..3], b"abc");
        assert_eq!(Segment::footer(&bytes).unwrap(), footer);
        assert!(Segment::footer(&bytes[..bytes.len() - 1]).is_err());
    }

    /// Objects carry a CRC32C for each part of their data, which a ranged read of their end finds.
    #[test]
    fn objects_hold_their_parts_crcs() {
        let contents: Vec<u8> = (0..PART + 7).map(|i| (i % 251) as u8).collect();
        let bytes = with_footer(&contents, &"footer");
        let frame = Frame::of(&bytes).unwrap();
        assert_eq!(frame, Frame { data_len: PART + 7, footer_len: encode(&"footer").len() as u64 });
        assert_eq!(frame.parts(), 2);
        assert_eq!(frame.part(1), PART..PART + 7);
        assert_eq!(frame.size(), bytes.len() as u64);
        let suffix = &bytes[frame.crcs().start as usize - 3..];
        let crcs = vec![crc(&contents[..PART as usize]), crc(&contents[PART as usize..])];
        assert_eq!(frame.crcs_in(suffix).unwrap(), crcs);
        assert_eq!(Frame::of_trailer(&bytes[bytes.len() - 16..]).unwrap(), frame);
        assert!(frame.crcs_in(&bytes[frame.crcs().start as usize + 1..]).is_err());
        assert_eq!(data(&bytes).unwrap(), &contents[..]);
        assert_eq!(footer::<String>(&bytes).unwrap(), "footer");

        let mut flipped = bytes.clone();
        flipped[PART as usize + 3] ^= 1;
        assert_eq!(
            data(&flipped),
            Err(crate::Error::Corrupt("an object's data, by its CRCs".into()))
        );
        assert!(Frame::of(&bytes[1..]).is_err());

        let empty = with_footer(&[], &());
        assert_eq!(Frame::of(&empty).unwrap(), Frame { data_len: 0, footer_len: 0 });
        assert_eq!(empty.len() as u64, TRAILER);
    }
}
