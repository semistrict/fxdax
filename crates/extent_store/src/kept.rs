// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the extents freed whose data stays ([`Kept`]), for the uploader and for views, indexed so
//! that what a commit does with them takes the time of what it lets go of and of what views keep,
//! not of all of them: the uploader keeps every free until it has uploaded the commit, thousands of
//! them behind object storage (TASK-41).

use crate::layout::Extent;
use crate::views::Keeping;
use std::collections::{BTreeSet, HashMap};
use storage_device::ExtentId;

/// An extent freed whose data stays, sealed: for the uploader to read, until it releases the
/// commit that freed it, and for views older than that commit, or pinning it ([`Keeping`]). A
/// free's is kept until the commit that frees it decides.
pub(crate) struct Kept {
    pub entry: Extent,
    /// The epoch of the commit that freed it.
    pub freed: u64,
    /// Whether the uploader still needs it.
    pub uploader: bool,
}

/// The extents kept, by ID, and by the commits that freed them.
#[derive(Default)]
pub(crate) struct KeptExtents {
    kept: HashMap<ExtentId, Kept>,
    /// All of them, by the epoch of the commit that freed each.
    by_freed: BTreeSet<(u64, ExtentId)>,
    /// Those the uploader doesn't need, by the epoch of the commit that freed each: those a trim
    /// may let go of, but for those views keep.
    released: BTreeSet<(u64, ExtentId)>,
    /// The latest commit the uploader released, whose frees and those before it it needs no more.
    released_to: u64,
    /// The bytes of their data.
    bytes: u64,
    /// The bytes of the data of those views keep, as of the last trim, which count as used.
    for_views: u64,
}

impl KeptExtents {
    pub fn get(&self, extent: &ExtentId) -> Option<&Kept> {
        self.kept.get(extent)
    }

    /// The bytes of the data of the extents kept.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The bytes of the data of those views kept as of the last trim.
    pub fn for_views(&self) -> u64 {
        self.for_views
    }

    /// Keeps `extent`, freed as `kept` says.
    pub fn insert(&mut self, extent: ExtentId, kept: Kept) {
        debug_assert!(kept.freed > self.released_to, "a free after the commits released");
        if let Some(old) = self.kept.remove(&extent) {
            self.unindex(extent, &old);
        }
        self.by_freed.insert((kept.freed, extent));
        if !kept.uploader {
            self.released.insert((kept.freed, extent));
        }
        self.bytes += kept.entry.len;
        self.kept.insert(extent, kept);
    }

    /// The uploader needs those the commits up to `epoch` freed no more.
    pub fn release(&mut self, epoch: u64) {
        if epoch <= self.released_to {
            return;
        }
        let first = (self.released_to + 1, ExtentId::new(0, 0));
        let last = (epoch, ExtentId::new(u64::MAX, u64::MAX));
        for &(freed, extent) in self.by_freed.range(first..=last) {
            let kept = self.kept.get_mut(&extent).unwrap();
            if kept.uploader {
                kept.uploader = false;
                self.released.insert((freed, extent));
            }
        }
        self.released_to = epoch;
    }

    /// Takes out those the commits up to `published` freed that the uploader needs no more, and
    /// views don't keep, as `keeping` says, and returns them; and counts what views keep.
    pub fn trim(&mut self, keeping: &Keeping, published: u64) -> Vec<(ExtentId, Kept)> {
        let last = (published, ExtentId::new(u64::MAX, u64::MAX));
        let gone: Vec<(u64, ExtentId)> = (self.released.range(..=last))
            .filter(|(_, extent)| !Self::views_keep(keeping, *extent, &self.kept[extent]))
            .copied()
            .collect();
        let trimmed = (gone.into_iter())
            .map(|(freed, extent)| {
                self.released.remove(&(freed, extent));
                self.by_freed.remove(&(freed, extent));
                let kept = self.kept.remove(&extent).unwrap();
                self.bytes -= kept.entry.len;
                (extent, kept)
            })
            .collect();
        self.for_views = self.views_bytes(keeping);
        trimmed
    }

