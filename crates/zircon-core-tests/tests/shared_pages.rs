// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of `zx_pager_supply_shared_pages`, which the in-process kernel adds to Zircon: a pager
//! supplies a VMO with the pages of another, which the two share until either changes them.

use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;
use zircon_core_tests::vmo_vmar_test;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

fn psu() -> usize {
    page_size() as usize
}

/// Returns the content the pager supplies to `vmo` for `num_pages` from `page_offset`.
fn contents(vmo: &Vmo, num_pages: u64, page_offset: u64) -> Vec<u8> {
    let mut buffer = vec![0; num_pages as usize * psu()];
    vmo.generate_buffer_contents(&mut buffer, num_pages, page_offset);
    buffer
}

fn share(pager: &UserPager, vmo: &Vmo, page_offset: u64, num_pages: u64, src: &Vmo, src_page: u64) {
    pager
        .pager()
        .supply_shared_pages(
            vmo.vmo(),
            page_offset * page_size()..(page_offset + num_pages) * page_size(),
            src.vmo(),
            src_page * page_size(),
        )
        .unwrap();
}

// A VMO reads the pages it shares, without page requests.
vmo_vmar_test!(shared_pages_are_read, |check_vmar| {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(3).unwrap();
    assert!(pager.supply_pages(&src, 0, 3));
    let vmo = pager.create_vmo(2).unwrap();
    share(&pager, &vmo, 0, 2, &src, 1);

    assert!(check_buffer_data(&vmo, 0, 2, &contents(&src, 2, 1), check_vmar));
    assert!(!pager.wait_for_page_read(&vmo, 0, 2, INFINITE_PAST));
});

// Shared pages are counted once, by the VMO they came from.
#[test]
fn shared_pages_are_populated_but_not_committed() {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(2).unwrap();
    assert!(pager.supply_pages(&src, 0, 2));
    let vmo = pager.create_vmo(2).unwrap();
    share(&pager, &vmo, 0, 2, &src, 0);

    let info = vmo.vmo().info().unwrap();
    assert_eq!(info.committed_bytes, 0);
    assert_eq!(info.populated_bytes, 2 * page_size());
    let info = src.vmo().info().unwrap();
    assert_eq!(info.committed_bytes, 2 * page_size());
}

// Writing a shared page raises a DIRTY request, and the write lands in a copy of the page, leaving
// the VMO it came from unchanged.
vmo_vmar_test!(writing_a_shared_page_copies_it, |check_vmar| {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(2).unwrap();
    assert!(pager.supply_pages(&src, 0, 2));
    let vmo = pager.create_vmo_with_options(2, zx::VmoOptions::TRAP_DIRTY).unwrap();
    share(&pager, &vmo, 0, 2, &src, 0);
    // Map the shared pages first, so that the write faults on a mapped page.
    assert!(check_buffer_data(&vmo, 0, 2, &contents(&src, 2, 0), true));

    let data = 0x77u8;
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        if check_vmar {
            // SAFETY: The VMO is mapped for its life.
            unsafe { (v.base_addr() as *mut u8).write_volatile(data) };
            true
        } else {
            v.vmo().write(&[data], 0).is_ok()
        }
    });
    assert!(t.start());
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t.wait());

    let mut expected = contents(&src, 2, 0);
    expected[0] = data;
    assert!(check_buffer_data(&vmo, 0, 2, &expected, check_vmar));
    assert!(check_buffer_data(&src, 0, 2, &contents(&src, 2, 0), check_vmar));
    let info = vmo.vmo().info().unwrap();
    assert_eq!(info.committed_bytes, page_size());
    assert_eq!(info.populated_bytes, 2 * page_size());
});

// Changing a page that is shared takes it back from the VMOs sharing it, which request it again.
vmo_vmar_test!(changing_a_shared_page_takes_it_back, |check_vmar| {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&src, 0, 1));
    let vmo = pager.create_vmo(1).unwrap();
    share(&pager, &vmo, 0, 1, &src, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &contents(&src, 1, 0), check_vmar));

    src.vmo().write(&[0x77], 0).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t.wait());
});

// Destroying the VMO that pages came from takes them back.
#[test]
fn destroying_the_source_takes_back_its_pages() {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&src, 0, 1));
    let vmo = pager.create_vmo(1).unwrap();
    share(&pager, &vmo, 0, 1, &src, 0);

    pager.release_vmo(&src);
    drop(src);
    assert_eq!(vmo.vmo().info().unwrap().populated_bytes, 0);

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, false));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t.wait());
}

#[test]
fn invalid_supply_shared_pages() {
    let mut pager = UserPager::initialized();
    let src = pager.create_vmo(2).unwrap();
    assert!(pager.supply_pages(&src, 0, 1));
    let vmo = pager.create_vmo(2).unwrap();
    let supply = |vmo: &zx::Vmo, range: std::ops::Range<u64>, src: &zx::Vmo, src_offset| {
        pager.pager().supply_shared_pages(vmo, range, src, src_offset)
    };
    let ps = page_size();

    // Unaligned ranges.
    assert_eq!(supply(vmo.vmo(), 1..ps, src.vmo(), 0), Err(zx::Status::INVALID_ARGS));
    assert_eq!(supply(vmo.vmo(), 0..ps, src.vmo(), 1), Err(zx::Status::INVALID_ARGS));
    // Beyond either VMO.
    assert_eq!(supply(vmo.vmo(), 0..ps, src.vmo(), 2 * ps), Err(zx::Status::OUT_OF_RANGE));
    assert_eq!(supply(vmo.vmo(), 2 * ps..3 * ps, src.vmo(), 0), Err(zx::Status::OUT_OF_RANGE));
    // A source page that is not there.
    assert_eq!(supply(vmo.vmo(), 0..2 * ps, src.vmo(), 0), Err(zx::Status::NOT_FOUND));
    // A source that is not pager backed.
    let anonymous = zx::Vmo::create(ps).unwrap();
    assert_eq!(supply(vmo.vmo(), 0..ps, &anonymous, 0), Err(zx::Status::NOT_SUPPORTED));
    // A source that cannot be read.
    let unreadable = src.vmo().duplicate_handle(zx::Rights::WRITE).unwrap();
    assert_eq!(supply(vmo.vmo(), 0..ps, &unreadable, 0), Err(zx::Status::ACCESS_DENIED));
    // A VMO of another pager.
    let mut other_pager = UserPager::initialized();
    let other = other_pager.create_vmo(1).unwrap();
    assert_eq!(supply(other.vmo(), 0..ps, src.vmo(), 0), Err(zx::Status::INVALID_ARGS));
    // Nothing was shared.
    assert_eq!(vmo.vmo().info().unwrap().populated_bytes, 0);
}
