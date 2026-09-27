// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/pager-writeback/pager-writeback.cc`.
//!
//! Tests upstream skips without the system resource, only as `TestThread::wait_for_blocked` is
//! unreliable in a component, are run, as it asks the kernel about the test thread directly.
//!
//! Test threads that update the expected contents in Zircon update them here in the test itself
//! before checking, as the updates are deterministic.
//!
//! Not ported, as the kernel lacks what they need:
//! - VMOs have no children: `NoDirtyRequestsForClones`, `NoQueryOnClone`, `ChildAttributedCounts`.
//! - There is no system resource, so no BTIs to pin with: `DelayedPinAwaitingClean`,
//!   `FailedPinAwaitingClean`, `DirtyAfterPin`, `PinAfterDirty`, `NotModifiedFailedPinWrite`,
//!   `NotModifiedPartialFailedPinWrite`.
//! - Threads cannot be suspended: `SuspendDirtyTest`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;
use zircon_core_tests::vmo_vmar_test;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

fn ps() -> u64 {
    page_size()
}

fn psu() -> usize {
    page_size() as usize
}

/// `count` pages of `fill`, as `vmo_test::TestFillPages` returns.
fn test_fill_pages(count: usize, fill: u8) -> Vec<u8> {
    vec![fill; count * psu()]
}

fn range(offset: u64, length: u64) -> zx_vmo_dirty_range_t {
    zx_vmo_dirty_range_t { offset, length, options: 0 }
}

fn zero_range(offset: u64, length: u64) -> zx_vmo_dirty_range_t {
    zx_vmo_dirty_range_t { offset, length, options: zx::sys::ZX_VMO_DIRTY_RANGE_IS_ZERO }
}

/// Defines a module testing `$body` with VMOs created with `$base` options both with and without
/// `TRAP_DIRTY`, as `TEST_WITH_AND_WITHOUT_TRAP_DIRTY`. The body creates VMOs with `$create_option`.
macro_rules! trap_dirty_test {
    ($name:ident, $base:expr, |$create_option:ident| $body:block) => {
        mod $name {
            #[allow(unused_imports)]
            use super::*;

            fn run($create_option: zx::VmoOptions) $body

            #[test]
            fn trap_dirty() {
                run($base | zx::VmoOptions::TRAP_DIRTY)
            }

            #[test]
            fn no_trap_dirty() {
                run($base)
            }
        }
    };
}

/// Supplies `count` pages at `page_offset` from the empty `vmo_src`, so that the destination gets
/// zero page markers.
fn supply_zero_pages(pager: &UserPager, vmo: &Vmo, vmo_src: &zx::Vmo, page_offset: u64, count: u64) {
    let range = page_offset * ps()..(page_offset + count) * ps();
    pager.pager().supply_pages(vmo.vmo(), range, vmo_src, 0).unwrap();
}

fn committed_bytes(vmo: &Vmo) -> u64 {
    vmo.vmo().info().unwrap().committed_bytes
}

/// Asserts there are no outstanding read or dirty requests for `vmo`.
fn assert_no_requests(pager: &UserPager, vmo: &Vmo) {
    assert!(pager.get_page_dirty_request(vmo, INFINITE_PAST).is_none());
    assert!(pager.get_page_read_request(vmo, INFINITE_PAST).is_none());
}

/// Maps `len` bytes of `vmo` readable and writable, returning the address.
fn map_rw(vmo: &Vmo, len: u64) -> Option<usize> {
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
    match root_vmar().map(0, vmo.vmo(), 0, len as usize, flags) {
        Ok(ptr) => Some(ptr),
        Err(_) => {
            eprintln!("could not map vmo");
            None
        }
    }
}

/// Unmaps a mapping when dropped, as the tests' deferred unmaps do.
struct Unmap(Arc<AtomicUsize>, u64);

impl Drop for Unmap {
    fn drop(&mut self) {
        let ptr = self.0.load(Ordering::Acquire);
        if ptr != 0 {
            // Cleanup the mapping we created.
            // SAFETY: Nothing refers to the mapping any more.
            let _ = unsafe { root_vmar().unmap(ptr, self.1 as usize) };
        }
    }
}