    /// The bytes of the data of those views keep, as `keeping` says: those freed after the oldest
    /// view's commit, and those they pin.
    pub fn views_bytes(&self, keeping: &Keeping) -> u64 {
        let after_oldest = keeping.oldest().into_iter().flat_map(|oldest| {
            let first = (oldest + 1, ExtentId::new(0, 0));
            self.by_freed.range(first..).map(|(_, extent)| *extent)
        });
        // Those pinned it doesn't count already.
        let pinned = keeping.pins().filter(|extent| {
            self.kept
                .get(extent)
                .is_some_and(|kept| keeping.oldest().is_none_or(|oldest| kept.freed <= oldest))
        });
        (after_oldest.chain(pinned))
            .map(|extent| (extent, &self.kept[&extent]))
            .filter(|(extent, kept)| Self::views_keep(keeping, *extent, kept))
            .map(|(_, kept)| kept.entry.len)
            .sum()
    }

    /// Whether views keep `extent`, kept as `kept` says.
    fn views_keep(keeping: &Keeping, extent: ExtentId, kept: &Kept) -> bool {
        keeping.keeps(extent, kept.freed)
    }

    fn unindex(&mut self, extent: ExtentId, kept: &Kept) {
        self.by_freed.remove(&(kept.freed, extent));
        self.released.remove(&(kept.freed, extent));
        self.bytes -= kept.entry.len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Data;

    fn freed(freed: u64, uploader: bool, len: u64) -> Kept {
        Kept { entry: Extent { len, data: Data::Remote }, freed, uploader }
    }

    fn extent(sequence: u64) -> ExtentId {
        ExtentId::new(1, sequence)
    }

    fn trimmed(extents: &mut KeptExtents, keeping: &Keeping, published: u64) -> Vec<ExtentId> {
        extents.trim(keeping, published).into_iter().map(|(extent, _)| extent).collect()
    }

    /// The uploader's frees stay until it releases their commits; then those up to the commit
    /// published go.
    #[test]
    fn frees_go_once_released_and_published() {
        let mut extents = KeptExtents::default();
        let none = Keeping::none();
        extents.insert(extent(1), freed(1, true, 4096));
        extents.insert(extent(2), freed(2, true, 4096));
        extents.insert(extent(3), freed(3, true, 4096));
        assert_eq!(trimmed(&mut extents, &none, 3), []);
        extents.release(2);
        assert_eq!(trimmed(&mut extents, &none, 1), [extent(1)]);
        assert_eq!(trimmed(&mut extents, &none, 3), [extent(2)]);
        assert_eq!(extents.bytes(), 4096);
        assert!(extents.get(&extent(3)).is_some_and(|kept| kept.uploader));
        extents.release(3);
        assert_eq!(trimmed(&mut extents, &none, 3), [extent(3)]);
        assert_eq!(extents.bytes(), 0);
    }

    /// Views keep the frees after the oldest's commit, and those they pin, whenever freed.
    #[test]
    fn views_keep_frees_after_the_oldest_and_pins() {
        let mut extents = KeptExtents::default();
        for sequence in 1..=4 {
            extents.insert(extent(sequence), freed(sequence, false, sequence << 12));
        }
        let keeping = Keeping::of(Some(2), [extent(1)]);
        assert_eq!(extents.views_bytes(&keeping), (1 + 3 + 4) << 12);
        assert_eq!(trimmed(&mut extents, &keeping, 4), [extent(2)]);
        assert_eq!(extents.views_bytes(&keeping), (1 + 3 + 4) << 12);
        assert_eq!(extents.bytes(), (1 + 3 + 4) << 12);
        // Once the views go, what they kept does.
        assert_eq!(trimmed(&mut extents, &Keeping::none(), 4), [extent(1), extent(3), extent(4)]);
        assert_eq!(extents.views_bytes(&Keeping::none()), 0);
    }

    /// A trim counts what views keep, which stays counted until the next, though frees come
    /// meanwhile.
    #[test]
    fn trims_count_what_views_keep() {
        let mut extents = KeptExtents::default();
        extents.insert(extent(1), freed(2, false, 1 << 12));
        extents.insert(extent(2), freed(3, false, 2 << 12));
        assert_eq!(extents.for_views(), 0);
        let keeping = Keeping::of(Some(2), []);
        assert_eq!(trimmed(&mut extents, &keeping, 3), [extent(1)]);
        assert_eq!(extents.for_views(), 2 << 12);
        extents.insert(extent(3), freed(4, false, 4 << 12));
        assert_eq!(extents.for_views(), 2 << 12);
        assert_eq!(trimmed(&mut extents, &keeping, 3), []);
        assert_eq!(extents.for_views(), (2 + 4) << 12);
        assert_eq!(trimmed(&mut extents, &Keeping::none(), 4), [extent(2), extent(3)]);
        assert_eq!(extents.for_views(), 0);
    }
}
