// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/pager-writeback/pager-writeback.cc`.
//!
//! Tests upstream skips without the system resource, only as `TestThread::wait_for_blocked` is
//! unreliable in a component, are run, as it asks the kernel about the test thread directly.
//!
//! `QueryVmoStatsInvalidPointer` expects `ZX_ERR_INVALID_ARGS` rather than any failure.
//!
//! Test threads that update the expected contents in Zircon update them here in the test itself
//! before checking, as the updates are deterministic.
//!
//! Not ported, as the kernel lacks what they need:
//! - VMOs have no children: `NoDirtyRequestsForClones`, `NoQueryOnClone`, `OpZeroClone`,
//!   `NotModifiedCloneWrite`, `SliceWrite`, `SliceOpZero`, `ChildAttributedCounts`.
//! - There is no system resource, so no BTIs to pin with: `NotModifiedOnFailedResize`,
//!   `PinForRead`, `PinForWrite`, `PinnedWriteback`, `ReadPinAwaitingClean`,
//!   `WritePinAwaitingClean`, `DelayedPinAwaitingClean`, `FailedPinAwaitingClean`, `DirtyAfterPin`,
//!   `PinAfterDirty`, `PinForWriteUnpopulated`, `NotModifiedFailedPinWrite`,
//!   `NotModifiedPartialFailedPinWrite`, `SlicePinWrite`, `OpZeroPinned`.
//! - Threads cannot be suspended: `SuspendDirtyTest`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;
use zircon_core_tests::vmo_vmar_test;
use zx::sys::{
    ZX_ERR_ACCESS_DENIED, ZX_ERR_BAD_HANDLE, ZX_ERR_BAD_STATE, ZX_ERR_BUFFER_TOO_SMALL,
    ZX_ERR_INTERNAL, ZX_ERR_INVALID_ARGS, ZX_ERR_NOT_FOUND, ZX_ERR_NOT_SUPPORTED,
    ZX_ERR_OUT_OF_RANGE, ZX_ERR_WRONG_TYPE, ZX_HANDLE_INVALID, ZX_OK, ZX_PAGER_OP_DIRTY,
    ZX_PAGER_OP_FAIL, ZX_PAGER_OP_WRITEBACK_BEGIN, ZX_PAGER_OP_WRITEBACK_END, zx_handle_t,
    zx_status_t,
};

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
fn supply_zero_pages(
    pager: &UserPager,
    vmo: &Vmo,
    vmo_src: &zx::Vmo,
    page_offset: u64,
    count: u64,
) {
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

/// Reads the byte at `ptr`.
fn read_byte(ptr: usize) -> u8 {
    // SAFETY: The tests only pass addresses of mapped pages.
    unsafe { (ptr as *const u8).read_volatile() }
}

/// Writes `data` to the byte at `ptr`.
fn write_byte(ptr: usize, data: u8) {
    // SAFETY: The tests only pass addresses of mapped pages.
    unsafe { (ptr as *mut u8).write_volatile(data) }
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
        for i in &written {
            write_byte(addr + *i as usize * psu(), data);
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
            read_byte(addr + i * psu());
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
        write_byte(addr, 0xcc);
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
        write_byte(addr, 0xdd);
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
        read_byte(addr);
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
        write_byte(addr, 0xdd);
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
    let mut t =
        TestThread::new(move || v.vmo().write(&test_fill_pages(NUM_PAGES as usize, 0), 0).is_ok());
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
        write_byte(addr, 0xcc);
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
        write_byte(ptr, data);
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
    assert_eq!(read_byte(ptr), data);
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
        if !rand().is_multiple_of(2) {
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
        if !rand().is_multiple_of(2) {
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
        let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::MAP_RANGE;
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
            read_byte(addr + i * psu());
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
            write_byte(addr + i * psu(), 0xcc);
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
        write_byte(addr, 0xdd);
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

// Tests that a write after WRITEBACK_BEGIN but before WRITEBACK_END is handled correctly.
#[test]
fn dirty_during_writeback() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Dirty the page.
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Begin writeback on the page.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));

    // The page is still dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page before ending writeback. This should generate a dirty request.
    expected[0] = 0xcc;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[0xcc], 0).is_ok());

    assert!(t1.start());

    // Verify that we saw the dirty request but do not acknowledge it yet. The write will remain
    // blocked.
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // End the writeback. This should transition the page to clean.
    assert!(pager.writeback_end_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The writing thread is still blocked.
    assert!(t1.wait_for_blocked());

    // Now dirty the page, unblocking the writing thread.
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t1.wait());

    // The page is dirty again.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Begin another writeback, and try writing again before ending it. This time acknowledge the
    // dirty request while the writeback is in progress.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Write to the page before ending writeback. This should generate a dirty request.
    expected[0] = 0xdd;
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.vmo().write(&[0xdd], 0).is_ok());

    assert!(t2.start());

    // Verify that we saw the dirty request.
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // This should reset the page state to dirty so that it is not moved to clean when the
    // writeback ends later.
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(t2.wait());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Now end the writeback. This should *not* clean the page, as a write was accepted after
    // beginning the writeback.
    assert!(pager.writeback_end_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that mapping write permissions are cleared as expected on writeback.
#[test]
fn writeback_with_mapping() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Map the vmo.
    let ptr = map_rw(&vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // Write to the vmo. This will be trapped and generate a dirty request.
    let mut data = 0xaau8;
    expected[0] = data;
    let mut t1 = TestThread::new(move || {
        write_byte(ptr, data);
        true
    });

    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Dirty the page.
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t1.wait());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page again. This should go through without any page faults / dirty requests.
    data = 0xbb;
    write_byte(ptr, data);
    expected[0] = data;
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Start a writeback.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page again. This should result in a fault / dirty request.
    let mut t2 = TestThread::new(move || {
        write_byte(ptr, 0xcc);
        true
    });

    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Fail the dirty request so the writeback can complete.
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));
    assert!(t2.wait_for_crash(ptr, zx::Status::IO));

    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Complete the writeback, making the page clean.
    assert!(pager.writeback_end_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page again. This should again be trapped.
    data = 0xdd;
    expected[0] = data;
    let mut t3 = TestThread::new(move || {
        write_byte(ptr, data);
        true
    });

    assert!(t3.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));

    assert!(t3.wait());

    // The page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that the zero page marker cannot be overwritten by another page, unless written to at
// which point it is forked.
trap_dirty_test!(cannot_overwrite_zero_page, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();

    // Supply with empty source vmo so that the destination gets zero page markers.
    let vmo_src = zx::Vmo::create(ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, 1);

    // Verify that the pager vmo has no committed pages, i.e. it only has markers.
    assert_eq!(committed_bytes(&vmo), 0);

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Commit a page in the source to attempt another supply.
    vmo_src.write(&[0xaa], 0).unwrap();

    // Supplying the same page again should not overwrite the zero page marker. The supply will
    // succeed as a no-op.
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, 1);

    // No committed pages still.
    assert_eq!(committed_bytes(&vmo), 0);

    // The VMO is still all zeros.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Now write to the VMO. This should fork the zero page.
    expected[0] = 0xbb;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[0xbb], 0).is_ok());
    assert!(t1.start());

    // Wait for and acknowledge the dirty request if configured to trap dirty transitions.
    if create_option.contains(zx::VmoOptions::TRAP_DIRTY) {
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        // Dirty the first page.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    assert!(t1.wait());

    // Verify that the pager vmo has one committed page now.
    assert_eq!(committed_bytes(&vmo), ps());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Verify written data.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that VMOs created without the ZX_VMO_TRAP_DIRTY flag track dirty pages as expected.
#[test]
fn simple_dirty_no_trap() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create a VMO without the ZX_VMO_TRAP_DIRTY flag.
    let vmo = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Write to the page now. This should go through without blocking.
    let mut data = 0x77u8;
    expected[0] = data;
    vmo.vmo().write(&[data], 0).unwrap();

    // We should now have one dirty page.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Verify written data.
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

    // Map the vmo.
    let ptr = map_rw(&vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // Write to the vmo again via the mapping.
    data = 0x55;
    write_byte(ptr, data);
    expected[0] = data;

    // The page should get dirtied again.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No dirty or read requests seen.
    assert_no_requests(&pager, &vmo);
}

// Tests that VMOs created without the ZX_VMO_TRAP_DIRTY flag track dirty pages as expected for a
// random mix of zero and non-zero pages.
#[test]
fn dirty_no_trap_random_offsets() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create a VMO without the ZX_VMO_TRAP_DIRTY flag.
    const NUM_PAGES: u64 = 10;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];

    // Empty source vmo to supply with zero pages.
    let vmo_src = zx::Vmo::create(ps()).unwrap();

    // For each page offset, supply either a zero or a non-zero page.
    let mut non_zero_count = 0;
    for i in 0..NUM_PAGES {
        if !rand().is_multiple_of(2) {
            non_zero_count += 1;
            assert!(pager.supply_pages(&vmo, i, 1));
            vmo.generate_buffer_contents(&mut expected[i as usize * psu()..], 1, i);
        } else {
            supply_zero_pages(&pager, &vmo, &vmo_src, i, 1);
        }
    }

    // Only non-zero pages should be committed.
    assert_eq!(committed_bytes(&vmo), non_zero_count * ps());

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // Verify that we're able to write to the entire range regardless of the type of page. Alter
    // the expected contents to verify later.
    for (i, data) in (0..NUM_PAGES as usize).zip(0x77u8..) {
        expected[i * psu()] = data;
    }
    vmo.vmo().write(&expected, 0).unwrap();

    // All the pages should be committed and dirty now.
    assert_eq!(committed_bytes(&vmo), NUM_PAGES * ps());
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, NUM_PAGES)]));
    assert!(check_buffer_data(&vmo, 0, NUM_PAGES, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that adding the WRITE permission with zx_vmar_protect does not override read-only
// mappings required in order to track dirty transitions.
trap_dirty_test!(dirty_after_map_protect, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create a temporary VMAR to work with.
    let temp = TempVmar::allocate(psu());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Buffer to verify VMO contents later.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);

    // Map the vmo read-only first so that the protect step below is not a no-op.
    let ptr = temp.vmar.map(0, vmo.vmo(), 0, psu(), zx::VmarFlags::PERM_READ).unwrap();

    // Read the VMO through the mapping so that the hardware mapping is created.
    assert_eq!(read_byte(ptr), expected[0]);

    // Add the write permission now. This will allow us to write to the VMO below.
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
    // SAFETY: Only this test uses the mapping.
    unsafe { temp.vmar.protect(ptr, psu(), flags) }.unwrap();

    // Write to the vmo. This should trigger a write fault. If the protect above added the write
    // permission on the hardware mapping, this write will go through without generating a write
    // fault for dirty tracking.
    let data = 0xaau8;
    expected[0] = data;
    let mut t = TestThread::new(move || {
        write_byte(ptr, data);
        true
    });

    assert!(t.start());

    if create_option.contains(zx::VmoOptions::TRAP_DIRTY) {
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        // Dirty the page.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }
    assert!(t.wait());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));
    assert_eq!(read_byte(ptr), data);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that zero pages are supplied by the kernel for the newly extended range after a resize,
// and are not overwritten by a pager supply.
trap_dirty_test!(resize_supply_zero, zx::VmoOptions::RESIZABLE, |create_option| {
    const NUM_PAGES: u64 = 2;
    const NUM_PAGES_AFTER_RESIZE: u64 = 4;
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(NUM_PAGES, create_option).unwrap();

    // Resize the VMO up.
    assert!(vmo.resize(NUM_PAGES_AFTER_RESIZE));

    // Now try to access all the pages. The first two should result in read requests, but the last
    // two should be supplied with zeros without any read requests.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        let mut data = vec![0u8; NUM_PAGES_AFTER_RESIZE as usize * psu()];
        v.vmo().read(&mut data, 0).is_ok()
    });
    assert!(t.start());

    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    // No more read requests seen for the newly extended range.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    assert!(t.wait());

    // Verify that the last two pages are zeros.
    let mut expected = vec![0u8; NUM_PAGES_AFTER_RESIZE as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, 2, 0);
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // Only two pages should be committed in the VMO.
    assert_eq!(committed_bytes(&vmo), 2 * ps());

    // Supply pages in the newly extended range. This should be a no-op. Since the range is already
    // implicitly "supplied", another supply will be ignored.
    assert!(pager.supply_pages(&vmo, 2, 2));
    assert_eq!(committed_bytes(&vmo), 2 * ps());

    // Verify that the last two pages are still zero.
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // Writes for this case are tested separately in ResizeDirtyRequest. Skip the rest.
    if create_option.contains(zx::VmoOptions::TRAP_DIRTY) {
        return;
    }

    // Write to the last two pages now.
    vmo.vmo().write(&test_fill_pages(2, 0xaa), 2 * ps()).unwrap();

    // All four pages should be committed now.
    assert_eq!(committed_bytes(&vmo), 4 * ps());

    // Verify the contents.
    expected[2 * psu()..4 * psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // The last two pages should be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(2, 2)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

/// Writes `count` pages of `fill` at `page_offset` on a test thread, returning whether the write
/// returned `expected`.
fn write_pages_thread(
    vmo: &Arc<Vmo>,
    page_offset: u64,
    count: usize,
    fill: u8,
    expected: Result<(), zx::Status>,
) -> TestThread {
    let v = vmo.clone();
    TestThread::new(move || {
        v.vmo().write(&test_fill_pages(count, fill), page_offset * ps()) == expected
    })
}

// Tests that writing to the newly extended range after a resize can generate DIRTY requests as
// expected.
#[test]
fn resize_dirty_request() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(2, zx::VmoOptions::TRAP_DIRTY | zx::VmoOptions::RESIZABLE)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 2));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Now try to write pages 1 and 2. We should see dirty requests for both.
    let mut t1 = write_pages_thread(&vmo, 1, 2, 0xaa, Ok(()));
    assert!(t1.start());
    assert!(t1.wait_for_blocked());

    // No read requests seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Dirty request seen for the entire write range.
    assert!(pager.wait_for_page_dirty(&vmo, 1, 2, INFINITE));
    assert!(pager.dirty_pages(&vmo, 1, 2));

    assert!(t1.wait());

    // Verify the VMO contents. (Allocate a buffer large enough to reuse across all resizes.)
    let mut expected = vec![0u8; 8 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[psu()..3 * psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert_eq!(committed_bytes(&vmo), 3 * ps());

    // Verify that pages 1 and 2 are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 2)]));

    // Resize the VMO up again, and try writing to a page after a gap.
    assert!(vmo.resize(6));

    // Write to page 4.
    let mut t2 = write_pages_thread(&vmo, 4, 1, 0xbb, Ok(()));
    assert!(t2.start());
    assert!(t2.wait_for_blocked());

    // No read requests seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // We should only see a dirty request for page 4.
    assert!(pager.wait_for_page_dirty(&vmo, 4, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 4, 1));

    assert!(t2.wait());

    // Verify the contents again.
    expected[4 * psu()..5 * psu()].fill(0xbb);
    assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

    // Verify dirty ranges.
    let ranges = [range(1, 2), zero_range(3, 1), range(4, 1), zero_range(5, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // Resize up again, and try writing to the entire VMO at once.
    assert!(vmo.resize(8));

    let mut t3 = write_pages_thread(&vmo, 0, 8, 0xcc, Ok(()));
    assert!(t3.start());
    assert!(t3.wait_for_blocked());

    // No read requests seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // We should see a dirty request for page 0.
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));

    // We should see a dirty request for page 3.
    assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 3, 1));

    // We should see a dirty request for pages 5,6,7.
    assert!(pager.wait_for_page_dirty(&vmo, 5, 3, INFINITE));
    assert!(pager.dirty_pages(&vmo, 5, 3));

    assert!(t3.wait());

    // Verify the contents.
    expected.fill(0xcc);
    assert!(check_buffer_data(&vmo, 0, 8, &expected, true));

    // Verify that all the pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 8)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that writeback on a resized VMO works as expected.