// Tests that a VMO created with TRAP_DIRTY can be supplied, and generates VMO_DIRTY requests when
// written to.
vmo_vmar_test!(simple_trap_dirty, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));

    // Supply the page first and then attempt to write to it.
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t1.wait());

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    let data = 0x77u8;
    expected[0] = data;
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.vmo().write(&[data], 0).is_ok());
    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t2.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, check_vmar));

    // No more requests.
    assert_no_requests(&pager, &vmo);

    // Writes to a VMO created without TRAP_DIRTY go through without blocking.
    let vmo_no_trap = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&vmo_no_trap, 0, 1));
    let data = 0xccu8;
    vmo_no_trap.vmo().write(&[data], 0).unwrap();

    vmo_no_trap.generate_buffer_contents(&mut expected, 1, 0);
    expected[0] = data;
    assert!(check_buffer_data(&vmo_no_trap, 0, 1, &expected, check_vmar));

    // Verify that a non pager-backed vmo cannot be created with TRAP_DIRTY.
    let mut handle = 0;
    // SAFETY: `handle` is valid for writes.
    let status = unsafe { zx::sys::zx_vmo_create(ps(), zx::sys::ZX_VMO_TRAP_DIRTY, &mut handle) };
    assert_eq!(status, zx::sys::ZX_ERR_INVALID_ARGS);

    // No requests seen.
    assert_no_requests(&pager, &vmo_no_trap);
});

// Tests that OP_DIRTY dirties pages even without a write to the VMO.
#[test]
fn op_dirty_no_write() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create a VMO and supply a page.
    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Dirty the page directly with the pager op.
    assert!(pager.dirty_pages(&vmo, 0, 1));

    // The page should now be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // VMO content is unchanged.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Tests that writing to the VMO with zx_vmo_write generates DIRTY requests as expected.
#[test]
fn dirty_requests_on_vmo_write() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 20;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    let data = 0x77u8;
    // write alternate pages {0, 2, 4, 6, 8}.
    for i in (0..NUM_PAGES / 2).step_by(2) {
        expected[i as usize * psu()] = data;
    }
    // write consecutive runs of pages too.
    // pages written at this point are [0] [2,3,4] [6] [8].
    expected[3 * psu()] = data;
    // pages written are [11, 16).
    expected[11 * psu()..16 * psu()].fill(0);

    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        for i in (0..NUM_PAGES / 2).step_by(2) {
            if v.vmo().write(&[data], i * ps()).is_err() {
                return false;
            }
        }
        if v.vmo().write(&[data], 3 * ps()).is_err() {
            return false;
        }
        let zero_fill = test_fill_pages(5, 0);
        v.vmo().write(&zero_fill, 11 * ps()).is_ok()
    });
    assert!(t.start());

    for i in (0..NUM_PAGES / 2).step_by(2) {
        assert!(pager.wait_for_page_dirty(&vmo, i, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, i, 1));
    }

    assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 3, 1));

    assert!(pager.wait_for_page_dirty(&vmo, 11, 5, INFINITE));
    assert!(pager.dirty_pages(&vmo, 11, 5));

    assert!(t.wait());

    // Verify dirty ranges.
    let ranges = [range(0, 1), range(2, 3), range(6, 1), range(8, 1), range(11, 5)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that writing to the VMO through a VM mapping generates DIRTY requests as expected.
#[test]
fn dirty_requests_via_mapping() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 20;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    let data = 0xccu8;
    // write alternate pages {0, 2, 4, 6, 8}, [3] and [11, 16).
    let written: Vec<u64> = (0..NUM_PAGES / 2).step_by(2).chain([3]).chain(11..16).collect();
    for i in &written {
        expected[*i as usize * psu()] = data;
    }

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), NUM_PAGES * ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, NUM_PAGES * ps()) else { return false };
        p.store(addr, Ordering::Release);
        let buf = addr as *mut u8;
        for i in &written {
            // SAFETY: The page is mapped.
            unsafe { buf.add(*i as usize * psu()).write_volatile(data) };
        }
        true
    });
    assert!(t.start());

    for i in (0..NUM_PAGES / 2).step_by(2) {
        assert!(pager.wait_for_page_dirty(&vmo, i, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, i, 1));
    }

    assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 3, 1));

    // We're touching pages one by one via the mapping, so we'll see page requests for individual
    // pages. Wait for the first page request and dirty the whole range.
    assert!(pager.wait_for_page_dirty(&vmo, 11, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 11, 5));

    assert!(t.wait());

    // Verify dirty ranges.
    let ranges = [range(0, 1), range(2, 3), range(6, 1), range(8, 1), range(11, 5)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that no DIRTY requests are generated on a read.
#[test]
fn no_dirty_requests_on_read() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 3;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), NUM_PAGES * ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, NUM_PAGES * ps()) else { return false };
        p.store(addr, Ordering::Release);
        // Read pages.
        for i in 0..NUM_PAGES as usize {
            // SAFETY: The page is mapped.
            let _ = unsafe { (addr as *const u8).add(i * psu()).read_volatile() };
        }
        true
    });
    assert!(t.start());

    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    assert!(t.wait());

    // No dirty requests should be seen as none of the pages were dirtied.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // Should be able to read from the VMO without faulting now.
    let mut buf = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.vmo().read(&mut buf, 0).unwrap();

    // No dirty requests should be seen as none of the pages were dirtied.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // No remaining reads.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // No dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Verify contents.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));
}

