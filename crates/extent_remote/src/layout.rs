// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Where branches keep what, in a bucket, and what the objects hold (doc-7, Layout).
//!
//! Numbers in keys are fixed-width hex, so keys list in numeric order. Objects are read from a
//! bucket, by ranged reads, as [`crate::object`] reads them.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

pub use extent_frame::segment::{Chain, Contents, Footer, Location, epoch_of, tag};

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
    extent_frame::segment::extent_key(epoch, extent)
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

/// A fork of a branch, made at the end of the commits of its segment `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    pub child: u64,
    pub at: u64,
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
    /// The fencing token of the placement that took it: a branch's writer's placement's boot ID,
    /// which a later placement's exceeds, and which no lower one takes the lease from (TASK-39.7).
    /// The collector's is 0.
    pub token: u64,
}

impl Lease {
    pub const FINAL: u64 = u64::MAX;

    /// The lease `bytes` encode: as [`encode`] does, or as leases were before they had fencing
    /// tokens, an epoch and a holder, whose token is 0 (TASK-39.7).
    pub fn decode(bytes: &[u8]) -> Result<Self, crate::Error> {
        if bytes.len() == 16 {
            let (epoch, holder): (u64, u64) = decode(bytes)?;
            return Ok(Self { epoch, holder, token: 0 });
        }
        decode(bytes)
    }
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

pub use extent_frame::{Frame, PART, TRAILER, crc};

/// The bytes of an object of `data` and `footer`, framed (`extent_frame`): the data's parts,
/// compressed, the footer, the table of where the parts are, then the [`TRAILER`].
pub fn with_footer<T: Serialize>(data: &[u8], footer: &T) -> Vec<u8> {
    extent_frame::framed(data, &encode(footer))
}

/// The data of a whole object [`with_footer`] made, checked against its CRCs.
pub fn data(bytes: &[u8]) -> Result<Vec<u8>, crate::Error> {
    Ok(extent_frame::data(bytes)?)
}

/// The footer of a whole object [`with_footer`] made.
pub fn footer<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    decode(extent_frame::footer(bytes)?)
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

    /// A branch's extents stored on their own, whichever writer uploaded them, are under its
    /// prefix, and no other branch's are.
    #[test]
    fn extents_are_under_their_branchs_prefix() {
        assert_eq!(extents(0x12), "extents/0000000000000012/");
        for epoch in [1, 7] {
            assert!(extent(epoch, ExtentId::new(0x12, 5)).starts_with(&extents(0x12)));
        }
        assert!(!extent(1, ExtentId::new(0x13, 5)).starts_with(&extents(0x12)));
    }

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

    /// Leases written before they had fencing tokens decode, with token 0.
    #[test]
    fn leases_decode_as_they_were_written() {
        let lease = Lease { epoch: 4, holder: 9, token: 7 };
        assert_eq!(Lease::decode(&encode(&lease)), Ok(lease));
        let before = encode(&(4u64, 9u64));
        assert_eq!(Lease::decode(&before), Ok(Lease { epoch: 4, holder: 9, token: 0 }));
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

    /// Objects' footers are found by their frames.
    #[test]
    fn objects_hold_their_footers() {
        let bytes = with_footer(&[1; 5000], &"footer");
        assert_eq!(footer::<String>(&bytes).unwrap(), "footer");
        assert_eq!(data(&bytes).unwrap(), &[1; 5000][..]);
        let mut flipped = bytes.clone();
        flipped[7] ^= 1;
        assert_eq!(
            data(&flipped),
            Err(crate::Error::Corrupt("an object's part, by its CRC".into()))
        );
    }
}