trap_dirty_test!(resize_writeback, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Write to the first and the last page, leaving a gap in between.
    write_pages_acknowledging(&pager, &vmo, create_option, &[0, 2], 0xaa);

    // Verify VMO contents.
    let mut expected = vec![0xaau8; 3 * psu()];
    expected[psu()..2 * psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // Verify that all the pages are dirty.
    let ranges_before = [range(0, 1), zero_range(1, 1), range(2, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges_before));

    // Attempt to writeback all the pages.
    assert!(pager.writeback_begin_pages(&vmo, 0, 3));
    assert!(pager.writeback_end_pages(&vmo, 0, 3));

    // All pages should be clean now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Verify VMO contents.
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a resize down unblocks outstanding DIRTY requests that are out-of-bounds.
#[test]
fn resize_with_outstanding_dirty_requests() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(5, zx::VmoOptions::RESIZABLE | zx::VmoOptions::TRAP_DIRTY)
        .unwrap();

    // Supply page 1 as a zero page marker.
    let vmo_src = zx::Vmo::create(ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &vmo_src, 1, 1);

    // Supply page 3 as an actual page.
    assert!(pager.supply_pages(&vmo, 3, 1));

    // Resize the VMO up so there's a non-zero range that will be supplied as zero.
    assert!(vmo.resize(6));

    // The new "page" at the end should be indicated dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(5, 1)]));

    let out_of_range = Err(zx::Status::OUT_OF_RANGE);
    // Try to write to page 1 which is a zero marker.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[0xaa], ps()) == out_of_range);

    // Try to write to page 3 which is an actual clean page.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.vmo().write(&[0xbb], 3 * ps()) == out_of_range);

    // Try to write to page 5 which is a gap in the newly extended range.
    let v = vmo.clone();
    let mut t3 = TestThread::new(move || v.vmo().write(&[0xcc], 5 * ps()) == out_of_range);

    // Try to read page 2 which is a non-resident page.
    let v = vmo.clone();
    let mut t4 = TestThread::new(move || v.vmo().read(&mut [0u8], 2 * ps()) == out_of_range);

    // All four threads should block.
    assert!(t1.start());
    assert!(t2.start());
    assert!(t3.start());
    assert!(t4.start());

    // We should see dirty requests for pages 1, 3 and 5.
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
    assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
    assert!(pager.wait_for_page_dirty(&vmo, 5, 1, INFINITE));

    // We should see a read request for page 2.
    assert!(pager.wait_for_page_read(&vmo, 2, 1, INFINITE));

    // No more requests seen.
    assert_no_requests(&pager, &vmo);

    // Now resize down so that the pages all four threads are waiting for become out-of-bounds.
    assert!(vmo.resize(1));

    // All four threads should now see ZX_ERR_OUT_OF_RANGE returned for their reads/writes.
    assert!(t1.wait());
    assert!(t2.wait());
    assert!(t3.wait());
    assert!(t4.wait());

    // Trying to resolve the dirty and read requests we previously saw should fail.
    assert!(!pager.dirty_pages(&vmo, 1, 1));
    assert!(!pager.dirty_pages(&vmo, 3, 1));
    assert!(!pager.dirty_pages(&vmo, 5, 1));
    assert!(!pager.supply_pages(&vmo, 2, 1));

    // The VMO has no dirty pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that a resize down unblocks outstanding DIRTY requests that are out-of-bounds when the
// out-of-bounds range is in the process of being written back.
#[test]
fn resize_writeback_with_outstanding_dirty_requests() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(2, zx::VmoOptions::RESIZABLE | zx::VmoOptions::TRAP_DIRTY)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 2));

    // Resize the VMO up.
    assert!(vmo.resize(5));

    // Write to a page leaving a gap beyond the old size.
    let mut t1 = write_pages_thread(&vmo, 4, 1, 0xaa, Ok(()));
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 4, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 4, 1));
    assert!(t1.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(2, 2), range(4, 1)]));

    let mut expected = vec![0u8; 5 * psu()];
    vmo.generate_buffer_contents(&mut expected, 2, 0);
    expected[4 * psu()..].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 5, &expected, true));

    // Begin writeback for all the dirty pages. This will result in DIRTY requests if they are
    // written again.
    assert!(pager.writeback_begin_pages(&vmo, 2, 3));

    // Try to write to pages 1 and 2. This will trigger a DIRTY request.
    let mut t2 = write_pages_thread(&vmo, 1, 2, 0xbb, Err(zx::Status::OUT_OF_RANGE));
    assert!(t2.start());
    assert!(pager.wait_for_page_dirty(&vmo, 1, 2, INFINITE));

    // Try to write to pages 3 and 4. This will also trigger a DIRTY request.
    let mut t3 = write_pages_thread(&vmo, 3, 2, 0xcc, Err(zx::Status::OUT_OF_RANGE));
    assert!(t3.start());
    assert!(pager.wait_for_page_dirty(&vmo, 3, 2, INFINITE));

    // Complete writeback for the start of the dirty range so that the zero tail can be advanced.
    // This will give us a gap before the tail. Now we will be able to test all four cases - a
    // non-dirty page before the tail, a gap before the tail, a non-dirty page after the tail, and
    // a gap after the tail.
    assert!(pager.writeback_end_pages(&vmo, 2, 1));

    // Resize down so that both the DIRTY requests are now out of bounds.
    assert!(vmo.resize(1));

    // Wait for the threads to complete.
    assert!(t2.wait());
    assert!(t3.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // End the remaining range of the writeback we began previously. This will fail as it is out
    // of bounds.
    assert!(!pager.writeback_end_pages(&vmo, 3, 2));

    // Verify dirty ranges and VMO contents again.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that writing again to resized range that is being written back triggers new DIRTY
// requests.
#[test]
fn resize_writeback_new_dirty_requests_interleaved() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(1, zx::VmoOptions::RESIZABLE | zx::VmoOptions::TRAP_DIRTY)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Write to a page leaving a gap.
    let mut t1 = write_pages_thread(&vmo, 2, 1, 0xaa, Ok(()));
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 2, 1));
    assert!(t1.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1), range(2, 1)]));

    let mut expected = vec![0u8; 3 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // Beging writeback for all the dirty pages.
    assert!(pager.writeback_begin_pages(&vmo, 1, 2));

    // Try to write to page 1. This will trigger a DIRTY request.
    let mut t2 = write_pages_thread(&vmo, 1, 1, 0xbb, Ok(()));
    assert!(t2.start());
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));

    // Try to write to page 2. This will trigger a DIRTY request.
    let mut t3 = write_pages_thread(&vmo, 2, 1, 0xcc, Ok(()));
    assert!(t3.start());
    assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));

    // Resolve the DIRTY requests and wait for the threads to complete.
    assert!(pager.dirty_pages(&vmo, 1, 2));
    assert!(t2.wait());
    assert!(t3.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 2)]));

    expected[psu()..2 * psu()].fill(0xbb);
    expected[2 * psu()..].fill(0xcc);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // End the writeback we began previously. This will be a no-op as both pages were dirtied
    // again.
    assert!(pager.writeback_end_pages(&vmo, 1, 2));

    // Verify dirty ranges and VMO contents again.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 2)]));
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // Should be able to write to the two dirty pages again without blocking.
    vmo.vmo().write(&test_fill_pages(2, 0xdd), ps()).unwrap();

    // Verify dirty ranges and VMO contents again.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 2)]));
    expected[psu()..].fill(0xdd);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that writing again to a written back resized range triggers new DIRTY requests.
#[test]
fn resize_writeback_new_dirty_requests() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(1, zx::VmoOptions::RESIZABLE | zx::VmoOptions::TRAP_DIRTY)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Write to a page leaving a gap.
    let mut t1 = write_pages_thread(&vmo, 2, 1, 0xaa, Ok(()));
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 2, 1));
    assert!(t1.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1), range(2, 1)]));

    let mut expected = vec![0u8; 3 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // Writeback all the dirty pages.
    assert!(pager.writeback_begin_pages(&vmo, 1, 2));
    assert!(pager.writeback_end_pages(&vmo, 1, 2));

    // No dirty ranges remaining.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Try to write to page 1. This will trigger a DIRTY request.
    let mut t2 = write_pages_thread(&vmo, 1, 1, 0xbb, Ok(()));
    assert!(t2.start());
    // This was a gap that we've written back. So we'll first need to supply the page.
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));

    // Try to write to page 2. This will trigger a DIRTY request.
    let mut t3 = write_pages_thread(&vmo, 2, 1, 0xcc, Ok(()));
    assert!(t3.start());
    assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));

    // Resolve the DIRTY requests and wait for the threads to complete.
    assert!(pager.dirty_pages(&vmo, 1, 2));
    assert!(t2.wait());
    assert!(t3.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 2)]));

    expected[psu()..2 * psu()].fill(0xbb);
    expected[2 * psu()..].fill(0xcc);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

fn traps_dirty(create_option: zx::VmoOptions) -> bool {
    create_option.contains(zx::VmoOptions::TRAP_DIRTY)
}

// Tests that a write interleaved with a writeback retains the dirtied page that falls in the zero
// range being written back.
trap_dirty_test!(resize_writeback_intersecting_write, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(4));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 3)]));

    // Start writeback for the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 3));

    // Write to a page in the range.
    let mut t1 = write_pages_thread(&vmo, 2, 1, 0xaa, Ok(()));

    assert!(t1.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 2, 1));
    }
    assert!(t1.wait());

    // Verify VMO contents.
    let mut expected = vec![0u8; 4 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..3 * psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // Verify that the last three pages are dirty.
    let ranges_before = [zero_range(1, 1), range(2, 1), zero_range(3, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges_before));

    // End the writeback that we began previously.
    assert!(pager.writeback_end_pages(&vmo, 1, 3));

    // We should not have been able to clean the page that was dirtied after beginning the
    // writeback.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(2, 1)]));

    // Now attempt a writeback again for the entire VMO.
    assert!(pager.writeback_begin_pages(&vmo, 0, 4));
    assert!(pager.writeback_end_pages(&vmo, 0, 4));

    // All pages should be clean now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Verify VMO contents.
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a write outside of an awaiting clean zero range does not affect it.
trap_dirty_test!(
    resize_writeback_non_intersecting_write,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        assert!(vmo.resize(4));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 3)]));

        // Start writeback for a portion of the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, 2));

        // Write to a page following the awaiting clean range.
        let mut t1 = write_pages_thread(&vmo, 3, 1, 0xaa, Ok(()));

        assert!(t1.start());
        if traps_dirty(create_option) {
            assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 3, 1));
        }
        assert!(t1.wait());

        // Write to a page preceding the awaiting clean range.
        let mut t2 = write_pages_thread(&vmo, 0, 1, 0xbb, Ok(()));

        assert!(t2.start());
        if traps_dirty(create_option) {
            assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 0, 1));
        }
        assert!(t2.wait());

        // Verify VMO contents.
        let mut expected = vec![0u8; 4 * psu()];
        expected[..psu()].fill(0xbb);
        expected[3 * psu()..].fill(0xaa);
        assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

        // Verify that all of the pages are dirty.
        let ranges_before = [range(0, 1), zero_range(1, 2), range(3, 1)];
        assert!(pager.verify_dirty_ranges(&vmo, &ranges_before));

        // End the writeback that we began previously.
        assert!(pager.writeback_end_pages(&vmo, 1, 2));

        // The range that was written back should be clean now. The pages that were written should be
        // dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1), range(3, 1)]));

        // Attempt another writeback for the entire VMO.
        assert!(pager.writeback_begin_pages(&vmo, 0, 4));
        assert!(pager.writeback_end_pages(&vmo, 0, 4));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

