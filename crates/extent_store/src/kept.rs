// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the extents freed whose data stays ([`Kept`]), for the uploader and for views, indexed so
//! that what a commit does with them takes the time of what it lets go of, not of all of them: the
//! uploader keeps every free until it has uploaded the commit, thousands of them behind object
//! storage (TASK-41); nor of what views keep, which a view lagging behind keeps of every commit
//! since its own, counted on as views move on rather than again (TASK-92.29).

use crate::layout::Extent;
use crate::views::Keeping;
use std::cmp::Ordering;
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
    /// The epoch of the oldest view's commit as of the last count of what views keep, and the bytes
    /// of the data of those freed after it, which views keep: counted on from there as the oldest
    /// view moves on, rather than counted again by every commit.
    counted_after: u64,
    after_bytes: u64,
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
        if kept.freed > self.counted_after {
            self.after_bytes += kept.entry.len;
        }
        self.kept.insert(extent, kept);
    }

    /// The uploader needs those the commits up to `epoch` freed no more: returns them. fxdax: so
    /// that what they take of the host's disk counts as the branch's again (TASK-79.89).
    pub fn release(&mut self, epoch: u64) -> Vec<ExtentId> {
        let mut released = Vec::new();
        if epoch <= self.released_to {
            return released;
        }
        let first = (self.released_to + 1, ExtentId::new(0, 0));
        let last = (epoch, ExtentId::new(u64::MAX, u64::MAX));
        for &(freed, extent) in self.by_freed.range(first..=last) {
            let kept = self.kept.get_mut(&extent).unwrap();
            if kept.uploader {
                kept.uploader = false;
                self.released.insert((freed, extent));
                released.push(extent);
            }
        }
        self.released_to = epoch;
        released
    }

    /// Takes out those the commits up to `published` freed that the uploader needs no more, and
    /// views don't keep, as `keeping` says, and returns them; and counts what views keep. Those
    /// freed after the oldest view's commit views keep, which are not looked at again.
    pub fn trim(&mut self, keeping: &Keeping, published: u64) -> Vec<(ExtentId, Kept)> {
        let upto = keeping.keeps_after().map_or(published, |oldest| oldest.min(published));
        let last = (upto, ExtentId::new(u64::MAX, u64::MAX));
        let gone: Vec<ExtentId> = (self.released.range(..=last))
            .filter(|(_, extent)| !Self::views_keep(keeping, *extent, &self.kept[extent]))
            .map(|(_, extent)| *extent)
            .collect();
        let trimmed = (gone.into_iter())
            .map(|extent| {
                let kept = self.kept.remove(&extent).unwrap();
                self.unindex(extent, &kept);
                (extent, kept)
            })
            .collect();
        self.for_views = self.views_bytes(keeping);
        trimmed
    }

    /// The bytes of the data of those views keep, as `keeping` says: those freed after the oldest
    /// view's commit, and those they pin.
    pub fn views_bytes(&mut self, keeping: &Keeping) -> u64 {
        let after = keeping.keeps_after();
        let after_bytes = after.map_or(0, |oldest| self.bytes_after(oldest));
        // Those pinned it doesn't count already.
        let pinned: u64 = (keeping.pinned())
            .filter_map(|extent| self.kept.get(&extent))
            .filter(|kept| after.is_none_or(|oldest| kept.freed <= oldest))
            .map(|kept| kept.entry.len)
            .sum();
        after_bytes + pinned
    }

    /// The bytes of the data of those freed after commit `epoch`: counted on from the last count,
    /// over the frees between the two epochs, as the oldest view moves on, or one opens when none
    /// lived.
    fn bytes_after(&mut self, epoch: u64) -> u64 {
        let between = |from: u64, to: u64| {
            (from + 1, ExtentId::new(0, 0))..=(to, ExtentId::new(u64::MAX, u64::MAX))
        };
        let bytes = |range| -> u64 {
            let extents = self.by_freed.range(range);
            extents.map(|(_, extent)| self.kept[extent].entry.len).sum()
        };
        match epoch.cmp(&self.counted_after) {
            Ordering::Greater => self.after_bytes -= bytes(between(self.counted_after, epoch)),
            Ordering::Less => self.after_bytes += bytes(between(epoch, self.counted_after)),
            Ordering::Equal => {}
        }
        self.counted_after = epoch;
        self.after_bytes
    }

    /// Whether views keep `extent`, kept as `kept` says.
    fn views_keep(keeping: &Keeping, extent: ExtentId, kept: &Kept) -> bool {
        keeping.keeps(extent, kept.freed)
    }

    /// `extent`, kept as `kept` says, is kept no more.
    fn unindex(&mut self, extent: ExtentId, kept: &Kept) {
        self.by_freed.remove(&(kept.freed, extent));
        self.released.remove(&(kept.freed, extent));
        self.bytes -= kept.entry.len;
        if kept.freed > self.counted_after {
            self.after_bytes -= kept.entry.len;
        }
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

    /// An extent kept again is kept as the later free says: its bytes and its commit counted once.
    #[test]
    fn extents_kept_again_count_once() {
        let mut extents = KeptExtents::default();
        extents.insert(extent(1), freed(1, false, 4096));
        extents.insert(extent(1), freed(2, false, 8192));
        assert_eq!(extents.bytes(), 8192);
        // A view as of commit 1 keeps what commit 2 freed.
        assert_eq!(trimmed(&mut extents, &Keeping::of(Some(1), []), 2), []);
        assert_eq!(extents.views_bytes(&Keeping::of(Some(1), [])), 8192);
        assert_eq!(trimmed(&mut extents, &Keeping::none(), 2), [extent(1)]);
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

    /// What views keep, counted on from the last count as the oldest view moves on, or one opens
    /// when none lived, is what counting it again gives: the frees after the oldest's commit, and
    /// those pinned, as the uploader releases its commits and trims let the rest go (TASK-92.29).
    #[test]
    fn what_views_keep_is_counted_on_as_they_move_on() {
        let mut extents = KeptExtents::default();
        let again = |extents: &KeptExtents, keeping: &Keeping| -> u64 {
            (extents.kept.iter())
                .filter(|(extent, kept)| keeping.keeps(**extent, kept.freed))
                .map(|(_, kept)| kept.entry.len)
                .sum()
        };
        for epoch in 1..=40 {
            extents.insert(extent(epoch), freed(epoch, epoch % 3 == 0, epoch << 12));
            if epoch % 4 == 0 {
                extents.release(epoch - 2);
            }
            let oldest = match epoch {
                1..=12 => Some(epoch / 2),
                13..=20 => None,
                _ => Some(epoch - 5),
            };
            let keeping = Keeping::of(oldest, [extent(2), extent(30)]);
            extents.trim(&keeping, epoch);
            assert_eq!(extents.for_views(), again(&extents, &keeping), "at {epoch}");
        }
    }
}