// Tests that DIRTY requests are generated only on the first write.
#[test]
fn dirty_requests_repeated_writes() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    expected[0] = 0xcc;
    let mut t1 = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, ps()) else { return false };
        p.store(addr, Ordering::Release);
        // SAFETY: The page is mapped.
        unsafe { (addr as *mut u8).write_volatile(0xcc) };
        true
    });
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(t1.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page again.
    let addr = ptr.load(Ordering::Acquire);
    expected[0] = 0xdd;
    let mut t2 = TestThread::new(move || {
        // SAFETY: The page is mapped.
        unsafe { (addr as *mut u8).write_volatile(0xdd) };
        true
    });
    assert!(t2.start());

    // No more requests seen.
    assert_no_requests(&pager, &vmo);

    assert!(t2.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
}

// Tests that DIRTY requests are generated on a write to a page that was previously read from.
#[test]
fn dirty_requests_on_write_after_read() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t1 = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, ps()) else { return false };
        p.store(addr, Ordering::Release);
        // Read from the page.
        // SAFETY: The page is mapped.
        let _ = unsafe { (addr as *const u8).read_volatile() };
        true
    });
    assert!(t1.start());

    // No read or dirty requests.
    assert_no_requests(&pager, &vmo);

    assert!(t1.wait());

    // Now write to the page. This should trigger a dirty request.
    let addr = ptr.load(Ordering::Acquire);
    expected[0] = 0xdd;
    let mut t2 = TestThread::new(move || {
        // SAFETY: The page is mapped.
        unsafe { (addr as *mut u8).write_volatile(0xdd) };
        true
    });
    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(t2.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that writes for overlapping ranges generate the expected DIRTY requests.
#[test]
fn dirty_requests_overlap() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 20;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    // We should not have any page requests yet.
    assert_no_requests(&pager, &vmo);

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || {
        // write pages [4,9).
        v.vmo().write(&test_fill_pages(5, 0xaa), 4 * ps()).is_ok()
    });
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 4, 5, INFINITE));

    // No other page requests seen.
    assert_no_requests(&pager, &vmo);

    let v = vmo.clone();
    expected[2 * psu()..9 * psu()].fill(0xbb);
    let mut t2 = TestThread::new(move || {
        // write pages [2,9).
        v.vmo().write(&test_fill_pages(7, 0xbb), 2 * ps()).is_ok()
    });
    assert!(t2.start());

    // We should only see a request for the non-overlapping portion.
    assert!(pager.wait_for_page_dirty(&vmo, 2, 2, INFINITE));

    // No other page requests seen.
    assert_no_requests(&pager, &vmo);

    // Dirty the range [4,9).
    assert!(pager.dirty_pages(&vmo, 4, 5));
    assert!(t1.wait());

    // Dirty the range [2,4).
    assert!(pager.dirty_pages(&vmo, 2, 2));
    assert!(t2.wait());

    // Verify dirty ranges.
    let mut ranges = vec![range(2, 7)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    let v = vmo.clone();
    expected[11 * psu()..16 * psu()].fill(0xcc);
    let mut t3 = TestThread::new(move || {
        // write pages [11,16).
        v.vmo().write(&test_fill_pages(5, 0xcc), 11 * ps()).is_ok()
    });
    assert!(t3.start());
    assert!(pager.wait_for_page_dirty(&vmo, 11, 5, INFINITE));

    let v = vmo.clone();
    expected[15 * psu()..19 * psu()].fill(0xdd);
    let mut t4 = TestThread::new(move || {
        // write pages [15,19).
        v.vmo().write(&test_fill_pages(4, 0xdd), 15 * ps()).is_ok()
    });
    assert!(t4.start());
    assert!(t4.wait_for_blocked());

    // No remaining requests.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // Dirty the range [11,16).
    assert!(pager.dirty_pages(&vmo, 11, 5));

    // This should terminate t3, and wake up t4 until it blocks again for the remaining range.
    assert!(t3.wait());
    assert!(t4.wait_for_blocked());

    // Verify dirty ranges.
    ranges.push(range(11, 5));
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    assert!(pager.wait_for_page_dirty(&vmo, 16, 3, INFINITE));
    assert!(pager.dirty_pages(&vmo, 16, 3));

    assert!(t4.wait());

    // Verify dirty ranges.
    ranges.last_mut().unwrap().length = 8;
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // The contents of page 15 can vary depending on which of t3 or t4 wrote to it last, as both
    // were blocked on a dirty request for it at the same time, so there's a race.
    let outcome1 = check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true);
    expected[15 * psu()..16 * psu()].fill(0xcc);
    let outcome2 = check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true);
    assert!(outcome1 || outcome2);

    // No remaining requests.
    assert_no_requests(&pager, &vmo);
}