/// Picks random pages in `[first, last]` to write to, returning them as runs of `(offset,
/// length)`.
fn random_write_runs(first: u64, last: u64) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    let mut len = 0;
    for i in first..=last {
        let write = !rand().is_multiple_of(2);
        if write {
            // Begin a new range if we're not accumulated any pages to write yet.
            if len == 0 {
                runs.push((i, 0));
            }
            // Add to the running length.
            len += 1;
            runs.last_mut().unwrap().1 = len;
        } else {
            // A break in the run of pages to be written.
            len = 0;
        }
    }
    runs
}

/// Writes `runs` of pages of `fill` on a test thread, acknowledging the resulting DIRTY requests
/// if `create_option` traps dirty transitions.
fn write_runs(
    pager: &UserPager,
    vmo: &Arc<Vmo>,
    create_option: zx::VmoOptions,
    runs: &[(u64, u64)],
    fill: u8,
) {
    let (v, thread_runs) = (vmo.clone(), runs.to_vec());
    let mut t = TestThread::new(move || {
        thread_runs.iter().all(|&(offset, length)| {
            v.vmo().write(&test_fill_pages(length as usize, fill), offset * ps()).is_ok()
        })
    });

    assert!(t.start());
    if traps_dirty(create_option) {
        for &(offset, length) in runs {
            assert!(pager.wait_for_page_dirty(vmo, offset, length, INFINITE));
            assert!(pager.dirty_pages(vmo, offset, length));
        }
    }
    assert!(t.wait());
}

// Tests extending the VMO, starting a writeback on the extended range, and then writing to random
// pages in the extended range before ending the writeback.
trap_dirty_test!(
    resize_writeback_intersecting_random_writes,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        const EXTENDED_LEN: u64 = 40;
        assert!(vmo.resize(1 + EXTENDED_LEN));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, EXTENDED_LEN)]));

        // Start writeback for the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, EXTENDED_LEN));

        // Write to random pages in the range.
        let runs = random_write_runs(1, EXTENDED_LEN);
        write_runs(&pager, &vmo, create_option, &runs, 0xaa);

        // End the writeback that we began previously.
        assert!(pager.writeback_end_pages(&vmo, 1, EXTENDED_LEN));

        // We should not have been able to clean the dirtied pages but we should be able to clean
        // the zero ranges remaining. No dirty range will be a zero range.
        let ranges: Vec<_> = runs.iter().map(|&(offset, length)| range(offset, length)).collect();
        assert!(pager.verify_dirty_ranges(&vmo, &ranges));

        // Now attempt a writeback again for the entire VMO.
        assert!(pager.writeback_begin_pages(&vmo, 0, EXTENDED_LEN + 1));
        assert!(pager.writeback_end_pages(&vmo, 0, EXTENDED_LEN + 1));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests extending the VMO, starting a writeback on only a portion of the extended range, and then
// writing to random pages in the extended range before ending the writeback.
trap_dirty_test!(
    resize_partial_writeback_intersecting_random_writes,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        const EXTENDED_LEN: u64 = 40;
        assert!(vmo.resize(1 + EXTENDED_LEN));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, EXTENDED_LEN)]));

        // Start writeback for only a portion of the dirty zero range.
        const WRITEBACK_LEN: u64 = EXTENDED_LEN / 2;
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, WRITEBACK_LEN));

        // Write to random pages in the range.
        let runs = random_write_runs(1, EXTENDED_LEN);
        write_runs(&pager, &vmo, create_option, &runs, 0xaa);

        // End the writeback that we began previously.
        assert!(pager.writeback_end_pages(&vmo, 1, WRITEBACK_LEN));

        // We should not have been able to clean the dirtied pages but we should be able to clean
        // the zero ranges that fell in the written back range.
        let mut ranges: Vec<zx_vmo_dirty_range_t> = Vec::new();
        for &(offset, length) in &runs {
            if offset > WRITEBACK_LEN + 1 {
                let prev_end = ranges.last().map_or(0, |r| r.offset + r.length);
                let start = prev_end.max(WRITEBACK_LEN + 1);
                ranges.push(zero_range(start, offset - start));
            }
            ranges.push(range(offset, length));
        }
        let last_range = *ranges.last().unwrap();
        if last_range.offset + last_range.length < EXTENDED_LEN + 1 {
            let start = last_range.offset + last_range.length;
            ranges.push(zero_range(start, EXTENDED_LEN + 1 - start));
        }
        assert!(pager.verify_dirty_ranges(&vmo, &ranges));

        // Now attempt a writeback again for the entire VMO.
        assert!(pager.writeback_begin_pages(&vmo, 0, EXTENDED_LEN + 1));
        assert!(pager.writeback_end_pages(&vmo, 0, EXTENDED_LEN + 1));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests that a resize interleaved with a writeback trims / resets an awaiting clean zero range if
// it intersects it.
trap_dirty_test!(
    resize_writeback_intersecting_resize,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        assert!(vmo.resize(3));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

        // Verify VMO contents.
        let mut expected = vec![0u8; 3 * psu()];
        vmo.generate_buffer_contents(&mut expected, 1, 0);
        assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

        // Start writeback for the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, 2));

        // Resize the VMO down, so that part of the dirty range is still valid.
        assert!(vmo.resize(2));

        // Verify that the second page is still dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

        // Try to end the writeback that we began previously. This should fail as it is out of bounds.
        assert!(!pager.writeback_end_pages(&vmo, 1, 2));

        // Verify that the second page is still dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

        // End the writeback with the correct length.
        assert!(pager.writeback_end_pages(&vmo, 1, 1));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Resize the VMO up again.
        assert!(vmo.resize(3));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(2, 1)]));

        // Supply the second page as it has already been written back, and the user pager is expected
        // to supply it.
        // TODO(rashaeqbal): Supply with zeros once we have a quick OP_SUPPLY_ZERO. For now just supply
        // non-zero content; the content is irrelevant for this test.
        assert!(pager.supply_pages(&vmo, 1, 1));
        vmo.generate_buffer_contents(&mut expected[psu()..], 1, 1);

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

        // Start writeback for the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 2, 1));

        // Resize the VMO down, so that the entire dirty range is invalid.
        assert!(vmo.resize(2));

        // No pages should be dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Ending the writeback we began should fail as it is out of bounds.
        assert!(!pager.writeback_end_pages(&vmo, 2, 1));

        // All pages are clean.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

        // Resize the VMO up again.
        assert!(vmo.resize(3));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(2, 1)]));

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

        // Start writeback for the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 2, 1));

        // Resize the VMO down even further to before the start of the dirty range.
        assert!(vmo.resize(1));

        // No pages should be dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Ending the writeback we began should fail as it is out of bounds.
        assert!(!pager.writeback_end_pages(&vmo, 2, 1));

        // All pages are clean.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // Verify VMO contents.
        assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests that a resize beyond an awaiting clean zero range does not affect it.
trap_dirty_test!(
    resize_writeback_non_intersecting_resize,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        assert!(vmo.resize(3));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

        // Start writeback for a portion of the range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, 1));

        // Resize the VMO down, so that the new size falls beyond the awaiting clean range.
        assert!(vmo.resize(2));

        // Verify that the second page is still dirty.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

        // Try to end the writeback that we began previously. This should succeed as the resize did
        // not affect it.
        assert!(pager.writeback_end_pages(&vmo, 1, 1));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests that writeback on a resized range that starts after a gap (zero range) is handled.
trap_dirty_test!(resize_writeback_after_gap, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

    // Write to page 2 leaving a gap.
    let mut t1 = write_pages_thread(&vmo, 2, 1, 0xaa, Ok(()));

    assert!(t1.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 2, 1));
    }
    assert!(t1.wait());

    // Verify VMO contents.
    let mut expected = vec![0u8; 3 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // Verify dirty ranges.
    let ranges = [zero_range(1, 1), range(2, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // Attempt writeback page 2, leaving a gap at 1.
    assert!(pager.writeback_begin_pages(&vmo, 2, 1));
    assert!(pager.writeback_end_pages(&vmo, 2, 1));

    // We should be able to clean this page, leaving only one dirty range.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Since we cleaned the page, we should see a DIRTY request on another write (if applicable).
    let mut t2 = write_pages_thread(&vmo, 2, 1, 0xbb, Ok(()));

    assert!(t2.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 2, 1));
    }
    assert!(t2.wait());

    // Verify dirty ranges. Page 2 should be dirty again.
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // Verify VMO contents.
    expected[2 * psu()..].fill(0xbb);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

/// Writes one page of `fill` at each of `page_offsets` on a test thread, acknowledging the
/// resulting DIRTY requests if `create_option` traps dirty transitions.
fn write_pages_acknowledging(
    pager: &UserPager,
    vmo: &Arc<Vmo>,
    create_option: zx::VmoOptions,
    page_offsets: &[u64],
    fill: u8,
) {
    let runs: Vec<_> = page_offsets.iter().map(|&offset| (offset, 1)).collect();
    write_runs(pager, vmo, create_option, &runs, fill);
}

/// Maps `num_pages` of `vmo` readable, writable and allowing faults, for a resizable VMO.
fn map_rw_allow_faults(vmo: &Vmo, num_pages: u64) -> (usize, Unmap) {
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::ALLOW_FAULTS;
    let ptr = root_vmar().map(0, vmo.vmo(), 0, num_pages as usize * psu(), flags).unwrap();
    (ptr, Unmap(Arc::new(AtomicUsize::new(ptr)), num_pages * ps()))
}

// Tests that writeback on a resized range with multiple zero ranges (gaps) can clean all the gaps.
trap_dirty_test!(resize_writeback_multiple_gaps, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(6));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 5)]));

    // Write to pages 2 and 4, leaving gaps at 1, 3, and 5.
    write_pages_acknowledging(&pager, &vmo, create_option, &[2, 4], 0xaa);

    // Verify VMO contents.
    let mut expected = vec![0u8; 6 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..3 * psu()].fill(0xaa);
    expected[4 * psu()..5 * psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

    // Verify dirty ranges.
    let ranges_before =
        [zero_range(1, 1), range(2, 1), zero_range(3, 1), range(4, 1), zero_range(5, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges_before));

    // Begin writeback for all the dirty pages.
    assert!(pager.writeback_begin_pages(&vmo, 1, 5));

    // Writing to the AwaitingClean pages should trigger DIRTY requests, and so should writing to
    // gaps.
    let mut t2 = write_pages_thread(&vmo, 3, 2, 0xbb, Ok(()));

    assert!(t2.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 3, 2, INFINITE));
        assert!(pager.dirty_pages(&vmo, 3, 2));
    }
    assert!(t2.wait());

    // Complete the writeback we started.
    assert!(pager.writeback_end_pages(&vmo, 1, 5));

    // We should have been able to clean everything except the pages we just dirtied.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(3, 2)]));

    expected[3 * psu()..5 * psu()].fill(0xbb);
    assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests starting multiple sequential writebacks on the resized range, both for gaps and pages.
trap_dirty_test!(resize_writeback_sequential, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(6));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 5)]));

    // Write to pages 2 and 4, leaving gaps at 1, 3, and 5.
    write_pages_acknowledging(&pager, &vmo, create_option, &[2, 4], 0xaa);

    // Verify VMO contents.
    let mut expected = vec![0u8; 6 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[2 * psu()..3 * psu()].fill(0xaa);
    expected[4 * psu()..5 * psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

    // Verify dirty ranges.
    let ranges = [zero_range(1, 1), range(2, 1), zero_range(3, 1), range(4, 1), zero_range(5, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // Begin writeback for all the dirty ranges.
    for range in &ranges {
        if range.options == zx::sys::ZX_VMO_DIRTY_RANGE_IS_ZERO {
            assert!(pager.writeback_begin_zero_pages(&vmo, range.offset, range.length));
        } else {
            assert!(pager.writeback_begin_pages(&vmo, range.offset, range.length));
        }
    }

    // End writeback for all the dirty ranges.
    for range in &ranges {
        assert!(pager.writeback_end_pages(&vmo, range.offset, range.length));
    }

    // All pages should be clean.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a WritebackBegin on a resized range followed by a partial WritebackEnd works as
// expected.
trap_dirty_test!(resize_writeback_partial_end, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(5));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 4)]));

    // Writeback only a portion of the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 1));
    assert!(pager.writeback_end_pages(&vmo, 1, 1));

    // Verify that the written back portion has been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(2, 3)]));

    // Writeback another portion of the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 2, 1));
    assert!(pager.writeback_end_pages(&vmo, 2, 1));

    // Verify that the written back portion has been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(3, 2)]));

    // Writeback the remaining portion of the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 3, 2));
    assert!(pager.writeback_end_pages(&vmo, 3, 2));

    // Verify that all pages are clean now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests repeated writebacks on a resized range.
trap_dirty_test!(resize_writeback_repeated, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(5));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 4)]));

    // Start writeback for the entire zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 4));

    // Start another writeback but for a smaller sub-range. This should not override the previous
    // writeback.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 2));

    // Now try to end the first writeback we started.
    assert!(pager.writeback_end_pages(&vmo, 1, 4));

    // We should have been able to clean all the pages.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the VMO again so we have another dirty zero range.
    assert!(vmo.resize(10));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(5, 5)]));

    // End the second writeback we started. This should be a no-op.
    assert!(pager.writeback_end_pages(&vmo, 1, 2));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(5, 5)]));

    // Attempting to end the writeback without starting another one should have no effect.
    assert!(pager.writeback_end_pages(&vmo, 5, 2));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(5, 5)]));

    // Begin another writeback.
    assert!(pager.writeback_begin_zero_pages(&vmo, 5, 2));
    // Starting a redundant writeback for the same range should be a no-op.
    assert!(pager.writeback_begin_zero_pages(&vmo, 5, 2));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(5, 5)]));

    // Begin another writeback for the remaining range. This should have no effect.
    assert!(pager.writeback_begin_zero_pages(&vmo, 7, 3));

    // End the first writeback.
    assert!(pager.writeback_end_pages(&vmo, 5, 2));

    // End the second writeback.
    assert!(pager.writeback_end_pages(&vmo, 7, 3));

    // Only the first portion should have been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(7, 3)]));

    // End the redundant writeback we started. This should be a no-op.
    assert!(pager.writeback_end_pages(&vmo, 5, 2));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(7, 3)]));

    // Writeback the remaining range.
    assert!(pager.writeback_begin_pages(&vmo, 7, 3));
    assert!(pager.writeback_end_pages(&vmo, 7, 3));

    // Verify that all pages are clean now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a resized range that has mappings can be written back as expected.
