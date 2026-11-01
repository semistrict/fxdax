// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: where a device keeps data, as backlog doc-4's extent store names it.

use fprint::TypeFingerprint;
use fuchsia_sync::Mutex;
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

/// What a device knows to be open: the extents its filesystem minted since the last seal. An extent
/// goes minted, open, sealed, and never back (TASK-153.3). A write opens nothing: one sent after a
/// seal to an extent minted before it, which no request reached, finds a store that takes it, as
/// the seal sealed nothing of it there, and an extent read as sealed read as open again, for a
/// reader and a writer to meet on (TASK-99, TASK-140), or before a refusal (TASK-91.9).
///
/// A filesystem's sequence numbers only grow, so those open are a range, which a seal empties, and
/// which starts no earlier than it did: there is no way to have a sealed extent read as open.
#[derive(Debug, Default)]
pub struct OpenExtents(Mutex<Minted>);

/// The sequence numbers `first..next` of `branch` minted since the last seal.
#[derive(Debug, Default)]
struct Minted {
    branch: u64,
    first: u64,
    next: u64,
}

impl OpenExtents {
    /// Records that `extent` was just minted. One no newer than the last of its branch is as it
    /// was, sealed if it is. A branch takes over from another as a store forks, sealed.
    pub fn minted(&self, extent: ExtentId) {
        let mut minted = self.0.lock();
        if extent.branch != minted.branch {
            *minted = Minted { branch: extent.branch, first: extent.sequence, next: 0 };
        }
        if extent.sequence >= minted.next {
            if minted.first == minted.next {
                minted.first = extent.sequence;
            }
            minted.next = extent.sequence + 1;
        }
    }

    /// A seal: none of those minted so far is open from now on.
    pub fn seal(&self) {
        let mut minted = self.0.lock();
        minted.first = minted.next;
    }

    /// Whether `extent` is open: minted, and no seal since.
    pub fn contains(&self, extent: ExtentId) -> bool {
        let minted = self.0.lock();
        extent.branch == minted.branch && (minted.first..minted.next).contains(&extent.sequence)
    }
}

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

#[cfg(test)]
mod tests {
    use super::{ExtentId, OpenExtents};

    /// Minted, open, sealed, and never back, whatever says it was minted again.
    #[test]
    fn test_open_extents_go_one_way() {
        let open = OpenExtents::default();
        let (a, b, c) = (ExtentId::new(1, 5), ExtentId::new(1, 6), ExtentId::new(1, 7));
        assert!(!open.contains(a));
        open.minted(a);
        open.minted(b);
        assert!(open.contains(a) && open.contains(b));
        assert!(!open.contains(c));
        open.seal();
        assert!(!open.contains(a) && !open.contains(b));
        open.minted(a);
        assert!(!open.contains(a));
        open.minted(c);
        assert!(open.contains(c));
        assert!(!open.contains(b));
    }

    /// Sequence numbers are a branch's: another's isn't open for having one in the range.
    #[test]
    fn test_open_extents_are_of_one_branch() {
        let open = OpenExtents::default();
        open.minted(ExtentId::new(1, 5));
        assert!(!open.contains(ExtentId::new(2, 5)));
        open.seal();
        open.minted(ExtentId::new(2, 9));
        assert!(open.contains(ExtentId::new(2, 9)));
        assert!(!open.contains(ExtentId::new(2, 8)));
        assert!(!open.contains(ExtentId::new(1, 5)));
    }
}
