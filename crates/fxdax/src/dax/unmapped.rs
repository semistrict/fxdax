// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax (TASK-91.16): the pages a second stage in another process, a VM process, still maps
//! though the kernel unmapped them, as a thread deferring its waits had it do
//! ([`SecondStage::unmap_pages`](super::SecondStage::unmap_pages)): what it is told of them once
//! the kernel maps a page at one of their offsets, by any thread, or once the deferring ends.
//!
//! A commit's seal remaps an open extent's pages read-only, the same physical pages, which the
//! sealed extent takes: told of the unmap and the map apart, the VM process replaces its memory
//! twice, each change a flush of the host's TLBs on every CPU running the VM, and of KVM's
//! mappings. Told of them together, it makes the page read-only in place.
//!
//! The changes a stage sends match the kernel's page tables only if what it decides from these
//! pages is decided, and sent, in the order the kernel changed its page tables: the stage keeps
//! them per window, not per thread, and decides from them and sends what it decides holding one
//! lock. A fault served on another thread that maps one of these offsets so has the VM process
//! unmap the page first, or reprotect it; otherwise its map reaches the VM process before the
//! unmap, which then takes away the page the kernel maps, whose faults then find it mapped and
//! change nothing, for good.

use crate::Protection;
use crate::kernel::{PhysLocation, page_size};
use std::collections::BTreeMap;

/// A change to a second stage, as one in another process is told of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Map { offset: u64, phys: PhysLocation, len: u64, protection: Protection },
    Protect { offset: u64, len: u64, protection: Protection },
    Unmap { offset: u64, len: u64 },
}

impl Change {
    /// Whether it takes access to a page away, which the guest may still have until it is made.
    pub fn takes_access(&self) -> bool {
        match self {
            Change::Map { .. } => false,
            Change::Protect { protection, .. } => *protection != Protection::ReadWrite,
            Change::Unmap { .. } => true,
        }
    }
}

/// The pages unmapped that the VM process has yet to be told of, by their offsets in the window,
/// and the physical page each was.
#[derive(Default)]
pub(crate) struct Unmapped(BTreeMap<u64, PhysLocation>);

impl Unmapped {
    /// Records the pages at `offset` on, one a page of `phys`, as unmapped.
    pub fn unmapped(&mut self, offset: u64, phys: &[PhysLocation]) {
        let ps = page_size();
        for (at, page) in phys.iter().enumerate() {
            self.0.insert(offset + at as u64 * ps, *page);
        }
    }

    /// The changes that map the `len` bytes at `offset` to the physical pages from `phys` on,
    /// with `protection`, given the pages unmapped there: a page made read-only where it was, the
    /// same page, has its protection changed alone; the others are unmapped first. A page made
    /// writable is mapped anew, as the VM process's memory may have been restricted since it was
    /// mapped (`dax::uffd`'s `restrict`), which mapping it anew alone undoes.
    pub fn map(
        &mut self,
        offset: u64,
        phys: PhysLocation,
        len: u64,
        protection: Protection,
    ) -> Vec<Change> {
        let map = |offset, phys, len| Change::Map { offset, phys, len, protection };
        if self.0.range(offset..offset + len).next().is_none() {
            return vec![map(offset, phys, len)];
        }
        let ps = page_size();
        let page = |at: u64| PhysLocation { arena: phys.arena, offset: phys.offset + at * ps };
        let mut unmaps = Vec::new();
        let mut again = Vec::new();
        for at in 0..len / ps {
            let was = self.0.remove(&(offset + at * ps));
            let same = protection == Protection::Read && was == Some(page(at));
            if was.is_some() && !same {
                unmaps.push(offset + at * ps);
            }
            again.push(same);
        }
        let mut changes: Vec<Change> =
            runs(unmaps).into_iter().map(|(offset, len)| Change::Unmap { offset, len }).collect();
        let mut at = 0;
        while at < again.len() {
            let same = again[at];
            let count = again[at..].iter().take_while(|again| **again == same).count();
            let (start, run) = (offset + at as u64 * ps, count as u64 * ps);
            changes.push(if same {
                Change::Protect { offset: start, len: run, protection }
            } else {
                map(start, page(at as u64), run)
            });
            at += count;
        }
        changes
    }