trap_dirty_test!(resize_writeback_with_mapping, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(2));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Map the resized VMO.
    let (ptr, _unmap) = map_rw_allow_faults(&vmo, 2);

    // Commit a page in the resized range.
    let mut t1 = TestThread::new(move || {
        write_byte(ptr + psu(), 0xaa);
        true
    });

    assert!(t1.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 1, 1));
    }
    assert!(t1.wait());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 1)]));

    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[psu()] = 0xaa;
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // Writeback the VMO.
    assert!(pager.writeback_begin_pages(&vmo, 0, 2));
    assert!(pager.writeback_end_pages(&vmo, 0, 2));

    // Verify that all pages are clean.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Trying to write to the committed page again should trap as write permissions will have been
    // cleared.
    let mut t2 = TestThread::new(move || {
        write_byte(ptr + psu(), 0xbb);
        true
    });

    assert!(t2.start());
    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 1, 1));
    }
    assert!(t2.wait());

    // The page should now be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 1)]));

    // Verify VMO contents.
    expected[psu()] = 0xbb;
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a resized range that has mappings and is in the process of being written back is
// dirtied again on a write.
trap_dirty_test!(
    resize_writeback_interleaved_write_with_mapping,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
        assert!(pager.supply_pages(&vmo, 0, 1));

        // Resize the VMO up.
        assert!(vmo.resize(6));

        // Newly extended range should be dirty and zero.
        assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 5)]));

        // Map the resized VMO.
        let (ptr, _unmap) = map_rw_allow_faults(&vmo, 6);

        // Begin a writeback for the dirty zero range.
        assert!(pager.writeback_begin_zero_pages(&vmo, 1, 5));

        // Write to two pages in the resized range leaving gaps.
        let mut t1 = TestThread::new(move || {
            write_byte(ptr + 2 * psu(), 0xaa);
            write_byte(ptr + 4 * psu(), 0xaa);
            true
        });

        assert!(t1.start());
        if traps_dirty(create_option) {
            assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 2, 1));
            assert!(pager.wait_for_page_dirty(&vmo, 4, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 4, 1));
        }
        assert!(t1.wait());

        // Verify dirty ranges.
        let ranges1 =
            [zero_range(1, 1), range(2, 1), zero_range(3, 1), range(4, 1), zero_range(5, 1)];
        assert!(pager.verify_dirty_ranges(&vmo, &ranges1));

        // Verify VMO contents.
        let mut expected = vec![0u8; 6 * psu()];
        vmo.generate_buffer_contents(&mut expected, 1, 0);
        expected[2 * psu()] = 0xaa;
        expected[4 * psu()] = 0xaa;
        assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

        // We should be able to write to the two committed pages again without blocking as they
        // were dirtied after beginning the writeback.
        write_byte(ptr + 2 * psu(), 0xbb);
        write_byte(ptr + 4 * psu(), 0xbb);

        // Verify dirty ranges and VMO contents.
        assert!(pager.verify_dirty_ranges(&vmo, &ranges1));
        expected[2 * psu()] = 0xbb;
        expected[4 * psu()] = 0xbb;
        assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

        // End the writeback we started previously. We should only have been able to clean the
        // gaps.
        assert!(pager.writeback_end_pages(&vmo, 1, 5));
        assert!(pager.verify_dirty_ranges(&vmo, &[range(2, 1), range(4, 1)]));

        // Try to write to a gap. This should block as well.
        let mut t3 = TestThread::new(move || {
            write_byte(ptr + 3 * psu(), 0xdd);
            true
        });
        assert!(t3.start());

        assert!(pager.wait_for_page_read(&vmo, 3, 1, INFINITE));
        assert!(pager.supply_pages(&vmo, 3, 1));

        if traps_dirty(create_option) {
            assert!(pager.wait_for_page_dirty(&vmo, 3, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 3, 1));
        }
        assert!(t3.wait());

        // Verify dirty ranges.
        assert!(pager.verify_dirty_ranges(&vmo, &[range(2, 3)]));

        // Verify VMO contents.
        vmo.generate_buffer_contents(&mut expected[3 * psu()..], 1, 3);
        expected[3 * psu()] = 0xdd;
        assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

        // Writeback the dirty ranges.
        assert!(pager.writeback_begin_pages(&vmo, 2, 3));
        assert!(pager.writeback_end_pages(&vmo, 2, 3));

        // All pages should be clean now.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));
        assert!(check_buffer_data(&vmo, 0, 6, &expected, true));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests that writing a page after a dirty zero range is queried but before it is written back is
// left dirty.
trap_dirty_test!(resize_writeback_dirty_after_query, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(4));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 3)]));

    // Write a page in the dirty zero range so that a page is committed.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[0xaa], 2 * ps()).is_ok());
    assert!(t.start());

    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 2, 1));
    }

    assert!(t.wait());

    // Writeback the dirty zero range we previously queried, explicitly stating that we will be
    // writing back zeroes.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 3));
    assert!(pager.writeback_end_pages(&vmo, 1, 3));

    // The writeback should have left the dirty (non-zero) page dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(2, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

/// Creates a VMO of one supplied page, resizes it up to 10 pages, and dirties pages {1, 6, 9} so
/// that the newly extended dirty zero range is fragmented, as the tests of combined and out of
/// order writebacks start. Returns the VMO and its dirty ranges.
fn fragmented_dirty_zero_range(
    pager: &mut UserPager,
    create_option: zx::VmoOptions,
) -> (Arc<Vmo>, [zx_vmo_dirty_range_t; 5]) {
    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(10));

    // Newly extended range should be dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 9)]));

    // Dirty a few pages so that the zero range is fragmented. Pages dirtied are {1, 6, 9}.
    let v = vmo.clone();
    let mut t =
        TestThread::new(move || [1, 6, 9].iter().all(|i| v.vmo().write(&[0xaa], i * ps()).is_ok()));
    assert!(t.start());

    if traps_dirty(create_option) {
        for i in [1, 6, 9] {
            assert!(pager.wait_for_page_dirty(&vmo, i, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, i, 1));
        }
    }

    assert!(t.wait());

    // Verify dirty ranges.
    let ranges = [range(1, 1), zero_range(2, 4), range(6, 1), zero_range(7, 2), range(9, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));
    (vmo, ranges)
}

// Verifies that multiple dirty ranges (zero and non-zero) can be written back together.
trap_dirty_test!(writeback_combined, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let (vmo, _) = fragmented_dirty_zero_range(&mut pager, create_option);

    // Writeback the entire range together.
    assert!(pager.writeback_begin_pages(&vmo, 1, 9));
    assert!(pager.writeback_end_pages(&vmo, 1, 9));

    // All dirty ranges have been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
});

/// The order the out of order writeback tests write back the dirty ranges in.
const OUT_OF_ORDER_INDICES: [usize; 5] = [3, 0, 2, 1, 4];

// Verifies that writing back dirty ranges (both zero and non-zero) out of order cleans
// everything.
trap_dirty_test!(writeback_out_of_order, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let (vmo, ranges) = fragmented_dirty_zero_range(&mut pager, create_option);

    // Writeback the dirty ranges out of order.
    // Beging all the writebacks first.
    for i in OUT_OF_ORDER_INDICES {
        assert!(pager.writeback_begin_pages(&vmo, ranges[i].offset, ranges[i].length));
    }
    // End all the writebacks.
    for i in OUT_OF_ORDER_INDICES {
        assert!(pager.writeback_end_pages(&vmo, ranges[i].offset, ranges[i].length));
    }

    // All dirty ranges have been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
});

// Verifies starting writeback on ranges out of order and ending all together.
trap_dirty_test!(writeback_out_of_order_combined_end, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let (vmo, ranges) = fragmented_dirty_zero_range(&mut pager, create_option);

    // Writeback the dirty ranges out of order.
    // Begin writeback for all the dirty ranges, and end writeback with a single call over the
    // range.
    for i in OUT_OF_ORDER_INDICES {
        assert!(pager.writeback_begin_pages(&vmo, ranges[i].offset, ranges[i].length));
    }
    assert!(pager.writeback_end_pages(&vmo, 1, 9));

    // All dirty ranges have been cleaned.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
});

// Verifies starting writeback on ranges all together and ending out of order.
trap_dirty_test!(
    writeback_out_of_order_combined_begin,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let (vmo, ranges) = fragmented_dirty_zero_range(&mut pager, create_option);

        // Writeback the dirty ranges out of order.
        // Begin writeback with a single call and then end writeback for each range.
        assert!(pager.writeback_begin_pages(&vmo, 1, 9));
        for i in OUT_OF_ORDER_INDICES {
            assert!(pager.writeback_end_pages(&vmo, ranges[i].offset, ranges[i].length));
        }

        // All dirty ranges have been cleaned.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // No more page requests seen.
        assert_no_requests(&pager, &vmo);
    }
);

/// Zeroes `count` pages at `page_offset` of `vmo` with `ZX_VMO_OP_ZERO`.
fn zero_op(vmo: &Vmo, page_offset: u64, count: u64) -> Result<(), zx::Status> {
    vmo.vmo().op_range(zx::VmoOp::ZERO, page_offset * ps(), count * ps())
}

// Test that OP_ZERO writes zeros in a pager-backed VMO.
#[test]
fn op_zero() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(3, zx::VmoOptions::RESIZABLE).unwrap();
    // Supply only one page and let the others be faulted in as required.
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Verify VMO contents for the supplied page.
    let mut expected = vec![0u8; 4 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Verify that no pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Zero the first two pages.
    let v = vmo.clone();
    let mut t = TestThread::new(move || zero_op(&v, 0, 2).is_ok());
    assert!(t.start());

    // We should be able to zero without a read request.
    assert!(t.wait());

    // Verify that the contents are zero.
    expected[..2 * psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // Verify that zero content is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 2)]));

    // Resize the VMO up.
    assert!(vmo.resize(4));

    // Zero the tail end of the VMO that was newly extended. This should be a no-op as it is
    // already zero.
    zero_op(&vmo, 3, 1).unwrap();

    // No pages should be committed as we should have been able to replace the supplied pages with
    // zero intervals.
    assert_eq!(committed_bytes(&vmo), 0);

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 2), zero_range(3, 1)]));
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(check_buffer_data(&vmo, 3, 1, &expected, true));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Test OP_ZERO on a pager-backed VMO created with ZX_VMO_TRAP_DIRTY.
#[test]
fn op_zero_trap_dirty() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(2, zx::VmoOptions::RESIZABLE | zx::VmoOptions::TRAP_DIRTY)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 2));

    // Verify VMO contents.
    let mut expected = vec![0u8; 4 * psu()];
    vmo.generate_buffer_contents(&mut expected, 2, 0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // Verify that no pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Zero the first page.
    let v = vmo.clone();
    let mut t = TestThread::new(move || zero_op(&v, 0, 1).is_ok());
    assert!(t.start());

    // We should be able to zero without a dirty request.
    assert!(t.wait());

    // Verify that the contents are zero.
    expected[..psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // Verify that zero content is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 1)]));

    // Resize the VMO up.
    assert!(vmo.resize(4));

    // Zero the tail end of the VMO that was newly extended. This should be a no-op as it is
    // already zero.
    zero_op(&vmo, 2, 2).unwrap();

    // Only the page that we supplied previously but did not zero should be committed in the VMO.
    assert_eq!(committed_bytes(&vmo), ps());

    // Verify dirty ranges and VMO contents.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 1), zero_range(2, 2)]));
    assert!(check_buffer_data(&vmo, 0, 4, &expected, true));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Test that OP_ZERO is a no-op over a newly extended (but not written back yet) uncommitted
// range.
trap_dirty_test!(op_zero_tail, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 3 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

    // Only the single page we supplied previously should be committed.
    assert_eq!(committed_bytes(&vmo), ps());

    // Zero the newly extended range.
    zero_op(&vmo, 1, 2).unwrap();

    // This should be a no-op and not alter the VMO's pages.
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

    // Only the single page we supplied previously should be committed.
    assert_eq!(committed_bytes(&vmo), ps());

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Test that OP_ZERO can decommit committed pages in a newly extended (but not written back yet)
// range.
trap_dirty_test!(op_zero_decommit, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 3 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

    // Write to a page in the newly extended range leaving a gap
    let data = 0xaau8;
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[data], 2 * ps()).is_ok());
    assert!(t1.start());

    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 2, 1));
    }
    assert!(t1.wait());

    // Verify VMO contents and dirty pages.
    expected[2 * psu()] = data;
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1), range(2, 1)]));

    // Check that two pages are committed.
    assert_eq!(committed_bytes(&vmo), 2 * ps());

    // Now zero the entire VMO.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || zero_op(&v, 0, 3).is_ok());
    assert!(t2.start());

    // We should be able to zero without generating any more DIRTY requests because the tail can
    // simply be advanced from 1 (set during the resize) to 0, indicating that everything from
    // offset 0 is dirty and filled with zeros.
    assert!(t2.wait());

    // Verify that the VMO is now all zeros.
    expected.fill(0);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));

    // We should have been able to decommit all the pages.
    assert_eq!(committed_bytes(&vmo), 0);

    // Verify dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 3)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