/// The C library's `rand()`, as the tests use.
fn rand() -> u32 {
    // SAFETY: Always safe to call.
    unsafe { libc::rand() as u32 }
}

// Tests that DIRTY requests are generated as expected for a VMO that has random offsets in
// various page states: {Empty, Clean, Dirty}.
#[test]
fn dirty_requests_random_offsets() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 10;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // 0 for empty, 1 for clean, and 2 for dirty
    let mut page_state = [0u32; NUM_PAGES as usize];
    for i in 0..NUM_PAGES {
        page_state[i as usize] = rand() % 3;
        if page_state[i as usize] == 0 {
            // Page not present. Skip ahead.
            continue;
        } else if page_state[i as usize] == 1 {
            // Page is present and clean.
            assert!(pager.supply_pages(&vmo, i, 1));
        } else {
            // Page is present and dirty.
            assert!(pager.supply_pages(&vmo, i, 1));
            assert!(pager.dirty_pages(&vmo, i, 1));
        }
    }

    // Now write to the entire range. We should see a combination of read and dirty requests.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        v.vmo().write(&test_fill_pages(NUM_PAGES as usize, 0), 0).is_ok()
    });
    assert!(t.start());

    let mut clean_start = 0u64;
    let mut clean_len = 0u64;
    let mut i = 0i64;
    while i < NUM_PAGES as i64 {
        if page_state[i as usize] == 0 {
            // Page is not present.
            // This might break an in-progress clean run, resolve that first.
            if clean_len > 0 {
                assert!(pager.wait_for_page_dirty(&vmo, clean_start, clean_len, INFINITE));
                assert!(pager.dirty_pages(&vmo, clean_start, clean_len));
            }

            // Check how many pages after this are not present. The request will be batched.
            let mut not_present_run_size = 1u64;
            for j in (i as u64 + 1)..NUM_PAGES {
                if page_state[j as usize] != 0 {
                    break;
                }
                not_present_run_size += 1;
            }

            // Should see a read request for this not-present page run.
            assert!(pager.wait_for_page_read(&vmo, i as u64, not_present_run_size, INFINITE));
            assert!(pager.supply_pages(&vmo, i as u64, not_present_run_size));

            // After the supply, visit this page again, as it might get combined into a subsequent
            // clean run. Set all the pages in the run to clean and decrement i.
            for j in i as u64..i as u64 + not_present_run_size {
                page_state[j as usize] = 1;
            }
            i -= 1;

            clean_start = (i + 1) as u64;
            clean_len = 0;
        } else if page_state[i as usize] == 1 {
            // Page is present and clean. Accumulate into the clean run.
            clean_len += 1;
        } else {
            // Page is present and dirty.
            // This might break an in-progress clean run, resolve that first.
            if clean_len > 0 {
                assert!(pager.wait_for_page_dirty(&vmo, clean_start, clean_len, INFINITE));
                assert!(pager.dirty_pages(&vmo, clean_start, clean_len));
            }
            clean_start = (i + 1) as u64;
            clean_len = 0;
        }
        i += 1;
    }

    // Resolve the last clean run if any.
    if clean_len > 0 {
        assert!(pager.wait_for_page_dirty(&vmo, clean_start, clean_len, INFINITE));
        assert!(pager.dirty_pages(&vmo, clean_start, clean_len));
    }

    assert!(t.wait());

    // No remaining requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that ZX_PAGER_OP_FAIL can fail DIRTY page requests and propagate the failure up.