    /// The unmaps of every page left, which nothing mapped again: the pages taken.
    pub fn take(&mut self) -> Vec<Change> {
        self.take_within(0..u64::MAX)
    }

    /// The unmaps of the pages left at offsets within `range`, taken: what the VM process is told
    /// of before another change there, which the kernel makes of what it maps alone.
    pub fn take_within(&mut self, range: std::ops::Range<u64>) -> Vec<Change> {
        let pages: Vec<u64> = self.0.range(range).map(|(at, _)| *at).collect();
        for at in &pages {
            self.0.remove(at);
        }
        runs(pages).into_iter().map(|(offset, len)| Change::Unmap { offset, len }).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `pages`, the offsets of pages in order, in runs of consecutive pages: their offsets and
/// lengths.
fn runs(pages: impl IntoIterator<Item = u64>) -> Vec<(u64, u64)> {
    let ps = page_size();
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for at in pages {
        match runs.last_mut() {
            Some((start, len)) if *start + *len == at => *len += ps,
            _ => runs.push((at, ps)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ArenaId;

    fn phys(page: u64) -> PhysLocation {
        PhysLocation { arena: ArenaId::DEFAULT, offset: page * page_size() }
    }

    /// A page unmapped and mapped read-only where it was, the same page, has its protection
    /// changed alone; another page mapped there has the first unmapped before.
    #[test]
    fn pages_mapped_read_only_again_where_they_were_change_their_protection() {
        let ps = page_size();
        let mut unmapped = Unmapped::default();
        unmapped.unmapped(0, &[phys(10), phys(11), phys(12), phys(13)]);
        assert_eq!(
            unmapped.map(0, phys(10), 2 * ps, Protection::Read),
            [Change::Protect { offset: 0, len: 2 * ps, protection: Protection::Read }]
        );
        assert_eq!(
            unmapped.map(2 * ps, phys(20), ps, Protection::Read),
            [
                Change::Unmap { offset: 2 * ps, len: ps },
                Change::Map { offset: 2 * ps, phys: phys(20), len: ps, protection: Protection::Read },
            ]
        );
        assert_eq!(unmapped.take(), [Change::Unmap { offset: 3 * ps, len: ps }]);
        assert!(unmapped.is_empty(), "pages left once taken");
    }

    /// A page unmapped and mapped writable where it was, the same page, is unmapped and mapped
    /// anew: changing its protection alone would leave a restriction of the VM process's memory
    /// in place, which a store then faults on for good.
    #[test]
    fn pages_mapped_writable_again_are_mapped_anew() {
        let ps = page_size();
        let mut unmapped = Unmapped::default();
        unmapped.unmapped(ps, &[phys(11)]);
        assert_eq!(
            unmapped.map(0, phys(10), 3 * ps, Protection::ReadWrite),
            [
                Change::Unmap { offset: ps, len: ps },
                Change::Map {
                    offset: 0,
                    phys: phys(10),
                    len: 3 * ps,
                    protection: Protection::ReadWrite
                },
            ]
        );
        assert!(unmapped.is_empty(), "the page mapped again left unmapped");
    }

    /// Of the pages a mapping maps where pages were unmapped, those it maps again read-only, the
    /// same, change their protection, and the others are mapped, in runs, as they come.
    #[test]
    fn mappings_over_pages_unmapped_change_those_they_map_again() {
        let ps = page_size();
        let mut unmapped = Unmapped::default();
        unmapped.unmapped(ps, &[phys(11)]);
        assert_eq!(
            unmapped.map(0, phys(10), 3 * ps, Protection::Read),
            [
                Change::Map { offset: 0, phys: phys(10), len: ps, protection: Protection::Read },
                Change::Protect { offset: ps, len: ps, protection: Protection::Read },
                Change::Map { offset: 2 * ps, phys: phys(12), len: ps, protection: Protection::Read },
            ]
        );
    }
}