const U64_SIZE: u64 = size_of::<u64>() as u64;

// Test OP_ZERO that conflicts with a simultaneous resize.
trap_dirty_test!(op_zero_resize, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(4, create_option).unwrap();
    // Supply a page.
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Verify VMO contents for the supplied page.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // Verify that no pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Start the zeroing partway into page 1, so we can block on a page request for page 1.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || {
        v.vmo().op_range(zx::VmoOp::ZERO, ps() + U64_SIZE, 2 * ps())
            == Err(zx::Status::OUT_OF_RANGE)
    });
    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));

    // End the zeroing partway into page 2, so we can block on a page request for page 2.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || {
        v.vmo().op_range(zx::VmoOp::ZERO, 2 * ps(), ps() - U64_SIZE)
            == Err(zx::Status::OUT_OF_RANGE)
    });
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 2, 1, INFINITE));

    // While the threads are blocked on the page requests, shrink the VMO so that the pages they're
    // requesting are no longer valid. This should unblock the waiting threads and their OP_ZERO
    // should fail with ZX_ERR_OUT_OF_RANGE.
    assert!(vmo.resize(1));
    assert!(t1.wait());
    assert!(t2.wait());

    // Verify VMO contents for the remaining page.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));

    // No more page requests were seen.
    assert_no_requests(&pager, &vmo);
});

// Test OP_ZERO on partial pages.
trap_dirty_test!(op_zero_partial_page, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the VMO up.
    assert!(vmo.resize(2));

    // Verify VMO contents and dirty pages.
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Zero a few bytes in the middle of first page.
    let v = vmo.clone();
    let mut t =
        TestThread::new(move || v.vmo().op_range(zx::VmoOp::ZERO, U64_SIZE, U64_SIZE).is_ok());
    assert!(t.start());

    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }
    assert!(t.wait());

    // Verify VMO contents.
    let u64_size = U64_SIZE as usize;
    expected[u64_size..2 * u64_size].fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // The first page should also be dirty now.
    let ranges = [range(0, 1), zero_range(1, 1)];
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // Zero a range starting partway into the first page and ending before the end of the second
    // page.
    vmo.vmo().op_range(zx::VmoOp::ZERO, ps() - U64_SIZE, ps()).unwrap();

    // Verify VMO contents.
    expected[psu() - u64_size..psu()].fill(0);
    // Verify dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &ranges));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that OP_ZERO just before the tail can efficiently expand the tail and avoid page
// requests.
trap_dirty_test!(op_zero_expands_tail, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();

    // No dirty pages yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the VMO up.
    assert!(vmo.resize(3));

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 3 * psu()];
    assert!(check_buffer_data(&vmo, 2, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(2, 1)]));

    // Zero the second page. We should be able to perform this zeroing efficiently without having
    // to send any page requests.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || zero_op(&v, 1, 1).is_ok());
    assert!(t1.start());

    // No page requests seen.
    assert_no_requests(&pager, &vmo);

    assert!(t1.wait());

    // Verify VMO contents and dirty pages.
    assert!(check_buffer_data(&vmo, 1, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 2)]));

    // Zero the first page partially. Since this is a partial page zero, this will generate page
    // requests.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || {
        v.vmo().op_range(zx::VmoOp::ZERO, ps() - U64_SIZE, U64_SIZE).is_ok()
    });
    assert!(t2.start());

    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));

    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);

    assert!(t2.wait());

    // Verify VMO contents and dirty pages.
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[psu() - U64_SIZE as usize..psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 3, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1), zero_range(1, 2)]));
});

// Tests OP_ZERO with interleaved writeback.
trap_dirty_test!(op_zero_writeback, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the VMO up.
    assert!(vmo.resize(2));

    // Verify VMO contents and dirty pages.
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Begin writeback for the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 1));

    // Zero the first page while the writeback is in progress.
    zero_op(&vmo, 0, 1).unwrap();

    // Try to end the writeback we started. This will be a no-op.
    assert!(pager.writeback_end_pages(&vmo, 1, 1));

    // Verify VMO contents and dirty pages.
    expected[..psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 2)]));

    // Writeback the dirty zero range.
    assert!(pager.writeback_begin_zero_pages(&vmo, 0, 2));
    assert!(pager.writeback_end_pages(&vmo, 0, 2));

    // No dirty pages remain.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No page requests.
    assert_no_requests(&pager, &vmo);
});

// Tests OP_ZERO over zero page markers.
trap_dirty_test!(op_zero_with_markers, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(5, create_option).unwrap();

    // Supply with empty pages so we have zero markers. Insert zero markers at the tail as well as
    // in the middle with a gap.
    let empty_src = zx::Vmo::create(2 * ps()).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));
    supply_zero_pages(&pager, &vmo, &empty_src, 1, 1);
    assert!(pager.supply_pages(&vmo, 2, 1));
    supply_zero_pages(&pager, &vmo, &empty_src, 3, 2);

    // Verify VMO contents and dirty pages.
    let mut expected = vec![0u8; 5 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    vmo.generate_buffer_contents(&mut expected[2 * psu()..], 1, 2);
    assert!(check_buffer_data(&vmo, 0, 5, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Zero the marker in the middle. This should be a no-op.
    zero_op(&vmo, 1, 1).unwrap();

    // No page requests seen.
    assert_no_requests(&pager, &vmo);

    // Verify VMO contents and dirty pages.
    assert!(check_buffer_data(&vmo, 0, 5, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Zero the markers at the end. This should also be a no-op.
    zero_op(&vmo, 3, 2).unwrap();

    // No page requests seen.
    assert_no_requests(&pager, &vmo);

    // Verify VMO contents and dirty pages.
    assert!(check_buffer_data(&vmo, 0, 5, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
});

// Tests that zeroing the tail unblocks any previous read requests.
trap_dirty_test!(op_zero_unblocks_read_request, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();

    // Supply the first page.
    assert!(pager.supply_pages(&vmo, 0, 1));

    // No dirty ranges yet.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Read from the second (and last) page.
    let data = Arc::new(parking_lot::Mutex::new(vec![0xffu8; psu()]));
    let (v, d) = (vmo.clone(), data.clone());
    let mut t = TestThread::new(move || v.vmo().read(&mut d.lock(), ps()).is_ok());
    assert!(t.start());

    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));

    // Now zero the last page.
    zero_op(&vmo, 1, 1).unwrap();

    // This should unblock the previous read request, as the kernel has been able to expand the
    // tail and will supply zeroes for this page from this point on.
    assert!(t.wait());

    // Verify VMO contents.
    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));

    // The last page should be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // The last page should have read as zeroes.
    assert_eq!(*data.lock(), expected[psu()..]);

    // No other page requests seen.
    assert_no_requests(&pager, &vmo);
});

// Tests that dirty pages can be written back after detach.
trap_dirty_test!(writeback_dirty_pages_after_detach, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Write to a page.
    let data = 0xaau8;
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[data], 0).is_ok());
    assert!(t.start());

    if traps_dirty(create_option) {
        // Dirty the page.
        assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }
    assert!(t.wait());

    // We should have committed the page.
    assert_eq!(committed_bytes(&vmo), ps());

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo));
    assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));

    // Verify that the page is still dirty.
    assert_eq!(committed_bytes(&vmo), ps());
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Should be able to read the page and verify its contents.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    expected[0] = data;
    // We should be able to read the dirty range both through mappings and with a VMO read.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(check_buffer_data(&vmo, 0, 1, &expected, false));

    // Writeback the page.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));
    assert!(pager.writeback_end_pages(&vmo, 0, 1));

    // Verify that the page is clean now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a newly resized range can be written back after detach.
trap_dirty_test!(
    writeback_resized_range_after_detach,
    zx::VmoOptions::RESIZABLE,
    |create_option| {
        let mut pager = UserPager::new();
        assert!(pager.init());

        let vmo = pager.create_vmo_with_options(1, create_option).unwrap();

        // Resize the VMO up and write a page leaving a gap.
        assert!(vmo.resize(3));

        let data = 0xbbu8;
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.vmo().write(&[data], 2 * ps()).is_ok());
        assert!(t.start());

        if traps_dirty(create_option) {
            // Dirty the page.
            assert!(pager.wait_for_page_dirty(&vmo, 2, 1, INFINITE));
            assert!(pager.dirty_pages(&vmo, 2, 1));
        }
        assert!(t.wait());

        // Verify dirty ranges.
        let ranges = [zero_range(1, 1), range(2, 1)];
        assert!(pager.verify_dirty_ranges(&vmo, &ranges));

        // Only the last page should be committed.
        assert_eq!(committed_bytes(&vmo), ps());

        // Detach the VMO.
        assert!(pager.detach_vmo(&vmo));
        assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));

        // Everything beyond the original size is dirty so should remain intact.
        assert_eq!(committed_bytes(&vmo), ps());
        assert!(pager.verify_dirty_ranges(&vmo, &ranges));

        // Verify VMO contents in the dirty range.
        let mut expected = vec![0u8; 3 * psu()];
        expected[2 * psu()] = data;
        // We should be able to read the dirty range both through mappings and with a VMO read.
        assert!(check_buffer_data(&vmo, 1, 2, &expected[psu()..], true));
        assert!(check_buffer_data(&vmo, 1, 2, &expected[psu()..], false));

        // Can writeback the dirty ranges.
        assert!(pager.writeback_begin_pages(&vmo, 1, 2));
        assert!(pager.writeback_end_pages(&vmo, 1, 2));

        // No more dirty pages.
        assert!(pager.verify_dirty_ranges(&vmo, &[]));

        // No more requests.
        assert_no_requests(&pager, &vmo);
    }
);

// Tests that clean pages are decommitted on detach.
trap_dirty_test!(decommit_clean_on_detach, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // We have one committed page.
    assert_eq!(committed_bytes(&vmo), ps());

    // No dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo));
    assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));

    // No dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No committed pages.
    assert_eq!(committed_bytes(&vmo), 0);

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that DIRTY requests cannot be generated after detach.
vmo_vmar_test!(no_dirty_requests_after_detach, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo1 = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo1, 0, 1));

    // Verify that no pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo1, &[]));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo1));
    assert!(pager.wait_for_page_complete(vmo1.key(), INFINITE));

    // Try to write to the VMO. As we are starting with a clean page, this would have generated a
    // DIRTY request pre-detach, but will now fail.
    if check_vmar {
        let ptr = vmo1.base_addr();
        let mut t1 = TestThread::new(move || {
            write_byte(ptr, 0xaa);
            true
        });
        assert!(t1.start());
        assert!(t1.wait_for_crash(ptr, zx::Status::BAD_STATE));
    } else {
        assert_eq!(vmo1.vmo().write(&[0xaa], 0), Err(zx::Status::BAD_STATE));
    }

    // No more requests.
    assert_no_requests(&pager, &vmo1);

    // No pages are dirty still.
    assert!(pager.verify_dirty_ranges(&vmo1, &[]));

    // Try again but this time with an AwaitingClean page, which would also have generated a DIRTY
    // request before the detach.
    let vmo2 = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo2, 0, 1));
    assert!(pager.dirty_pages(&vmo2, 0, 1));

    // Verify that the page is dirty.
    assert!(pager.verify_dirty_ranges(&vmo2, &[range(0, 1)]));

    // Being writeback, putting the page in AwaitingClean.
    assert!(pager.writeback_begin_pages(&vmo2, 0, 1));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo2));
    assert!(pager.wait_for_page_complete(vmo2.key(), INFINITE));

    // Try to write to the VMO. This will fail.
    if check_vmar {
        let ptr = vmo2.base_addr();
        let mut t2 = TestThread::new(move || {
            write_byte(ptr, 0xaa);
            true
        });
        assert!(t2.start());
        assert!(t2.wait_for_crash(ptr, zx::Status::BAD_STATE));
    } else {
        assert_eq!(vmo2.vmo().write(&[0xaa], 0), Err(zx::Status::BAD_STATE));
    }

    // The page is still dirty (AwaitingClean, but not clean yet).
    assert!(pager.verify_dirty_ranges(&vmo2, &[range(0, 1)]));

    // End the writeback. This should clean the page.
    assert!(pager.writeback_end_pages(&vmo2, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo2, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo2);
});

// Tests that detach with a pending DIRTY request fails the request.
vmo_vmar_test!(detach_with_pending_dirty_request, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // We have one committed page.
    assert_eq!(committed_bytes(&vmo), ps());

    // Try to write.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        let data = 0xaa;
        if check_vmar {
            write_byte(v.base_addr(), data);
            return true;
        }
        v.vmo().write(&[data], 0) == Err(zx::Status::BAD_STATE)
    });
    assert!(t.start());

    // Wait for the dirty request.
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo));
    assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));

    // The thread should terminate.
    if check_vmar {
        assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::BAD_STATE));
    } else {
        assert!(t.wait());
    }

    // Verify that no pages are dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No pages are committed.
    assert_eq!(committed_bytes(&vmo), 0);

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that resolving (or failing) a DIRTY request after the VMO is detached fails.
#[test]
fn resolve_dirty_request_after_detach() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[0xaa], 0) == Err(zx::Status::BAD_STATE));
    assert!(t.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Detach the VMO.
    assert!(pager.detach_vmo(&vmo));
    assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));

    // The write should fail.
    assert!(t.wait());

    // No more requests seen.
    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // This should fail.
    assert!(!pager.fail_pages(&vmo, 0, 1, zx::Status::IO));

    assert!(pager.get_page_dirty_request(&vmo, INFINITE_PAST).is_none());

    // Dirtying the page should fail as well.
    assert!(!pager.dirty_pages(&vmo, 0, 1));

    // The page was not dirtied.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that a VMO is marked modified on a zx_vmo_write.
