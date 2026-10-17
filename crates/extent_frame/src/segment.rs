// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A segment's footer (doc-7, Segments), the same in a store's closed segments and in object
//! storage, which hold the same bytes: a closed local segment is uploaded as it is.

use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

/// Where an extent's bytes are: a range of the data of an object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    pub key: String,
    pub offset: u64,
    pub len: u64,
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
    /// Its commits, by tags its writer gives them, unique to each commit: a store's segment names
    /// the last of its commits ([`tag`]).
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

impl Footer {
    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("footers serialize")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, crate::Corrupt> {
        bincode::deserialize(bytes).map_err(|err| crate::Corrupt(format!("a footer: {err}")))
    }
}

/// Commits' tags: the writer's epoch above this bit, the store's epoch below, so that no two
/// writers' commits share one.
pub const TAG_SHIFT: u32 = 40;

/// The tag of the store's commit `epoch` that the writer of lease epoch `writer` uploads.
pub fn tag(writer: u64, epoch: u64) -> u64 {
    (writer << TAG_SHIFT) | epoch
}

/// The store's epoch a commit's tag names.
pub fn epoch_of(tag: u64) -> u64 {
    tag & ((1 << TAG_SHIFT) - 1)
}

/// The key of the object of an extent stored on its own, uploaded by the writer of lease epoch
/// `epoch`.
pub fn extent_key(epoch: u64, extent: ExtentId) -> String {
    format!("extents/{:016x}/{epoch:016x}.{:016x}", extent.branch, extent.sequence)
}