#[test]
fn fail_dirty_requests() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 2;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t1 = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, ps()) else { return false };
        p.store(addr, Ordering::Release);
        // Write page 0.
        // SAFETY: The page is mapped.
        unsafe { (addr as *mut u8).write_volatile(0xcc) };
        true
    });
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));

    assert!(t1.wait_for_crash(ptr.load(Ordering::Acquire), zx::Status::IO));

    // No pages should be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    let v = vmo.clone();
    let mut t2 = TestThread::new(move || {
        // Write page 1.
        v.vmo().write(&[0xdd], ps()).is_ok()
    });
    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
    assert!(pager.fail_pages(&vmo, 1, 1, zx::Status::IO));

    assert!(t2.wait_for_failure());

    // No pages should be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that DIRTY requests are generated for zero pages, i.e. pages supplied as zero markers.
#[test]
fn dirty_requests_for_zero_pages() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 2;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Supply with empty source vmo so that the destination gets zero page markers.
    let vmo_src = zx::Vmo::create(NUM_PAGES * ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, NUM_PAGES);

    // Verify that the pager vmo has no committed pages, i.e. it only has markers.
    assert_eq!(committed_bytes(&vmo), 0);

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];

    // Write to the first page with zx_vmo_write.
    let data = 0xaau8;
    expected[0] = data;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[data], 0).is_ok());
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Dirty the first page.
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t1.wait());

    // Verify that the pager vmo has one committed page now.
    assert_eq!(committed_bytes(&vmo), ps());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // Map the second page of the vmo.
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
    let ptr = root_vmar().map(0, vmo.vmo(), ps(), psu(), flags).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // Write to the second page via the mapping.
    let data = 0xbbu8;
    expected[psu()] = data;
    let mut t2 = TestThread::new(move || {
        // SAFETY: The page is mapped.
        unsafe { (ptr as *mut u8).write_volatile(data) };
        true
    });

    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));

    // Dirty the second page.
    assert!(pager.dirty_pages(&vmo, 1, 1));
    assert!(t2.wait());

    // Verify that the pager vmo has both pages committed now.
    assert_eq!(committed_bytes(&vmo), NUM_PAGES * ps());

    // Verify that both the pages are now dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 2)]));
    // SAFETY: The page is mapped.
    assert_eq!(unsafe { (ptr as *const u8).read_volatile() }, data);
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that ZX_PAGER_OP_DIRTY works for a mix of zero and non-zero pages.
#[test]
fn dirty_zero_and_non_zero_pages() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 10;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];

    // Empty source vmo to supply with zero pages.
    let vmo_src = zx::Vmo::create(ps()).unwrap();

    // For each page offset, supply either a zero or a non-zero page.
    let mut non_zero_count = 0;
    for i in 0..NUM_PAGES {
        if rand() % 2 != 0 {
            non_zero_count += 1;
            assert!(pager.supply_pages(&vmo, i, 1));
            vmo.generate_buffer_contents(&mut expected[i as usize * psu()..], 1, i);
        } else {
            supply_zero_pages(&pager, &vmo, &vmo_src, i, 1);
        }
    }

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // Only non-zero pages should be committed.
    assert_eq!(committed_bytes(&vmo), non_zero_count * ps());

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Verify that we're able to dirty the entire range regardless of the type of page.
    assert!(pager.dirty_pages(&vmo, 0, NUM_PAGES));

    // All the pages should be committed and dirty now.
    assert_eq!(committed_bytes(&vmo), NUM_PAGES * ps());
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, NUM_PAGES)]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that ZX_PAGER_OP_FAIL can fail DIRTY page requests for zero pages.
#[test]
fn fail_dirty_requests_for_zero_pages() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Supply with empty source vmo so that the destination gets zero page markers.
    let vmo_src = zx::Vmo::create(ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, 1);

    // Verify that the pager vmo has no committed pages, i.e. it only has markers.
    assert_eq!(committed_bytes(&vmo), 0);

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Buffer to verify VMO contents later.
    let expected = vec![0u8; psu()];

    // Attempt to write to the first page.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[0xaa], 0).is_ok());
    assert!(t.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Fail the dirty request.
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));

    // The thread should exit with failure.
    assert!(t.wait_for_failure());

    // No committed pages still.
    assert_eq!(committed_bytes(&vmo), 0);

    // No dirty pages too.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that DIRTY requests are generated for ranges including zero pages as expected.