trap_dirty_test!(modified_on_vmo_write, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    if traps_dirty(create_option) {
        // Dirty the page in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Write to the VMO.
    let data = 0xaau8;
    vmo.vmo().write(&[data], 0).unwrap();

    // The VMO should be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    expected[0] = data;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is marked modified when written through a mapping.
trap_dirty_test!(modified_on_mapping_write, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    if traps_dirty(create_option) {
        // Dirty the page in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // Map the VMO.
    let ptr = map_rw(&vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Write to the VMO via the mapping.
    let data = 0xbbu8;
    write_byte(ptr, data);

    // The VMO should be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    expected[0] = data;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is marked modified on resize.
trap_dirty_test!(modified_on_resize, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The VMO hasn't been resized yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Resize the VMO down.
    assert!(vmo.resize(0));

    // The VMO should be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the VMO up.
    assert!(vmo.resize(2));

    // The VMO should be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    expected.fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 2)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is marked modified on a ZX_VMO_OP_ZERO.
trap_dirty_test!(modified_on_op_zero, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 2));

    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 2, 0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    if traps_dirty(create_option) {
        // Dirty the page in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Zero a page in the VMO.
    zero_op(&vmo, 0, 1).unwrap();

    // The VMO should be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    expected[..psu()].fill(0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is not marked modified on a zx_vmo_read.
trap_dirty_test!(not_modified_on_vmo_read, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Read from the VMO.
    vmo.vmo().read(&mut [0u8], 0).unwrap();

    // The VMO shouldn't be modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is not marked modified when read through a mapping.
trap_dirty_test!(not_modified_on_mapping_read, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Map the VMO.
    let ptr = map_rw(&vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Read from the VMO via the mapping.
    let data = read_byte(ptr);

    // The VMO shouldn't be modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert_eq!(expected[0], data);

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is not marked modified when a write is failed by failing a DIRTY request.
#[test]
fn not_modified_on_failed_dirty_request() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Try to write to the VMO.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.vmo().write(&[0xaa], 0).is_ok());
    assert!(t1.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Fail the dirty request.
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));
    assert!(t1.wait_for_failure());

    // The VMO should not be modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Map the VMO.
    let ptr = map_rw(&vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // Try to write to the VMO via the mapping.
    let mut t2 = TestThread::new(move || {
        write_byte(ptr, 0xbb);
        true
    });
    assert!(t2.start());

    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Fail the dirty request.
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));
    assert!(t2.wait_for_crash(ptr, zx::Status::IO));

    // The VMO should not be modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that a VMO is not marked modified on a failed zx_vmo_write.
trap_dirty_test!(not_modified_on_failed_vmo_write, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    if traps_dirty(create_option) {
        // Dirty the page in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Write to the VMO with the source buffer set up such that the copying fails. Make the source
    // buffer pager backed too, and fail reads from it.
    let src_vmo = pager.create_vmo(1).unwrap();

    // Map the source VMO.
    let ptr = map_rw(&src_vmo, ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), ps());

    // Attempt the VMO write.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        // SAFETY: The source is mapped, and the kernel copies from it.
        let buf = unsafe { std::slice::from_raw_parts(ptr as *const u8, 1) };
        v.vmo().write(buf, 0).is_ok()
    });
    assert!(t.start());

    // We should see a read request when the VMO write attempts reading from the source VMO.
    assert!(pager.wait_for_page_read(&src_vmo, 0, 1, INFINITE));

    // Fail the read request so that the write fails.
    assert!(pager.fail_pages(&src_vmo, 0, 1, zx::Status::IO));
    assert!(t.wait_for_failure());

    // The VMO should not be modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    // We mark pages dirty when they are looked up, i.e. *before* writing to them, so they will
    // still be reported as dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that a VMO is marked modified when a zx_vmo_write partially succeeds.
trap_dirty_test!(modified_on_partial_vmo_write, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 2));

    let mut expected = vec![0u8; 2 * psu()];
    vmo.generate_buffer_contents(&mut expected, 2, 0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    if traps_dirty(create_option) {
        // Dirty the pages in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 2));
    }

    // Write to the VMO with the source buffer set up such that the copying partially fails. Make
    // the source buffer pager backed too, and fail reads from it.
    let src_vmo = pager.create_vmo(2).unwrap();
    // Supply a single page in the source, so we can partially read from it.
    assert!(pager.supply_pages(&src_vmo, 0, 1));

    // Map the source VMO.
    let ptr = map_rw(&src_vmo, 2 * ps()).unwrap();
    let _unmap = Unmap(Arc::new(AtomicUsize::new(ptr)), 2 * ps());

    // Attempt the VMO write.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        // SAFETY: The source is mapped, and the kernel copies from it.
        let buf = unsafe { std::slice::from_raw_parts(ptr as *const u8, 2 * psu()) };
        v.vmo().write(buf, 0).is_ok()
    });
    assert!(t.start());

    // We should see a read request when the VMO write attempts reading from the source VMO.
    assert!(pager.wait_for_page_read(&src_vmo, 1, 1, INFINITE));

    // Fail the read request so that the write fails.
    assert!(pager.fail_pages(&src_vmo, 1, 1, zx::Status::IO));
    assert!(t.wait_for_failure());

    // The write will have failed, so VMO should not be marked as modified.
    assert!(!pager.verify_modified(&vmo));

    // Verify dirty pages and contents.
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    // We mark pages dirty when they are looked up, i.e. *before* writing to them, so they will
    // still be reported as dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 2)]));

    // We will now try a partial write by failing dirty requests, which is only relevant for
    // TRAP_DIRTY.
    if !traps_dirty(create_option) {
        return;
    }

    // Start with clean pages again.
    assert!(pager.writeback_begin_pages(&vmo, 0, 2));
    assert!(pager.writeback_end_pages(&vmo, 0, 2));

    // Dirty a single page, so that writing to the other generates a dirty request.
    assert!(pager.dirty_pages(&vmo, 0, 1));

    // Try to write to the VMO.
    let mut t1 = write_pages_thread(&vmo, 0, 2, 0xaa, Ok(()));
    assert!(t1.start());

    // Should see a dirty request for page 1.
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));

    // Fail the dirty request.
    assert!(pager.fail_pages(&vmo, 1, 1, zx::Status::IO));
    assert!(t1.wait_for_failure());

    // The write succeeded partially, so the VMO should be modified.
    assert!(pager.verify_modified(&vmo));

    // Verify contents and dirty ranges.
    expected[..psu()].fill(0xaa);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests that querying the modified state without the reset option does not reset.
trap_dirty_test!(modified_no_reset, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    if traps_dirty(create_option) {
        // Dirty the page in preparation for the write, avoiding the need to trap.
        assert!(pager.dirty_pages(&vmo, 0, 1));
    }

    // The VMO hasn't been written to yet, so it shouldn't be marked modified.
    assert!(!pager.verify_modified(&vmo));

    // Write to the VMO.
    let data = 0xaau8;
    vmo.vmo().write(&[data], 0).unwrap();

    // Verify modified state without resetting it.
    let mut stats = zx_pager_vmo_stats_t::default();
    // SAFETY: The buffer is the size of the stats.
    let status = unsafe {
        zx::sys::zx_pager_query_vmo_stats(
            pager.pager().raw_handle(),
            vmo.vmo().raw_handle(),
            0,
            (&raw mut stats).cast(),
            size_of::<zx_pager_vmo_stats_t>(),
        )
    };
    assert_eq!(status, zx::sys::ZX_OK);
    assert_eq!(stats.modified, ZX_PAGER_VMO_STATS_MODIFIED);

    // Verify contents and dirty ranges.
    expected[0] = data;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // The VMO should still be marked modified.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state now with the reset option should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests a racing resize while a commit is blocked on a page request.
trap_dirty_test!(commit_resize_race, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create the VMO and supply only one page. Let the commit fault the other one in.
    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // The VMO hasn't been modified yet.
    assert!(!pager.verify_modified(&vmo));

    // Commit all the pages.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().op_range(zx::VmoOp::COMMIT, 0, 2 * ps()).is_ok());
    assert!(t.start());

    // We should see a READ request for the unpopulated page.
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));

    // Resize down the VMO invalidating the unpopulated page, so that the commit has no work to do
    // when woken up from the page request wait.
    assert!(vmo.resize(1));

    // Since the remaining page was already supplied, the commit should succeed.
    assert!(t.wait());

    // Resize should have modified the VMO.
    assert!(pager.verify_modified(&vmo));
    // Querying the modified state should have reset the modified flag.
    assert!(!pager.verify_modified(&vmo));

    // Verify VMO contents and dirty ranges.
    let mut expected = vec![0u8; psu()];
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
});

// Tests dirtying a large range at once. The core-tests run with random delayed PMM allocation, so
// by requiring a large number of pages to be allocated at once, we increase the likelihood of
// falling back to single page allocations and gradually accumulating the required number of pages.
#[test]
fn dirty_large_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(100, zx::VmoOptions::TRAP_DIRTY | zx::VmoOptions::RESIZABLE)
        .unwrap();

    // Empty source VMO to supply with zero pages.
    let vmo_src = zx::Vmo::create(100 * ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &vmo_src, 0, 100);

    // Resize the VMO up so that we also need to add zero pages at the tail.
    assert!(vmo.resize(200));

    // No pages in the VMO yet.
    assert_eq!(committed_bytes(&vmo), 0);

    // Dirty the entire VMO at once. This will allocate all 200 pages.
    assert!(pager.dirty_pages(&vmo, 0, 200));

    // All pages have been allocated and dirtied.
    assert_eq!(committed_bytes(&vmo), 200 * ps());
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 200)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests that committing both actual pages and zero page markers does not dirty the pages.
trap_dirty_test!(no_dirty_on_commit, zx::VmoOptions::empty(), |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(2, create_option).unwrap();

    // Supply an actual page and a zero page marker.
    assert!(pager.supply_pages(&vmo, 0, 1));
    let empty_src = zx::Vmo::create(ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &empty_src, 1, 1);

    // Commit the entire VMO.
    vmo.vmo().op_range(zx::VmoOp::COMMIT, 0, 2 * ps()).unwrap();

    // Both pages should be committed.
    assert_eq!(committed_bytes(&vmo), 2 * ps());

    // No pages should be dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // The VMO wasn't modified.
    assert!(!pager.verify_modified(&vmo));

    // No page requests seen.
    assert_no_requests(&pager, &vmo);
});

// Tests that committing pages in the newly extended tail does not lose dirtiness.
trap_dirty_test!(commit_extended_tail, zx::VmoOptions::RESIZABLE, |create_option| {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(1, create_option).unwrap();

    // Verify dirty ranges.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Resize the vmo up.
    assert!(vmo.resize(2));

    // Verify VMO contents and dirty ranges.
    let mut expected = vec![0u8; 2 * psu()];
    assert!(check_buffer_data(&vmo, 1, 1, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Commit the VMO. The existing page will be faulted in and the page beyond the old size will
    // transition from dirty zero to dirty non-zero.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().op_range(zx::VmoOp::COMMIT, 0, 2 * ps()).is_ok());
    assert!(t.start());

    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));

    if traps_dirty(create_option) {
        assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, 1, 1));
    }

    assert!(t.wait());

    // Verify VMO contents and dirty ranges.
    vmo.generate_buffer_contents(&mut expected, 1, 0);
    assert!(check_buffer_data(&vmo, 0, 2, &expected, true));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 1)]));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
});

/// Dirties `count` pages at `page_offset` with ZX_PAGER_OP_DIRTY, returning its status.
fn op_dirty(pager: &UserPager, vmo: &Vmo, page_offset: u64, count: u64) -> Result<(), zx::Status> {
    let range = page_offset * ps()..(page_offset + count) * ps();
    pager.pager().op_range(zx::PagerOp::Dirty, vmo.vmo(), range)
}

// Tests a race between resolving a DIRTY request for a zero range and marking the zero range
// clean.
#[test]
fn delayed_dirty_not_found() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY | zx::VmoOptions::RESIZABLE)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Resize the VMO up so that there's a zero range.
    assert!(vmo.resize(2));

    // Verify that the resized range is dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(1, 1)]));

    // Write to the zero range. This will generate a DIRTY request.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[0xaa], ps()).is_ok());
    assert!(t.start());
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));

    // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
    // now responsible for supplying all pages.
    assert!(pager.writeback_begin_zero_pages(&vmo, 1, 1));
    assert!(pager.writeback_end_pages(&vmo, 1, 1));

    // Verify that no pages are dirty now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No new requests generated.
    assert_no_requests(&pager, &vmo);

    // Now try to resolve the DIRTY request we saw earlier. This will fail with ZX_ERR_NOT_FOUND as
    // there is no committed page for the kernel to dirty.
    assert_eq!(op_dirty(&pager, &vmo, 1, 1), Err(zx::Status::NOT_FOUND));

    // Failing the DIRTY request will cause the kernel to try again, this time with a READ request
    // first as the zero page has been marked clean without being committed.
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(pager.wait_for_page_dirty(&vmo, 1, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 1, 1));
    assert!(t.wait());

    // The last page should still be dirty but non-zero now.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(1, 1)]));

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests zeroing a range and then marking it clean with an outstanding READ request.
#[test]
fn delayed_read_zero_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY | zx::VmoOptions::RESIZABLE)
        .unwrap();

    // Read from the VMO. This will generate a READ request.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        let mut data = [0xaau8];
        if v.vmo().read(&mut data, 0).is_err() {
            return false;
        }
        data[0] == 0
    });
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));

    // Zero the VMO.
    zero_op(&vmo, 0, 1).unwrap();

    // Verify that the range is dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 1)]));

    // The zeroing should also have unblocked the read request to be fulfilled with zeros now.
    assert!(t.wait());

    // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
    // now responsible for supplying all pages.
    assert!(pager.writeback_begin_zero_pages(&vmo, 0, 1));
    assert!(pager.writeback_end_pages(&vmo, 0, 1));

    // Verify that no pages are dirty now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // No new requests generated.
    assert_no_requests(&pager, &vmo);

    // No pages populated in the VMO.
    assert_eq!(committed_bytes(&vmo), 0);

    // No more requests.
    assert_no_requests(&pager, &vmo);
}

