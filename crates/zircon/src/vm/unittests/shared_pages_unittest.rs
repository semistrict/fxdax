// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of pages shared between pager-backed VMOs (`VmCowPages::lend_pages`), which Zircon lacks.

use super::test_helper::{make_committed_pager_vmo, make_partially_committed_pager_vmo};
use crate::vm::page::VmPagePtr;
use crate::vm::page_queues::PageQueues;
use crate::vm::pmm;
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_cow_pages::EvictionAction;
use crate::vm::vm_object_paged::VmObjectPaged;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: u64 = page::SIZE as u64;

/// Returns a VMO of `num_pages` clean pages, page `i` filled with `i + 1`.
fn make_lender(num_pages: u64) -> Arc<VmObjectPaged> {
    let (lender, _) = make_committed_pager_vmo(num_pages as usize, false, true).unwrap();
    for i in 0..num_pages {
        lender.write(&[i as u8 + 1; page::SIZE], i * PAGE_SIZE).unwrap();
    }
    let len = num_pages * PAGE_SIZE;
    lender.writeback_begin(0, len, false).unwrap();
    lender.writeback_end(0, len).unwrap();
    lender
}

/// Returns a VMO of `num_pages` with no pages, whose page source must not be asked for any.
fn make_borrower(num_pages: u64, trap_dirty: bool) -> Arc<VmObjectPaged> {
    let (borrower, _) =
        make_partially_committed_pager_vmo(num_pages as usize, 0, trap_dirty, true, false).unwrap();
    borrower
}

/// Ages the page queues until `pages` can be reclaimed.
fn make_reclaimable(pages: &[VmPagePtr]) {
    let page_queues = pmm::node().page_queues();
    for _ in 0..100 {
        if pages.iter().all(|page| PageQueues::is_page_reclaimable(*page)) {
            return;
        }
        page_queues.rotate_reclaim_queues();
        drop(page_queues.peek_isolate(0));
    }
    panic!("pages did not age");
}

fn read_page(vmo: &VmObjectPaged, index: u64) -> Vec<u8> {
    let mut buf = vec![0; page::SIZE];
    vmo.read(&mut buf, index * PAGE_SIZE).unwrap();
    buf
}

#[test]
fn shared_pages_are_the_lenders() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    for i in 0..2 {
        assert_eq!(borrower.debug_get_page(i * PAGE_SIZE), lender.debug_get_page(i * PAGE_SIZE));
        assert_eq!(read_page(&borrower, i), vec![i as u8 + 1; page::SIZE]);
    }
    assert_eq!(lender.committed_bytes(), 2 * PAGE_SIZE);
    assert_eq!(borrower.committed_bytes(), 0);
    assert_eq!(borrower.borrowed_bytes(), 2 * PAGE_SIZE);
    assert_eq!(lender.cow_pages().debug_loan_count(), 2);
}

#[test]
fn shared_pages_at_an_offset() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(4);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(PAGE_SIZE, PAGE_SIZE, &lender, 2 * PAGE_SIZE).unwrap();

    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), lender.debug_get_page(2 * PAGE_SIZE));
    assert_eq!(read_page(&borrower, 1), vec![3; page::SIZE]);
}

#[test]
fn shared_pages_only_fill_absent_content() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let first = make_lender(1);
    let borrower = make_borrower(2, false);
    // The borrower's first page comes from another lender, so only the second is lent.
    borrower.supply_shared_pages(0, PAGE_SIZE, &first, 0).unwrap();
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    assert_eq!(borrower.debug_get_page(0), first.debug_get_page(0));
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), lender.debug_get_page(PAGE_SIZE));
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);
}

#[test]
fn sharing_needs_every_page_present_and_clean() {
    let _scanner = AutoVmScannerDisable::new();
    let (lender, _) = make_partially_committed_pager_vmo(2, 1, false, true, false).unwrap();
    let borrower = make_borrower(2, false);
    assert_eq!(borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0), Err(Status::NOT_FOUND));

    let dirty = make_lender(1);
    dirty.write(&[9; 1], 0).unwrap();
    assert_eq!(borrower.supply_shared_pages(0, PAGE_SIZE, &dirty, 0), Err(Status::NOT_FOUND));
    assert_eq!(borrower.borrowed_bytes(), 0);
}

#[test]
fn sharing_checks_its_arguments() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    assert_eq!(
        borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, PAGE_SIZE),
        Err(Status::OUT_OF_RANGE)
    );
    assert_eq!(
        borrower.supply_shared_pages(PAGE_SIZE, 2 * PAGE_SIZE, &lender, 0),
        Err(Status::OUT_OF_RANGE)
    );
    assert_eq!(lender.supply_shared_pages(0, PAGE_SIZE, &lender, 0), Err(Status::INVALID_ARGS));
    let anonymous = VmObjectPaged::create(0, 0, PAGE_SIZE).unwrap();
    assert_eq!(
        borrower.supply_shared_pages(0, PAGE_SIZE, &anonymous, 0),
        Err(Status::NOT_SUPPORTED)
    );
}

#[test]
fn a_vmo_only_lends_or_borrows() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    let other = make_lender(1);
    assert_eq!(other.supply_shared_pages(0, PAGE_SIZE, &borrower, 0), Err(Status::BAD_STATE));
    let other_borrower = make_borrower(1, false);
    assert_eq!(
        lender.supply_shared_pages(0, PAGE_SIZE, &other_borrower, 0),
        Err(Status::BAD_STATE)
    );
}