#[test]
fn dirty_requests_for_zero_ranges() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 10;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];

    // Empty source vmo to supply with zero pages.
    let vmo_src = zx::Vmo::create(ps()).unwrap();

    // Supply zero page markers for pages 0 and 1.
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, 1);
    supply_zero_pages(&pager, &vmo, &vmo_src, 1, 1);

    // Attempt to write to the range [0, 2).
    expected[..2 * psu()].fill(0xaa);
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&test_fill_pages(2, 0xaa), 0).is_ok());

    assert!(t1.start());

    // We should see a dirty request for the range [0, 2). Verifies that the range is extended to
    // include another marker.
    assert!(pager.wait_for_page_dirty(&vmo, 0, 2, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 2));
    assert!(t1.wait());

    // Verify dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 2)]));
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // Supply a zero marker for page 2 and a non-zero page for page 3.
    supply_zero_pages(&pager, &vmo, &vmo_src, 2, 1);
    assert!(pager.supply_pages(&vmo, 3, 1));

    // Attempt to write to the range [2, 4).
    expected[2 * psu()..4 * psu()].fill(0xbb);
    let v = vmo.clone();
    let mut t2 =
        TestThread::new(move || v.vmo().write(&test_fill_pages(2, 0xbb), 2 * ps()).is_ok());

    assert!(t2.start());

    // We should see a dirty request for the range [2, 4). Verifies that the range is extended to
    // include a non-zero clean page.
    assert!(pager.wait_for_page_dirty(&vmo, 2, 2, INFINITE));
    assert!(pager.dirty_pages(&vmo, 2, 2));
    assert!(t2.wait());

    // Verify dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 4)]));
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // For the rest of the pages, supply a mix of zero and non-zero pages, leaving a gap at the end.
    for i in 4..NUM_PAGES - 1 {
        if rand() % 2 != 0 {
            assert!(pager.supply_pages(&vmo, i, 1));
        } else {
            supply_zero_pages(&pager, &vmo, &vmo_src, i, 1);
        }
    }

    // Attempt to write to the range [4, 10).
    let len = NUM_PAGES as usize - 4;
    expected[4 * psu()..].fill(0xcc);
    let v = vmo.clone();
    let mut t3 =
        TestThread::new(move || v.vmo().write(&test_fill_pages(len, 0xcc), 4 * ps()).is_ok());

    assert!(t3.start());

    // We should see a dirty request for pages [4, 9). Verifies that zero and non-zero clean pages
    // get picked up in a single range, and that the range stops before a gap.
    assert!(pager.wait_for_page_dirty(&vmo, 4, 5, INFINITE));
    assert!(pager.dirty_pages(&vmo, 4, 5));

    // We should now see a read request followed by a dirty request for the last gap.
    assert!(pager.wait_for_page_read(&vmo, 9, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 9, 1));
    assert!(pager.wait_for_page_dirty(&vmo, 9, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 9, 1));
    assert!(t3.wait());

    // Verify dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, NUM_PAGES)]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that no DIRTY requests are generated on a commit.
#[test]
fn no_dirty_requests_on_commit() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 5;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    // Supply some pages.
    assert!(pager.supply_pages(&vmo, 0, 2));

    // Commit the vmo.
    let v = vmo.clone();
    let mut t =
        TestThread::new(move || v.vmo().op_range(zx::VmoOp::COMMIT, 0, NUM_PAGES * ps()).is_ok());
    assert!(t.start());

    // Should see a read request for the uncommitted portion.
    assert!(pager.wait_for_page_read(&vmo, 2, NUM_PAGES - 2, INFINITE));
    assert!(pager.supply_pages(&vmo, 2, NUM_PAGES - 2));

    // The thread should be able to exit now.
    assert!(t.wait());

    // No dirty requests should be seen as none of the pages were dirtied.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // No remaining reads.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // No dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
}