// Tests zeroing a range and then marking it clean with an outstanding DIRTY request.
#[test]
fn delayed_dirty_zero_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager
        .create_vmo_with_options(1, zx::VmoOptions::TRAP_DIRTY | zx::VmoOptions::RESIZABLE)
        .unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Write to the VMO. This will generate a DIRTY request.
    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&[0xaa], 0).is_ok());
    assert!(t.start());
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));

    // Zero the VMO.
    zero_op(&vmo, 0, 1).unwrap();

    // Verify that the range is dirty and zero.
    assert!(pager.verify_dirty_ranges(&vmo, &[zero_range(0, 1)]));

    // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
    // now responsible for supplying all pages.
    assert!(pager.writeback_begin_zero_pages(&vmo, 0, 1));
    assert!(pager.writeback_end_pages(&vmo, 0, 1));

    // Verify that no pages are dirty now.
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // Now try to resolve the DIRTY request we saw earlier. This will fail with ZX_ERR_NOT_FOUND as
    // there is no committed page for the kernel to dirty.
    assert_eq!(op_dirty(&pager, &vmo, 0, 1), Err(zx::Status::NOT_FOUND));

    // Failing the DIRTY request will cause the kernel to try again, this time with a READ request
    // first as the zero page has been marked clean without being committed.
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    let empty_src = zx::Vmo::create(ps()).unwrap();
    supply_zero_pages(&pager, &vmo, &empty_src, 0, 1);
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t.wait());

    // The page is now non-zero and dirty.
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // No new requests generated.
    assert_no_requests(&pager, &vmo);
}

/// `zx_pager_create_vmo`, as the tests of API violations call it.
fn pager_create_vmo(pager: &zx::Pager, options: u32, port: &zx::Port, size: u64) -> zx::Vmo {
    let mut handle = ZX_HANDLE_INVALID;
    // SAFETY: `handle` is valid for writes.
    let status = unsafe {
        zx::sys::zx_pager_create_vmo(
            pager.raw_handle(),
            options,
            port.raw_handle(),
            0,
            size,
            &mut handle,
        )
    };
    assert_eq!(status, ZX_OK);
    // SAFETY: The VMO's handle was just created.
    zx::Vmo::from(unsafe { zx::Handle::from_raw(handle) }.expect("created vmo"))
}

fn pager_op_range(
    pager: zx_handle_t,
    op: u32,
    vmo: zx_handle_t,
    offset: u64,
    length: u64,
    data: u64,
) -> zx_status_t {
    // SAFETY: No pointers.
    unsafe { zx::sys::zx_pager_op_range(pager, op, vmo, offset, length, data) }
}

/// `zx_pager_query_dirty_ranges` into nothing, for the tests of API violations.
fn query_dirty_ranges_null(
    pager: zx_handle_t,
    vmo: zx_handle_t,
    offset: u64,
    length: u64,
) -> zx_status_t {
    // SAFETY: Null pointers are not written.
    unsafe {
        zx::sys::zx_pager_query_dirty_ranges(
            pager,
            vmo,
            offset,
            length,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

/// `zx_pager_query_vmo_stats` into `buffer`, for the tests of API violations.
fn query_vmo_stats(
    pager: zx_handle_t,
    vmo: zx_handle_t,
    options: u32,
    buffer: *mut u8,
    buffer_size: usize,
) -> zx_status_t {
    // SAFETY: The tests pass null, invalid pointers, or stats of `buffer_size`.
    unsafe { zx::sys::zx_pager_query_vmo_stats(pager, vmo, options, buffer, buffer_size) }
}

/// `zx_pager_query_dirty_ranges`, returning the status, the number of ranges written, and the
/// number there are.
fn query_dirty_ranges(
    pager: zx_handle_t,
    vmo: zx_handle_t,
    offset: u64,
    length: u64,
    buffer: *mut u8,
    buffer_size: usize,
) -> (zx_status_t, usize, usize) {
    let (mut actual, mut avail) = (usize::MAX, usize::MAX);
    // SAFETY: The tests pass buffers of `buffer_size`, or invalid pointers.
    let status = unsafe {
        zx::sys::zx_pager_query_dirty_ranges(
            pager,
            vmo,
            offset,
            length,
            buffer,
            buffer_size,
            &mut actual,
            &mut avail,
        )
    };
    (status, actual, avail)
}

// Tests API violations for zx_pager_op_range.
#[test]
fn invalid_pager_op_range() {
    const NUM_VALID_OP_CODES: usize = 4;
    let opcodes: [u32; NUM_VALID_OP_CODES] = [
        ZX_PAGER_OP_FAIL,
        ZX_PAGER_OP_DIRTY,
        ZX_PAGER_OP_WRITEBACK_BEGIN,
        ZX_PAGER_OP_WRITEBACK_END,
    ];

    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let port = zx::Port::create();

    // Test common failure conditions for all ops.
    for opcode in opcodes {
        let vmo = pager_create_vmo(&pager, zx::sys::ZX_VMO_TRAP_DIRTY, &port, ps());
        let (p, v) = (pager.raw_handle(), vmo.raw_handle());

        // bad handles
        assert_eq!(pager_op_range(ZX_HANDLE_INVALID, opcode, v, 0, 0, 0), ZX_ERR_BAD_HANDLE);
        assert_eq!(pager_op_range(p, opcode, ZX_HANDLE_INVALID, 0, 0, 0), ZX_ERR_BAD_HANDLE);

        // wrong handle types
        assert_eq!(pager_op_range(v, opcode, v, 0, 0, 0), ZX_ERR_WRONG_TYPE);
        assert_eq!(pager_op_range(p, opcode, p, 0, 0, 0), ZX_ERR_WRONG_TYPE);

        // missing rights
        let ro_rights = rights::ZX_DEFAULT_VMO_RIGHTS & !zx::sys::ZX_RIGHT_WRITE;
        let ro_vmo = vmo.duplicate_handle(zx::Rights::from_bits_truncate(ro_rights)).unwrap();
        assert_eq!(pager_op_range(p, opcode, ro_vmo.raw_handle(), 0, 0, 0), ZX_ERR_ACCESS_DENIED);

        // using a non-pager-backed vmo
        let vmo2 = zx::Vmo::create(ps()).unwrap();
        assert_eq!(pager_op_range(p, opcode, vmo2.raw_handle(), 0, 0, 0), ZX_ERR_INVALID_ARGS);

        // using a pager vmo from another pager
        let pager2 = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        assert_eq!(pager_op_range(pager2.raw_handle(), opcode, v, 0, 0, 0), ZX_ERR_INVALID_ARGS);

        // misaligned offset or length
        assert_eq!(pager_op_range(p, opcode, v, 1, 0, 0), ZX_ERR_INVALID_ARGS);
        assert_eq!(pager_op_range(p, opcode, v, 0, 1, 0), ZX_ERR_INVALID_ARGS);

        // out of range
        let data = if opcode == ZX_PAGER_OP_FAIL { ZX_ERR_BAD_STATE as u64 } else { 0 };
        assert_eq!(pager_op_range(p, opcode, v, ps(), ps(), data), ZX_ERR_OUT_OF_RANGE);
    }

    // Test op-specific failure conditions
    let vmo = pager_create_vmo(&pager, 0, &port, ps());
    let (p, v) = (pager.raw_handle(), vmo.raw_handle());

    // invalid opcode
    assert_eq!(pager_op_range(p, 0, v, 0, 0, 0), ZX_ERR_NOT_SUPPORTED);

    // ZX_PAGER_OP_FAIL: invalid error codes
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_FAIL, v, 0, 0, 0x11ffffffff), ZX_ERR_INVALID_ARGS);

    assert_eq!(
        pager_op_range(p, ZX_PAGER_OP_FAIL, v, 0, 0, ZX_ERR_INTERNAL as u64),
        ZX_ERR_INVALID_ARGS
    );

    assert_eq!(pager_op_range(p, ZX_PAGER_OP_FAIL, v, 0, 0, 100), ZX_ERR_INVALID_ARGS);

    // ZX_PAGER_OP_DIRTY: VMO created without TRAP_DIRTY
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_DIRTY, v, 0, 0, 0), ZX_ERR_NOT_SUPPORTED);

    let vmo = pager_create_vmo(&pager, zx::sys::ZX_VMO_TRAP_DIRTY, &port, ps());
    let v = vmo.raw_handle();

    // ZX_PAGER_OP_DIRTY: dirty an unsupplied page
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_DIRTY, v, 0, ps(), 0), ZX_ERR_NOT_FOUND);

    // ZX_PAGER_OP_DIRTY: invalid data
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_DIRTY, v, 0, 0, 100), ZX_ERR_INVALID_ARGS);

    // ZX_PAGER_OP_WRITEBACK_BEGIN: invalid data
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_WRITEBACK_BEGIN, v, 0, 0, 100), ZX_ERR_INVALID_ARGS);

    // ZX_PAGER_OP_WRITEBACK_END: invalid data
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_WRITEBACK_END, v, 0, 0, 100), ZX_ERR_INVALID_ARGS);

    // missing pager rights
    let rights = rights::ZX_DEFAULT_PAGER_RIGHTS & !zx::sys::ZX_RIGHT_MANAGE_VMO;
    let pager = pager.replace_handle(zx::Rights::from_bits_truncate(rights)).unwrap();
    for opcode in opcodes {
        assert_eq!(pager_op_range(pager.raw_handle(), opcode, v, 0, 0, 0), ZX_ERR_ACCESS_DENIED);
    }
}

// Tests API violations for zx_pager_query_vmo_stats and zx_pager_query_dirty_ranges.
#[test]
fn invalid_query() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let port = zx::Port::create();

    let vmo = pager_create_vmo(&pager, 0, &port, ps());
    let (p, v) = (pager.raw_handle(), vmo.raw_handle());
    let null = std::ptr::null_mut();

    // bad handles
    assert_eq!(query_dirty_ranges_null(ZX_HANDLE_INVALID, v, 0, 0), ZX_ERR_BAD_HANDLE);
    assert_eq!(query_dirty_ranges_null(p, ZX_HANDLE_INVALID, 0, 0), ZX_ERR_BAD_HANDLE);
    assert_eq!(query_vmo_stats(ZX_HANDLE_INVALID, v, 0, null, 0), ZX_ERR_BAD_HANDLE);
    assert_eq!(query_vmo_stats(p, ZX_HANDLE_INVALID, 0, null, 0), ZX_ERR_BAD_HANDLE);

    // wrong handle types
    assert_eq!(query_dirty_ranges_null(v, v, 0, 0), ZX_ERR_WRONG_TYPE);
    assert_eq!(query_dirty_ranges_null(p, p, 0, 0), ZX_ERR_WRONG_TYPE);
    assert_eq!(query_vmo_stats(v, v, 0, null, 0), ZX_ERR_WRONG_TYPE);
    assert_eq!(query_vmo_stats(p, p, 0, null, 0), ZX_ERR_WRONG_TYPE);

    // missing rights
    let rights = rights::ZX_DEFAULT_VMO_RIGHTS & !zx::sys::ZX_RIGHT_READ;
    let dup = vmo.duplicate_handle(zx::Rights::from_bits_truncate(rights)).unwrap();
    assert_eq!(query_dirty_ranges_null(p, dup.raw_handle(), 0, 0), ZX_ERR_ACCESS_DENIED);
    assert_eq!(query_vmo_stats(p, dup.raw_handle(), 0, null, 0), ZX_ERR_ACCESS_DENIED);

    // using a non-pager-backed vmo
    let vmo2 = zx::Vmo::create(ps()).unwrap();
    assert_eq!(query_dirty_ranges_null(p, vmo2.raw_handle(), 0, 0), ZX_ERR_INVALID_ARGS);
    assert_eq!(query_vmo_stats(p, vmo2.raw_handle(), 0, null, 0), ZX_ERR_INVALID_ARGS);

    // using a pager vmo from another pager
    let pager2 = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    assert_eq!(query_dirty_ranges_null(pager2.raw_handle(), v, 0, 0), ZX_ERR_INVALID_ARGS);
    assert_eq!(query_vmo_stats(pager2.raw_handle(), v, 0, null, 0), ZX_ERR_INVALID_ARGS);

    // out of range
    assert_eq!(query_dirty_ranges_null(p, v, ps(), ps()), ZX_ERR_OUT_OF_RANGE);

    // invalid option
    let mut stats = zx_pager_vmo_stats_t::default();
    let stats_ptr = (&raw mut stats).cast::<u8>();
    let stats_size = size_of::<zx_pager_vmo_stats_t>();
    assert_eq!(query_vmo_stats(p, v, 100, stats_ptr, stats_size), ZX_ERR_INVALID_ARGS);

    // buffer too small
    assert_eq!(query_vmo_stats(p, v, 0, stats_ptr, 0), ZX_ERR_BUFFER_TOO_SMALL);

    // invalid buffer
    // Dirty a page first, so the query has something to enumerate.
    pager.supply_pages(&vmo, 0..ps(), &vmo2, 0).unwrap();
    vmo.write(&42u64.to_ne_bytes(), 0).unwrap();
    let (status, _, _) = query_dirty_ranges(p, v, 0, ps(), null, size_of::<zx_vmo_dirty_range_t>());
    assert_eq!(status, ZX_ERR_INVALID_ARGS);
    assert_eq!(query_vmo_stats(p, v, 0, null, stats_size), ZX_ERR_INVALID_ARGS);

    // missing pager rights
    let rights = rights::ZX_DEFAULT_PAGER_RIGHTS & !zx::sys::ZX_RIGHT_MANAGE_VMO;
    let pager = pager.replace_handle(zx::Rights::from_bits_truncate(rights)).unwrap();
    let p = pager.raw_handle();
    assert_eq!(query_dirty_ranges_null(p, v, 0, 0), ZX_ERR_ACCESS_DENIED);
    assert_eq!(query_vmo_stats(p, v, 0, null, 0), ZX_ERR_ACCESS_DENIED);
}

