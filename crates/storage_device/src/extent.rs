// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: where a device keeps data, as backlog doc-4's extent store names it.

use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::ops::Range;

/// An extent's ID: the branch that wrote the extent, and the sequence number that branch gave it.
///
/// IDs sort by sequence number, then branch, so that keys holding them sort by when their extents
/// were made, and a tree can seek by the sequence number.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TypeFingerprint,
)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct ExtentId {
    pub branch: u64,
    pub sequence: u64,
}

impl ExtentId {
    pub const fn new(branch: u64, sequence: u64) -> Self {
        Self { branch, sequence }
    }
}

impl Ord for ExtentId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.sequence, self.branch).cmp(&(other.sequence, other.branch))
    }
}

impl PartialOrd for ExtentId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The error of reading an extent the device doesn't hold: one never written, or freed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no extent {0:?}")]
pub struct NoExtent(pub ExtentId);

/// The error of writing an extent that is sealed: written before the last barrier (backlog doc-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("extent {0:?} is sealed")]
pub struct SealedExtent(pub ExtentId);

/// The error of writing an extent of another branch than the device's: one a fork inherited
/// (backlog doc-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("extent {0:?} is another branch's")]
pub struct OtherBranch(pub ExtentId);

/// A range of bytes within an extent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct ExtentRange {
    pub extent: ExtentId,
    pub range: Range<u64>,
}

impl ExtentRange {
    pub fn new(extent: ExtentId, range: Range<u64>) -> Self {
        Self { extent, range }
    }

    /// The range's length in bytes.
    pub fn len(&self) -> u64 {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.start >= self.range.end
    }
}

impl Ord for ExtentRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.extent
            .cmp(&other.extent)
            .then(self.range.start.cmp(&other.range.start))
            .then(self.range.end.cmp(&other.range.end))
    }
}

impl PartialOrd for ExtentRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