// Tests that no DIRTY requests are generated when a mapping is created with MAP_RANGE.
#[test]
fn no_dirty_requests_on_map_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 3;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), NUM_PAGES * ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t1 = TestThread::new(move || {
        // Map the vmo, and populate mappings for all committed pages. We know the pages are
        // pre-committed so we should not block on reads. And we should not be generating any
        // dirty requests to block on either.
        let flags =
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::MAP_RANGE;
        match root_vmar().map(0, v.vmo(), 0, NUM_PAGES as usize * psu(), flags) {
            Ok(addr) => {
                p.store(addr, Ordering::Release);
                true
            }
            Err(_) => false,
        }
    });

    assert!(t1.start());

    // No dirty requests should be seen as none of the pages were dirtied.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());
    // No reads either.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    assert!(t1.wait());

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    let addr = ptr.load(Ordering::Acquire);
    let mut t2 = TestThread::new(move || {
        // Read the mapped pages. This will not block.
        for i in 0..NUM_PAGES as usize {
            // SAFETY: The page is mapped.
            let _ = unsafe { (addr as *const u8).add(i * psu()).read_volatile() };
        }
        true
    });

    assert!(t2.start());

    // No dirty or read requests.
    assert_no_requests(&pager, &vmo);

    assert!(t2.wait());

    // No dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    for i in 0..NUM_PAGES as usize {
        expected[i * psu()] = 0xcc;
    }
    let mut t3 = TestThread::new(move || {
        // Now try to write to the vmo. This should result in write faults and dirty requests.
        for i in 0..NUM_PAGES as usize {
            // SAFETY: The page is mapped.
            unsafe { (addr as *mut u8).add(i * psu()).write_volatile(0xcc) };
        }
        true
    });

    assert!(t3.start());

    // The thread will block on dirty requests for each page.
    for i in 0..NUM_PAGES {
        assert!(pager.wait_for_page_dirty(&vmo, i, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, i, 1));
    }

    // The thread should now exit.
    assert!(t3.wait());

    // All pages are dirty now.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, NUM_PAGES)]));

    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more dirty or read requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that no DIRTY requests are generated when previously dirty pages are mapped and written
// to.
#[test]
fn no_dirty_requests_map_existing_dirty() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Dirty the page.
    expected[0] = 0xcc;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[0xcc], 0).is_ok());

    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(t1.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Map the page and try writing to it.
    let ptr = Arc::new(AtomicUsize::new(0));
    let _unmap = Unmap(ptr.clone(), ps());
    let (v, p) = (vmo.clone(), ptr.clone());
    expected[0] = 0xdd;
    let mut t2 = TestThread::new(move || {
        // Map the vmo.
        let Some(addr) = map_rw(&v, ps()) else { return false };
        p.store(addr, Ordering::Release);
        // SAFETY: The page is mapped.
        unsafe { (addr as *mut u8).write_volatile(0xdd) };
        true
    });

    assert!(t2.start());

    // No read or dirty requests.
    assert_no_requests(&pager, &vmo);

    assert!(t2.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
}

// Tests that WRITEBACK_BEGIN/END clean pages as expected.
trap_dirty_test!(simple_writeback, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Dirty the page by writing to it.
    let data = 0xaau8;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[data], 0).is_ok());
    assert!(t1.start());

    if create_option.contains(zx::VmoOptions::TRAP_DIRTY) {
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }
    assert!(t1.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    expected[0] = data;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Begin writeback on the page.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));

    // The page is still dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // This should transition the page to clean, and a subsequent write should trigger
    // another dirty request.
    assert!(pager.writeback_end_pages(&vmo, 0, 1));

    // No dirty pages after writeback end.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Dirty the page again.
    expected[0] = 0x77;
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.vmo().write(&[0x77], 0).is_ok());

    assert!(t2.start());

    if create_option.contains(zx::VmoOptions::TRAP_DIRTY) {
        // We should see a dirty request now.
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }
    assert!(t2.wait());

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});