// Regression test for https://fxbug.dev/504723239.
#[test]
fn query_vmo_stats_invalid_pointer() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let port = zx::Port::create();

    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, 4096).unwrap();

    // Pass an invalid non-canonical / kernel address as the buffer. This should not cause the
    // kernel to crash.
    let status = query_vmo_stats(
        pager.raw_handle(),
        vmo.raw_handle(),
        0,
        0xffffffffffffffff_usize as *mut u8,
        size_of::<zx_pager_vmo_stats_t>(),
    );
    assert_eq!(status, ZX_ERR_INVALID_ARGS);
}

// Tests that attempt to exercise the early termination of the query dirty ranges loop to avoid
// redundantly calculating an additional range prior to terminating if the user did not provide a
// large enough buffer.
#[test]
fn query_early_termination() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let port = zx::Port::create();

    let vmo = pager_create_vmo(&pager, 0, &port, ps() * 3);
    let (p, v) = (pager.raw_handle(), vmo.raw_handle());
    // Dirty some pages first, so the query has something to enumerate.
    let vmo2 = zx::Vmo::create(ps()).unwrap();
    pager.supply_pages(&vmo, 0..ps(), &vmo2, 0).unwrap();
    pager.supply_pages(&vmo, ps() * 2..ps() * 3, &vmo2, 0).unwrap();
    let mut data = 42u64;
    vmo.write(&data.to_ne_bytes(), 0).unwrap();
    vmo.write(&data.to_ne_bytes(), ps() * 2).unwrap();

    // Query with a buffer size too small to hold any of the results, but otherwise valid.
    let mut actual = 0usize;
    // SAFETY: The buffer is `data`.
    let status = unsafe {
        zx::sys::zx_pager_query_dirty_ranges(
            p,
            v,
            0,
            ps(),
            (&raw mut data).cast(),
            size_of::<u64>(),
            &mut actual,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, ZX_OK);
    assert_eq!(actual, 0);
    // Now query with a buffer large enough for the first dirty range, but not the second.
    let mut dirty_range = zx_vmo_dirty_range_t::default();
    let dirty_range_ptr = (&raw mut dirty_range).cast::<u8>();
    let range_size = size_of::<zx_vmo_dirty_range_t>();
    // SAFETY: The buffer is `dirty_range`.
    let status = unsafe {
        zx::sys::zx_pager_query_dirty_ranges(
            p,
            v,
            0,
            ps() * 3,
            dirty_range_ptr,
            range_size,
            &mut actual,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, ZX_OK);
    assert_eq!(dirty_range.offset, 0);
    assert_eq!(dirty_range.length, ps());
    assert_eq!(actual, 1);
    // Query again to get and check avail.
    let (status, _, avail) = query_dirty_ranges(p, v, 0, ps() * 3, dirty_range_ptr, range_size);
    assert_eq!(status, ZX_OK);
    assert_eq!(avail, 2);
    // Now query both ranges;
    let mut dirty_ranges = [zx_vmo_dirty_range_t::default(); 2];
    // SAFETY: The buffer is `dirty_ranges`.
    let status = unsafe {
        zx::sys::zx_pager_query_dirty_ranges(
            p,
            v,
            0,
            ps() * 3,
            dirty_ranges.as_mut_ptr().cast(),
            range_size * 2,
            &mut actual,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, ZX_OK);
    assert_eq!(dirty_range.offset, 0);
    assert_eq!(dirty_range.length, ps());
    assert_eq!(actual, 2);
}

// This value corresponds to `VmObjectPaged::ReadWriteInternal::kMaxWriteWaitPages`
const MAX_WRITE_WAIT_PAGES: u64 = 256;

// Test that a large write absent pages generates correct requests.
#[test]
fn large_write_batched() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = MAX_WRITE_WAIT_PAGES * 8;
    const PAGE_STEPS: u64 = 8;
    const _: () = assert!(NUM_PAGES.is_multiple_of(PAGE_STEPS));
    const _: () = assert!(PAGE_STEPS < MAX_WRITE_WAIT_PAGES);
    const _: () = assert!(MAX_WRITE_WAIT_PAGES.is_multiple_of(PAGE_STEPS));
    // Start the read at an offset to ensure there is a difference between the page requests VMO
    // offset, and the offset from the start of the request.
    const START_OFFSET: u64 = 2;
    const VMO_PAGES: u64 = NUM_PAGES + START_OFFSET;

    let vmo = pager.create_vmo_with_options(VMO_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    let mut expected = vec![0u8; NUM_PAGES as usize * psu()];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, START_OFFSET);

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.vmo().write(&expected, START_OFFSET * ps()).is_ok());

    assert!(t.start());

    for i in (0..NUM_PAGES).step_by(MAX_WRITE_WAIT_PAGES as usize) {
        // Expect to see a read request for the batch range.
        assert!(pager.wait_for_page_read(&vmo, i + START_OFFSET, MAX_WRITE_WAIT_PAGES, INFINITE));
        // Supply the read request in pieces.
        for j in (0..MAX_WRITE_WAIT_PAGES).step_by(PAGE_STEPS as usize) {
            assert!(pager.supply_pages(&vmo, i + j + START_OFFSET, PAGE_STEPS));
        }
        // Now there should be a dirty request for the batch range.
        assert!(pager.wait_for_page_dirty(&vmo, i + START_OFFSET, MAX_WRITE_WAIT_PAGES, INFINITE));
        // Dirty the range in pieces.
        for j in (0..MAX_WRITE_WAIT_PAGES).step_by(PAGE_STEPS as usize) {
            assert!(pager.dirty_pages(&vmo, i + j + START_OFFSET, PAGE_STEPS));
        }
    }
    assert!(t.wait());
    assert!(vmo.check_vmo(START_OFFSET, NUM_PAGES, None));
}

#[test]
fn unbounded() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    assert!(pager.create_vmo_with_options(1, zx::VmoOptions::UNBOUNDED).is_none());
}

/// Returns the dirty ranges of the first `pages` pages of `vmo`, in bytes, and how many there
/// are, as the tests of unbounded VMOs query them.
fn query_ranges(pager: &UserPager, vmo: &Vmo, pages: u64) -> (Vec<zx_vmo_dirty_range_t>, usize) {
    let mut ranges = [zx_vmo_dirty_range_t::default(); 2];
    let (status, actual, avail) = query_dirty_ranges(
        pager.pager().raw_handle(),
        vmo.vmo().raw_handle(),
        0,
        pages * ps(),
        ranges.as_mut_ptr().cast(),
        size_of_val(&ranges),
    );
    assert_eq!(status, ZX_OK);
    (ranges[..actual].to_vec(), avail)
}

fn byte_range(offset: u64, length: u64, options: u64) -> zx_vmo_dirty_range_t {
    zx_vmo_dirty_range_t { offset: offset * ps(), length: length * ps(), options }
}

// Tests that unbounded VMOs and setting the stream size correctly create a dirty-untracked zero
// range beyond the stream size.
#[test]
fn untracked_beyond_stream_size() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_unbounded_vmo(2 * ps(), zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Nothing dirty yet.
    assert!(!pager.verify_modified(&vmo));
    // Queried without the UserPager methods, as unbounded VMOs are tracked with a size of 0.
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![], 0));

    // Pages beyond the stream size should read as zeros.
    let expected = vec![0u8; 10 * psu()];
    assert!(check_buffer_data(&vmo, 2, 8, &expected, false));

    // Increase the stream size. This should reveal a dirty zero range.
    vmo.vmo().set_stream_size(5 * ps()).unwrap();
    assert!(pager.verify_modified(&vmo));
    let is_zero = zx::sys::ZX_VMO_DIRTY_RANGE_IS_ZERO;
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![byte_range(2, 3, is_zero)], 1));

    // Pages beyond the old stream size should still read as zeros.
    assert!(check_buffer_data(&vmo, 2, 8, &expected, false));

    // Decrease the stream size. The dirty zero range should get absorbed back.
    vmo.vmo().set_stream_size(ps()).unwrap();
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![], 0));

    // Pages beyond the new stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 1, 9, &expected, false));

    // No page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Tests that single page dirty zero interval slots are correctly absorbed into untracked zero
// intervals when setting the stream size.
#[test]
fn untracked_absorbs_interval_slots() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_vmo_with_options(3, zx::VmoOptions::TRAP_DIRTY).unwrap();
    // Create a single page untracked zero interval at page 2.
    vmo.vmo().set_stream_size(2 * ps()).unwrap();

    // Nothing dirty yet.
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 3), (vec![], 0));

    // Pages beyond the stream size should read as zeros.
    let expected = vec![0u8; 2 * psu()];
    assert!(check_buffer_data(&vmo, 2, 1, &expected, false));

    // Decrease the stream size by a single page. This should absorb the existing interval slot at
    // page 2 into the new untracked range [1, 3).
    vmo.vmo().set_stream_size(ps()).unwrap();

    // Nothing dirty still.
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 3), (vec![], 0));

    // Pages beyond the new stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 1, 2, &expected, false));

    // Increase the stream size again and commit page 1.
    vmo.vmo().set_stream_size(2 * ps()).unwrap();
    assert!(pager.dirty_pages(&vmo, 1, 1));

    // Verify that the committed page is dirty.
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 3), (vec![byte_range(1, 1, 0)], 1));

    // Pages beyond the old stream size should still read as zeros.
    assert!(check_buffer_data(&vmo, 2, 1, &expected, false));

    // Decrease the stream size again, overwriting the page. The single untracked zero slot should
    // get absorbed back.
    vmo.vmo().set_stream_size(ps()).unwrap();
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 3), (vec![], 0));

    // Pages beyond the new stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 1, 2, &expected, false));

    // No page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Tests that pages committed beyond the stream size are absorbed back into untracked (or dirty)
// zero intervals when setting the stream size.
#[test]
fn untracked_absorbs_pages() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo = pager.create_unbounded_vmo(2 * ps(), zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Nothing dirty yet.
    assert!(!pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![], 0));

    // Pages beyond the stream size should read as zeros.
    let expected = vec![0u8; 10 * psu()];
    assert!(check_buffer_data(&vmo, 2, 8, &expected, false));

    // Dirty a few pages beyond the stream size. Pages 4, 5, and 8 are written.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        [4, 5, 8].iter().all(|i| v.vmo().write(&42u64.to_ne_bytes(), i * ps()).is_ok())
    });
    assert!(t.start());

    for i in [4, 5, 8] {
        assert!(pager.wait_for_page_dirty(&vmo, i, 1, INFINITE));
        assert!(pager.dirty_pages(&vmo, i, 1));
    }

    assert!(t.wait());

    // Verify dirty ranges.
    let ranges = vec![byte_range(4, 2, 0), byte_range(8, 1, 0)];
    assert_eq!(query_ranges(&pager, &vmo, 10), (ranges, 2));

    // Pages not dirtied beyond stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 2, 2, &expected, false));
    assert!(check_buffer_data(&vmo, 6, 2, &expected, false));
    assert!(check_buffer_data(&vmo, 9, 1, &expected, false));

    // Increase the stream size so that some pages become a part of a dirty zero interval and some
    // of an untracked zero interval.
    vmo.vmo().set_stream_size(7 * ps()).unwrap();
    assert!(pager.verify_modified(&vmo));

    // Verify dirty ranges.
    let is_zero = zx::sys::ZX_VMO_DIRTY_RANGE_IS_ZERO;
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![byte_range(2, 5, is_zero)], 1));

    // Pages beyond the old stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 2, 8, &expected, false));

    // Decrease the stream size.
    vmo.vmo().set_stream_size(ps()).unwrap();
    assert!(pager.verify_modified(&vmo));
    assert_eq!(query_ranges(&pager, &vmo, 10), (vec![], 0));

    // Pages beyond the new stream size should read as zeros.
    assert!(check_buffer_data(&vmo, 1, 9, &expected, false));

    // No more page requests seen.
    assert_no_requests(&pager, &vmo);
}

// Regression test for https://fxbug.dev/502272571
#[test]
fn query_dirty_ranges_invalid_buffer() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let port = zx::Port::create();

    let vmo_size = ps() * 10;
    let vmo = pager_create_vmo(&pager, zx::sys::ZX_VMO_TRAP_DIRTY, &port, vmo_size);
    let (p, v) = (pager.raw_handle(), vmo.raw_handle());

    // Supply a page and dirty it so that EnumerateDirtyRanges finds something.
    let aux_vmo = zx::Vmo::create(ps()).unwrap();
    pager.supply_pages(&vmo, 0..ps(), &aux_vmo, 0).unwrap();
    assert_eq!(pager_op_range(p, ZX_PAGER_OP_DIRTY, v, 0, ps(), 0), ZX_OK);

    // Use an invalid buffer pointer and ensure that the syscall fails in some way, and does not
    // cause a kernel panic.
    let invalid = 0xffffffff00000000_usize as *mut u8;
    let (status, _, _) = query_dirty_ranges(p, v, 0, vmo_size, invalid, psu());
    assert_eq!(status, ZX_ERR_INVALID_ARGS);
}
