// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of which of a VMO's mappings its range changes reach (TASK-92.50).

use crate::vm::pmm;
use crate::vm::vm_cow_pages::{
    RangeChangeListener, RangeChangeOp, VmCowPages, VmCowPagesLocked, VmCowRange,
};
use parking_lot::Mutex;
use std::sync::Arc;

/// The range of the `pages` pages of a VMO from its page `page`.
fn pages(page: usize, pages: usize) -> VmCowRange {
    VmCowRange::new((page * page::SIZE) as u64, (pages * page::SIZE) as u64)
}

/// A mapping of a range of a VMO, which records the range changes that reach it.
struct Mapping {
    range: VmCowRange,
    changes: Mutex<Vec<(VmCowRange, RangeChangeOp)>>,
}

impl Mapping {
    fn new(range: VmCowRange) -> Arc<Self> {
        Arc::new(Self { range, changes: Mutex::default() })
    }
}

impl RangeChangeListener for Mapping {
    fn object_range(&self) -> VmCowRange {
        self.range
    }

    fn range_change_update(
        &self,
        _locked: &VmCowPagesLocked,
        range: VmCowRange,
        op: RangeChangeOp,
    ) {
        self.changes.lock().push((range, op));
    }
}

/// A VMO's range changes reach the mappings of part of the range changed, each once, and no
/// others, among 30k mappings of a page each, one for each page, and a few more: a second of a
/// page, one of four pages past them, which a change of any of its pages reaches, and one of all
/// the 30k, which a change of any of theirs reaches, though it starts before it. A mapping removed
/// is reached no more. The port's walk of every mapping of the VMO reached them all, as each page
/// `map_open` commits reached every mapping a DAX window had of the extent's VMO. Counted are the
/// mappings a change reaches, not what the walk looks at to find them, which a walk of every one
/// of them that skipped those of other ranges would match: the tree's tests (`vm_mapping_tree`)
/// count that.
#[test]
fn range_changes_reach_the_mappings_of_the_range_alone() {
    const PAGES: usize = 30_000;
    const FEW: usize = 1_000;
    // The mappings past those of a page each: a second of the page a third of the way through
    // them, one of four pages past them, and one of all of them.
    const TWICE: usize = PAGES;
    const FOUR: usize = PAGES + 1;
    const ALL: usize = PAGES + 2;
    let cow = VmCowPages::create(0, pmm::ALLOC_FLAG_ANY, pages(0, PAGES + 8).len);
    let mut mappings: Vec<Arc<Mapping>> =
        (0..PAGES).map(|page| Mapping::new(pages(page, 1))).collect();
    mappings.extend([pages(PAGES / 3, 1), pages(PAGES, 4), pages(0, PAGES)].map(Mapping::new));
    let listener = |n: usize| -> Arc<dyn RangeChangeListener> { mappings[n].clone() };
    for n in 0..mappings.len() {
        cow.lock().add_mapping(listener(n));
    }
    // Changes `range`, and asserts that the change reached the mappings `reached`, by their
    // indices, each once, and no others.
    let assert_reaches = |range: VmCowRange, reached: &[usize]| {
        let op = RangeChangeOp::UnmapZeroPage;
        cow.range_change_update_locked(&mut cow.lock(), range, op, None);
        let changes = mappings.iter().map(|mapping| std::mem::take(&mut *mapping.changes.lock()));
        let changed: Vec<_> =
            changes.enumerate().filter(|(_, changes)| !changes.is_empty()).collect();
        let once: Vec<_> = reached.iter().map(|&n| (n, vec![(range, op)])).collect();
        assert_eq!(changed, once, "{range:?}");
    };

    assert_reaches(pages(PAGES / 2, 1), &[PAGES / 2, ALL]);
    assert_reaches(pages(0, 1), &[0, ALL]);
    assert_reaches(pages(PAGES - 1, 1), &[PAGES - 1, ALL]);
    assert_reaches(pages(0, FEW), &(0..FEW).chain([ALL]).collect::<Vec<_>>());
    assert_reaches(pages(PAGES / 3, 1), &[PAGES / 3, TWICE, ALL]);
    assert_reaches(pages(PAGES + 2, 1), &[FOUR]);
    assert_reaches(pages(PAGES + 4, 1), &[]);
    assert_reaches(pages(PAGES - 1, 2), &[PAGES - 1, FOUR, ALL]);

    cow.lock().remove_mapping(&listener(TWICE));
    assert_reaches(pages(PAGES / 3, 1), &[PAGES / 3, ALL]);
    cow.lock().remove_mapping(&listener(ALL));
    assert_reaches(pages(PAGES / 3, 1), &[PAGES / 3]);
    assert_eq!(cow.lock().num_mappings(), mappings.len() - 2);
}