#[test]
fn writing_a_shared_page_copies_it() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    borrower.write(&[7; 3], 1).unwrap();

    assert_ne!(borrower.debug_get_page(0), lender.debug_get_page(0));
    let mut expected = vec![1; page::SIZE];
    expected[1..4].fill(7);
    assert_eq!(read_page(&borrower, 0), expected);
    assert_eq!(read_page(&lender, 0), vec![1; page::SIZE]);
    // The other page is still shared.
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), lender.debug_get_page(PAGE_SIZE));
    assert_eq!(borrower.committed_bytes(), PAGE_SIZE);
    assert_eq!(borrower.borrowed_bytes(), PAGE_SIZE);
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);
}

#[test]
fn dirtying_shared_pages_copies_them() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(3);
    let borrower = make_borrower(3, true);
    borrower.supply_shared_pages(0, 3 * PAGE_SIZE, &lender, 0).unwrap();

    // As the pager does in response to a DIRTY request.
    borrower.dirty_pages(0, 2 * PAGE_SIZE).unwrap();

    for i in 0..2 {
        assert_ne!(borrower.debug_get_page(i * PAGE_SIZE), lender.debug_get_page(i * PAGE_SIZE));
        assert_eq!(read_page(&borrower, i), vec![i as u8 + 1; page::SIZE]);
    }
    assert_eq!(borrower.debug_get_page(2 * PAGE_SIZE), lender.debug_get_page(2 * PAGE_SIZE));
    assert_eq!(borrower.committed_bytes(), 2 * PAGE_SIZE);
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);

    // The copies are dirty, so the write needs no DIRTY request.
    borrower.write(&[7; 1], PAGE_SIZE).unwrap();
    assert_eq!(read_page(&lender, 1), vec![2; page::SIZE]);
}

#[test]
fn pinning_a_shared_page_copies_it() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    borrower.commit_range_pinned(0, PAGE_SIZE, false).unwrap();
    assert_ne!(borrower.debug_get_page(0), lender.debug_get_page(0));
    assert_eq!(read_page(&borrower, 0), vec![1; page::SIZE]);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
    borrower.unpin(0, PAGE_SIZE);
}

#[test]
fn writing_a_lent_page_takes_it_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    lender.write(&[7; 1], 0).unwrap();

    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), lender.debug_get_page(PAGE_SIZE));
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);
}

#[test]
fn pinning_a_lent_page_takes_it_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    lender.commit_range_pinned(0, PAGE_SIZE, false).unwrap();
    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
    // A pinned page cannot be lent.
    assert_eq!(borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0), Err(Status::NOT_FOUND));
    lender.unpin(0, PAGE_SIZE);
}

#[test]
fn evicting_a_lent_page_takes_it_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    let page = lender.debug_get_page(0).unwrap();
    make_reclaimable(&[page, lender.debug_get_page(PAGE_SIZE).unwrap()]);
    let reclaimed = lender.cow_pages().reclaim_page(page, 0, EvictionAction::IgnoreHint).unwrap();
    assert_eq!(reclaimed.num_pages, 2);

    assert_eq!(lender.committed_bytes(), 0);
    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), None);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
}

#[test]
fn evicting_a_borrower_leaves_its_shared_pages() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    // The borrower has a page of its own, and borrows the second.
    let (borrower, pages) = make_partially_committed_pager_vmo(2, 1, false, true, false).unwrap();
    borrower.supply_shared_pages(PAGE_SIZE, PAGE_SIZE, &lender, 0).unwrap();

    make_reclaimable(&[pages[0], lender.debug_get_page(0).unwrap()]);
    let reclaimed =
        borrower.cow_pages().reclaim_page(pages[0], 0, EvictionAction::IgnoreHint).unwrap();
    assert_eq!(reclaimed.num_pages, 1);

    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(borrower.debug_get_page(PAGE_SIZE), lender.debug_get_page(0));
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);
}

#[test]
fn destroying_a_lender_takes_its_pages_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    drop(lender);
    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(borrower.borrowed_bytes(), 0);
}

#[test]
fn destroying_a_borrower_gives_its_pages_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();
    let page = lender.debug_get_page(0);

    drop(borrower);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
    assert_eq!(lender.debug_get_page(0), page);
    assert_eq!(read_page(&lender, 0), vec![1; page::SIZE]);
}

#[test]
fn shrinking_gives_back_or_takes_back_shared_pages() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(2);
    let borrower = make_borrower(2, false);
    borrower.supply_shared_pages(0, 2 * PAGE_SIZE, &lender, 0).unwrap();

    borrower.resize(PAGE_SIZE).unwrap();
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);
    assert_eq!(lender.committed_bytes(), 2 * PAGE_SIZE);

    lender.resize(0).unwrap();
    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
}

#[test]
fn zeroing_a_shared_page_gives_it_back() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    borrower.zero_range(0, PAGE_SIZE).unwrap();
    assert_eq!(read_page(&borrower, 0), vec![0; page::SIZE]);
    assert_eq!(read_page(&lender, 0), vec![1; page::SIZE]);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
}

#[test]
fn detaching_gives_back_or_takes_back_shared_pages() {
    let _scanner = AutoVmScannerDisable::new();
    let lender = make_lender(1);
    let borrower = make_borrower(1, false);
    let other_borrower = make_borrower(1, false);
    borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();
    other_borrower.supply_shared_pages(0, PAGE_SIZE, &lender, 0).unwrap();

    borrower.detach_source();
    assert_eq!(borrower.debug_get_page(0), None);
    assert_eq!(lender.cow_pages().debug_loan_count(), 1);

    lender.detach_source();
    assert_eq!(other_borrower.debug_get_page(0), None);
    assert_eq!(lender.cow_pages().debug_loan_count(), 0);
}
