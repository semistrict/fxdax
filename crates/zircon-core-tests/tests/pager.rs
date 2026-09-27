// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ported from `zircon/system/utest/core/pager/pager.cc`.
//!
//! Each `VMO_VMAR_TEST` is a module with a `vmar` and a `vmo` test, checking content through a
//! mapping or with `zx_vmo_read`.
//!
//! Not ported, as the kernel lacks what they need:
//! - Threads cannot be suspended: `SuspendReadTest`, `SuspendCommitTest`, `SuspendInKernelTest`,
//!   `SuspendInKernelCaptureFaultsTest`.
//! - VMOs have no children: `DecommitOnDetachTest`, `DeepHierarchy`,
//!   `PageRequestBatchingChecksAncestorPages`, `EvictionHintsWithClones`, `EvictionHintsCloneVmar`,
//!   `OpCommitCloneVmar`, `EarlyWakeCloneRace` and the `Clone` tests, and the parts of
//!   `DecommitTest` and `Prefetch` that use clones and slices.
//! - There is no system resource, so no BTIs or physical VMOs: `ResizeBlockedPin`,
//!   `PinnedSupplyPages`, `PinPagerVmoWithLoanedPages`, `PagerDetachPinned`, and the parts of
//!   `InvalidPagerSupplyPages` Zircon skips without one.

use std::sync::Arc;
use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;
use zircon_core_tests::vmo_vmar_test;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

fn page_size_usize() -> usize {
    page_size() as usize
}

/// Returns the bytes of a mapping.
///
/// # Safety
///
/// The range must be mapped for the life of the slice.
unsafe fn mapped<'a>(addr: usize, len: usize) -> &'a [u8] {
    // SAFETY: Guaranteed by the caller.
    unsafe { std::slice::from_raw_parts(addr as *const u8, len) }
}

// Simple test that checks that a single thread can access a single page.
vmo_vmar_test!(single_page_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t.wait());
});

// Test that calling zx_port_cancel_key on a pager's port with a VMO's key does not
// wedge the pager completion protocol (i.e. packet_busy_ is properly reset by Free()).
vmo_vmar_test!(cancel_key_does_not_wedge_the_pager, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    // 1. Provoke a page request on page 0 so a pager packet is bound to the port queue.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t1.start());
    assert!(t1.wait_for_blocked());

    // 2. Call zx_port_cancel_key on the pager's port with that VMO's key while the packet is
    // queued. This unlinks the bound PagerProxy packet and calls Free(), returning ZX_OK.
    pager.port().cancel(vmo.key()).unwrap();

    // 3. Service page 0 directly so that thread t1 completes cleanly.
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t1.wait());

    // 4. Assert the pager can still service a subsequent request - i.e., provoke a fault
    // on page 1 and verify that the reader finishes within a bounded wait without hanging.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || check_buffer(&v, 1, 1, check_vmar));
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(t2.wait());
});

// Test that a fault can be fulfilled with an uncommitted page.
vmo_vmar_test!(uncommitted_single_page_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let data = vec![0u8; page_size_usize()];
    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer_data(&v, 0, 1, &data, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    let empty = zx::Vmo::create(page_size()).unwrap();
    assert!(pager.supply_pages_from(&vmo, 0, 1, empty, 0));
    assert!(t.wait());
});

// Tests that pre-supplied pages don't result in requests.
vmo_vmar_test!(presupply_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(t.wait());
    assert!(!pager.wait_for_page_read(&vmo, 0, 1, INFINITE_PAST));
});

// Tests that supplies between the request and reading the port
// causes the request to be aborted.
vmo_vmar_test!(early_supply_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    // Use a second thread to make sure the queue of requests is flushed.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || check_buffer(&v, 1, 1, check_vmar));

    assert!(t1.start());
    assert!(t1.wait_for_blocked());
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t1.wait());

    assert!(t2.start());
    assert!(t2.wait_for_blocked());
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(t2.wait());

    assert!(!pager.wait_for_page_read(&vmo, 0, 1, INFINITE_PAST));
});

// Checks that a single thread can sequentially access multiple pages.
vmo_vmar_test!(sequential_multipage_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 32;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, NUM_PAGES, check_vmar));
    assert!(t.start());

    if check_vmar {
        // When mapped, the hardware faults will fault in pages one by one.
        for i in 0..NUM_PAGES {
            assert!(pager.wait_for_page_read(&vmo, i, 1, INFINITE));
            assert!(pager.supply_pages(&vmo, i, 1));
        }
    } else {
        assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
        assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));
    }
    assert!(t.wait());
});

// Tests that multiple threads can concurrently access different pages.
vmo_vmar_test!(concurrent_multipage_access_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || check_buffer(&v, 1, 1, check_vmar));
    assert!(t.start());
    assert!(t2.start());

    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 2));

    assert!(t.wait());
    assert!(t2.wait());
});

// Tests that multiple threads can concurrently access a single page.
vmo_vmar_test!(concurrent_overlapping_access_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    const NUM_THREADS: usize = 32;
    let mut threads = Vec::new();
    for _ in 0..NUM_THREADS {
        let v = vmo.clone();
        let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
        t.start();
        assert!(t.wait_for_blocked());
        threads.push(t);
    }

    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));

    for t in &mut threads {
        assert!(t.wait());
    }
    assert!(!pager.wait_for_page_read(&vmo, 0, 1, INFINITE_PAST));
});

// Tests that multiple threads can concurrently access multiple pages and
// be satisfied by a single supply operation.
vmo_vmar_test!(bulk_single_supply_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 8;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let mut ts = Vec::new();
    for i in 0..NUM_PAGES {
        let v = vmo.clone();
        let mut t = TestThread::new(move || check_buffer(&v, i, 1, check_vmar));
        assert!(t.start());
        assert!(pager.wait_for_page_read(&vmo, i, 1, INFINITE));
        ts.push(t);
    }

    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));

    for t in &mut ts {
        assert!(t.wait());
    }
});

// Test body for odd supply tests.
fn bulk_odd_supply_test_inner(check_vmar: bool, use_src_offset: bool) {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Interesting supply lengths that will exercise splice logic.
    const SUPPLY_LENGTHS: [u64; 8] = [2, 3, 5, 7, 37, 5, 13, 23];
    let sum: u64 = SUPPLY_LENGTHS.iter().sum();
    let vmo = pager.create_vmo(sum).unwrap();

    let mut page_idx = 0;
    for supply_len in SUPPLY_LENGTHS {
        let offset = page_idx;
        let mut ts = Vec::new();
        for j in 0..supply_len {
            let thread_offset = offset + j;
            let v = vmo.clone();
            let mut t = TestThread::new(move || check_buffer(&v, thread_offset, 1, check_vmar));
            assert!(t.start());
            assert!(pager.wait_for_page_read(&vmo, thread_offset, 1, INFINITE));
            ts.push(t);
        }

        let src_offset = if use_src_offset { offset } else { 0 };
        assert!(pager.supply_pages_at(&vmo, offset, supply_len, src_offset));

        for t in &mut ts {
            assert!(t.wait());
        }
        page_idx += supply_len;
    }
}

// Test that exercises supply logic by supplying data in chunks of unusual length.
vmo_vmar_test!(bulk_odd_length_supply_test, |check_vmar| {
    bulk_odd_supply_test_inner(check_vmar, false)
});

// Test that exercises supply logic by supplying data in chunks of
// unusual lengths and offsets.
vmo_vmar_test!(bulk_odd_offset_supply_test, |check_vmar| {
    bulk_odd_supply_test_inner(check_vmar, true)
});

// Tests that supply doesn't overwrite existing content.
vmo_vmar_test!(overlap_supply_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let alt_data_vmo = zx::Vmo::create(page_size()).unwrap();
    let mut alt_data = vec![0u8; page_size_usize()];
    vmo.generate_buffer_contents(&mut alt_data, 1, 2);
    alt_data_vmo.write(&alt_data, 0).unwrap();

    assert!(pager.supply_pages_from(&vmo, 0, 1, alt_data_vmo, 0));
    assert!(pager.supply_pages(&vmo, 1, 1));

    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        check_buffer_data(&v, 0, 1, &alt_data, check_vmar) && check_buffer(&v, 1, 1, check_vmar)
    });
    assert!(t.start());
    assert!(t.wait());
    assert!(!pager.wait_for_page_read(&vmo, 0, 1, INFINITE_PAST));
});

// Tests that a pager can handle lots of pending page requests.
vmo_vmar_test!(many_request_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 257; // Arbitrary large number
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let mut ts = Vec::new();
    for i in 0..NUM_PAGES {
        let v = vmo.clone();
        let mut t = TestThread::new(move || check_buffer(&v, i, 1, check_vmar));
        assert!(t.start());
        assert!(t.wait_for_blocked());
        ts.push(t);
    }

    for (i, t) in ts.iter_mut().enumerate() {
        assert!(pager.wait_for_page_read(&vmo, i as u64, 1, INFINITE));
        assert!(pager.supply_pages(&vmo, i as u64, 1));
        assert!(t.wait());
    }
});

// Tests that a pager can support creating and destroying successive vmos.
#[test]
fn successive_vmo_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_VMOS: u32 = 64;
    for _ in 0..NUM_VMOS {
        let vmo = pager.create_vmo(1).unwrap();
        let v = vmo.clone();
        let mut t = TestThread::new(move || check_buffer(&v, 0, 1, true));
        assert!(t.start());
        assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
        assert!(pager.supply_pages(&vmo, 0, 1));
        assert!(t.wait());
        pager.release_vmo(&vmo);
    }
}

// Tests that a pager can support multiple concurrent vmos.
#[test]
fn multiple_concurrent_vmo_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_VMOS: usize = 8;
    let mut vmos = Vec::new();
    let mut ts = Vec::new();
    for _ in 0..NUM_VMOS {
        let vmo = pager.create_vmo(1).unwrap();
        let v = vmo.clone();
        let mut t = TestThread::new(move || check_buffer(&v, 0, 1, true));
        assert!(t.start());
        assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
        vmos.push(vmo);
        ts.push(t);
    }

    for (vmo, t) in vmos.iter().zip(ts.iter_mut()) {
        assert!(pager.supply_pages(vmo, 0, 1));
        assert!(t.wait());
    }
}

// Tests that unmapping a vmo while threads are blocked on a pager read
// eventually results in pagefaults.
#[test]
fn vmar_unmap_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let addr =
        root_vmar().map(0, vmo.vmo(), 0, page_size_usize(), zx::VmarFlags::PERM_READ).unwrap();

    let mut t = TestThread::new(move || {
        // SAFETY: The thread is expected to crash when the page is unmapped.
        let data = unsafe { (addr as *const u8).read_volatile() };
        // This comparison is not relevant. Just use the data so that the dereference is not
        // optimized out. The thread is expected to crash before this point anyway.
        data > 0
    });
    assert!(t.start());
    assert!(t.wait_for_blocked());

    // SAFETY: The thread's access is expected to fault.
    unsafe { root_vmar().unmap(addr, page_size_usize()) }.unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    assert!(t.wait_for_crash(addr, zx::Status::NOT_FOUND));
}

// Tests that replacing a vmar mapping while threads are blocked on a
// pager read results in reads to the new mapping.
#[test]
fn vmar_remap_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create two pager VMOs. We will switch the VM mapping from one VMO to the other mid-access.
    const NUM_PAGES: u64 = 8;
    let vmo1 = pager.create_vmo(NUM_PAGES).unwrap();
    let vmo2 = pager.create_vmo(NUM_PAGES).unwrap();
    let len = (NUM_PAGES * page_size()) as usize;

    // Create a mapping for vmo1.
    let addr = root_vmar().map(0, vmo1.vmo(), 0, len, zx::VmarFlags::PERM_READ).unwrap();

    let mut ts = Vec::new();
    for t in 0..NUM_PAGES {
        let v = vmo2.clone();
        let mut thread = TestThread::new(move || {
            let mut data = vec![0u8; page_size_usize()];
            v.generate_buffer_contents(&mut data, 1, t);
            // Thread |t| accesses page |t|. Each thread will block here and generate a page
            // request. We will resolve the page request after switching out the mapping to vmo2,
            // so the contents will match vmo2, not vmo1.
            // SAFETY: The page stays mapped until the thread is joined.
            data == unsafe { mapped(addr + (t * page_size()) as usize, page_size_usize()) }
        });
        assert!(thread.start());
        ts.push(thread);
    }

    for t in &ts {
        assert!(t.wait_for_blocked());
    }

    // Now that all the threads are blocked, swap out the mapping to point to vmo2.
    let info = root_vmar().info().unwrap();
    // SAFETY: The threads only read the mapping, and expect it to be replaced.
    let new_addr = unsafe {
        root_vmar().map_unsafe(
            addr - info.base,
            vmo2.vmo(),
            0,
            len,
            zx::VmarFlagsExtended::PERM_READ | zx::VmarFlagsExtended::SPECIFIC_OVERWRITE,
        )
    }
    .unwrap();
    assert_eq!(addr, new_addr);

    // Supply pages in vmo1. The threads will still remain blocked.
    assert!(pager.supply_pages(&vmo1, 0, NUM_PAGES));
    for t in &ts {
        assert!(t.wait_for_blocked());
    }

    // We should see page requests for vmo2.
    for _ in 0..NUM_PAGES {
        let (offset, length) = pager.get_page_read_request(&vmo2, INFINITE).unwrap();
        assert_eq!(length, 1);
        assert!(pager.supply_pages(&vmo2, offset, 1));
        assert!(ts[offset as usize].wait());
    }
    // SAFETY: Nothing refers to the mapping any more.
    unsafe { root_vmar().unmap(addr, len) }.unwrap();
}

// Tests that ZX_VM_MAP_RANGE works with pager vmos (i.e. maps in backed regions
// but doesn't try to pull in new regions).
#[test]
fn vmar_map_range_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create a vmo with 2 pages. Supply the first page but not the second.
    let vmo = pager.create_vmo(2).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));
    let len = 2 * page_size_usize();

    // Map the vmo. This shouldn't block or generate any new page requests.
    let ptr = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (v, p) = (vmo.clone(), ptr.clone());
    let mut t = TestThread::new(move || {
        let addr = root_vmar()
            .map(0, v.vmo(), 0, len, zx::VmarFlags::PERM_READ | zx::VmarFlags::MAP_RANGE)
            .unwrap();
        p.store(addr, std::sync::atomic::Ordering::Relaxed);
        true
    });
    assert!(t.start());
    assert!(t.wait());
    let addr = ptr.load(std::sync::atomic::Ordering::Relaxed);

    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Verify the buffer contents. This should generate a new request for
    // the second page, which we want to fulfill.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || {
        let mut data = vec![0u8; len];
        v.generate_buffer_contents(&mut data, 2, 0);
        // SAFETY: The mapping stays until the thread is joined.
        data == unsafe { mapped(addr, len) }
    });
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(t2.wait());

    // After the verification is done, make sure there are no unexpected
    // page requests.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Cleanup the mapping we created.
    // SAFETY: Nothing refers to the mapping any more.
    unsafe { root_vmar().unmap(addr, len) }.unwrap();
}

// Tests that reads don't block forever if a vmo is resized out from under a read.
vmo_vmar_test!(read_resize_test, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo_with_options(1, zx::VmoOptions::RESIZABLE).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(vmo.resize(0));
    if check_vmar {
        assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::OUT_OF_RANGE));
    } else {
        assert!(t.wait_for_failure());
    }
});

// Tests the ZX_INFO_VMO_PAGER_BACKED flag
#[test]
fn vmo_info_pager_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(page_size()).unwrap();

    // Check that the flag is set on a pager created vmo.
    let info = vmo.vmo().info().unwrap();
    assert!(info.flags.contains(zx::VmoInfoFlags::PAGER_BACKED));

    // Check that the flag isn't set on a regular vmo.
    let plain_vmo = zx::Vmo::create(page_size()).unwrap();
    let info = plain_vmo.info().unwrap();
    assert!(!info.flags.contains(zx::VmoInfoFlags::PAGER_BACKED));
}

// Tests that detaching results in a complete request.
#[test]
fn detach_page_complete_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();
    assert!(pager.detach_vmo(&vmo));
    assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));
}

// Tests that closing results in a complete request.
#[test]
fn close_page_complete_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();
    let key = vmo.key();
    pager.release_vmo(&vmo);
    drop(vmo);
    assert!(pager.wait_for_page_complete(key, INFINITE));
}

// Tests that accessing a VMO in non-mapping ways returns appropriate errors if detached.
#[test]
fn detach_non_mapping_access() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let vmo_result = Arc::new(parking_lot::Mutex::new(Ok(())));
    let (v, result) = (vmo.clone(), vmo_result.clone());
    let mut t = TestThread::new(move || {
        let mut val = [0u8; 8];
        // Do a read that strides two pages so we can succeed one and fail one.
        *result.lock() = v.vmo().read(&mut val, page_size() - 8 / 2);
        true
    });
    assert!(t.start());

    // Expect the whole range to be batched into one request.
    assert!(pager.wait_for_page_read(&vmo, 0, 2, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, 1));
    pager.detach_vmo(&vmo);
    assert!(t.wait_for_term());

    assert_eq!(*vmo_result.lock(), Err(zx::Status::BAD_STATE));

    // No page requests seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests that interrupting a read after receiving the request doesn't result in hanging threads.
fn read_interrupt_late_test(check_vmar: bool, detach: bool) {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));

    if detach {
        assert!(pager.detach_vmo(&vmo));
    } else {
        pager.close_pager_handle();
    }

    if check_vmar {
        assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::BAD_STATE));
    } else {
        assert!(t.wait_for_failure());
    }

    if detach {
        assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));
    }
}

vmo_vmar_test!(read_close_interrupt_late_test, |check_vmar| {
    read_interrupt_late_test(check_vmar, false)
});

vmo_vmar_test!(read_detach_interrupt_late_test, |check_vmar| {
    read_interrupt_late_test(check_vmar, true)
});

// Tests that interrupt a read before receiving requests doesn't result in hanging threads.
fn read_interrupt_early_test(check_vmar: bool, detach: bool) {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(t.wait_for_blocked());

    if detach {
        assert!(pager.detach_vmo(&vmo));
    } else {
        pager.close_pager_handle();
    }

    if check_vmar {
        assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::BAD_STATE));
    } else {
        assert!(t.wait_for_failure());
    }

    if detach {
        assert!(pager.wait_for_page_complete(vmo.key(), INFINITE));
    }
}

vmo_vmar_test!(read_close_interrupt_early_test, |check_vmar| {
    read_interrupt_early_test(check_vmar, false)
});

vmo_vmar_test!(read_detach_interrupt_early_test, |check_vmar| {
    read_interrupt_early_test(check_vmar, true)
});

// Tests that closing a pager while a thread is accessing it doesn't cause
// problems (other than a page fault in the accessing thread).
#[test]
fn close_pager_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, true));
    assert!(pager.supply_pages(&vmo, 1, 1));

    assert!(t.start());
    assert!(t.wait_for_blocked());

    pager.close_pager_handle();

    assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::BAD_STATE));
    assert!(check_buffer(&vmo, 1, 1, true));
}

// Tests that closing a pager while a vmo is being detached doesn't cause problems.
#[test]
fn detach_close_pager_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();
    assert!(pager.detach_vmo(&vmo));
    pager.close_pager_handle();
}

// Tests that closing an in use port doesn't cause issues (beyond no
// longer being able to receive requests).
#[test]
fn close_port_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(2).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, true));
    assert!(t.start());
    assert!(t.wait_for_blocked());

    pager.close_port_handle();

    assert!(pager.supply_pages(&vmo, 1, 1));
    assert!(check_buffer(&vmo, 1, 1, true));

    assert!(pager.detach_vmo(&vmo));
    assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::BAD_STATE));
}

// Tests that a commit is properly batched.
#[test]
fn simple_commit_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 555;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));
    assert!(t.wait());
}

// Tests that a commit over a partially populated range is properly split.
#[test]
fn split_commit_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 33;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    assert!(pager.supply_pages(&vmo, NUM_PAGES / 2, 1));

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());

    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES / 2, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES / 2));

    assert!(pager.wait_for_page_read(&vmo, NUM_PAGES / 2 + 1, NUM_PAGES / 2, INFINITE));
    assert!(pager.supply_pages(&vmo, NUM_PAGES / 2 + 1, NUM_PAGES / 2));

    assert!(t.wait());
}

// Tests that overlapping commits don't result in redundant requests.
#[test]
fn overlap_commit_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 32;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.commit(NUM_PAGES / 4, NUM_PAGES / 2));
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.commit(0, NUM_PAGES));

    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, NUM_PAGES / 4, NUM_PAGES / 2, INFINITE));

    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES / 4, INFINITE));

    assert!(pager.supply_pages(&vmo, 0, 3 * NUM_PAGES / 4));

    assert!(pager.wait_for_page_read(&vmo, 3 * NUM_PAGES / 4, NUM_PAGES / 4, INFINITE));
    assert!(pager.supply_pages(&vmo, 3 * NUM_PAGES / 4, NUM_PAGES / 4));

    assert!(t1.wait());
    assert!(t2.wait());
}

// Tests that overlapping commits are properly supplied.
#[test]
fn overlap_commit_supply_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const SUPPLY_LEN: u64 = 3;
    const COMMIT_LEN_A: u64 = 7;
    const COMMIT_LEN_B: u64 = 5;
    const NUM_PAGES: u64 = COMMIT_LEN_A * COMMIT_LEN_B * SUPPLY_LEN;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let mut ts_a = Vec::new();
    for i in 0..NUM_PAGES / COMMIT_LEN_A {
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.commit(i * COMMIT_LEN_A, COMMIT_LEN_A));
        assert!(t.start());
        assert!(pager.wait_for_page_read(&vmo, i * COMMIT_LEN_A, COMMIT_LEN_A, INFINITE));
        ts_a.push(t);
    }

    let mut ts_b = Vec::new();
    for i in 0..NUM_PAGES / COMMIT_LEN_B {
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.commit(i * COMMIT_LEN_B, COMMIT_LEN_B));
        assert!(t.start());
        assert!(t.wait_for_blocked());
        ts_b.push(t);
    }

    for i in 0..NUM_PAGES / SUPPLY_LEN {
        assert!(pager.supply_pages(&vmo, i * SUPPLY_LEN, SUPPLY_LEN));
    }

    for t in ts_a.iter_mut().chain(ts_b.iter_mut()) {
        assert!(t.wait());
    }
}

// Tests that a single commit can be fulfilled by multiple supplies.
#[test]
fn multisupply_commit_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 32;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    for i in 0..NUM_PAGES {
        assert!(pager.supply_pages(&vmo, i, 1));
    }
    assert!(t.wait());
}

// Tests that a single supply can fulfil multiple commits.
#[test]
fn multicommit_supply_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_COMMITS: u64 = 5;
    const NUM_SUPPLIES: u64 = 7;
    let vmo = pager.create_vmo(NUM_COMMITS * NUM_SUPPLIES).unwrap();

    let mut ts = Vec::new();
    for i in 0..NUM_COMMITS {
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.commit(i * NUM_SUPPLIES, NUM_SUPPLIES));
        assert!(t.start());
        assert!(pager.wait_for_page_read(&vmo, i * NUM_SUPPLIES, NUM_SUPPLIES, INFINITE));
        ts.push(t);
    }

    for i in 0..NUM_SUPPLIES {
        assert!(pager.supply_pages(&vmo, NUM_COMMITS * i, NUM_COMMITS));
    }

    for t in &mut ts {
        assert!(t.wait());
    }
}

// Tests that redundant supplies for a single commit don't cause errors.
#[test]
fn commit_redundant_supply_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 8;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    for i in 1..=NUM_PAGES {
        assert!(pager.supply_pages(&vmo, 0, i));
    }
    assert!(t.wait());
}

// Test that resizing out from under a commit is handled.
#[test]
fn resize_commit_test() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo_with_options(3, zx::VmoOptions::RESIZABLE).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, 3));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 3, INFINITE));

    // Supply one of the pages that will be removed.
    assert!(pager.supply_pages(&vmo, 2, 1));

    // Truncate the VMO.
    assert!(vmo.resize(1));

    // Make sure the thread is still blocked (i.e. check the accounting
    // w.r.t. the page that was removed).
    assert!(t.wait_for_blocked());

    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(t.wait());

    // Make sure there are no extra requests.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

fn raw(handle: &impl zx::AsHandleRef) -> zx::sys::zx_handle_t {
    handle.as_handle_ref().raw_handle()
}

macro_rules! duplicate {
    ($handle:expr, $rights:expr) => {
        $handle.duplicate_handle(zx::Rights::from_bits_retain($rights)).unwrap()
    };
}

// Tests API violations for pager_create.
#[test]
fn invalid_pager_create() {
    let mut handle = 0;
    // bad options
    // SAFETY: `handle` is valid for writes.
    assert_eq!(unsafe { zx::sys::zx_pager_create(1, &mut handle) }, zx::sys::ZX_ERR_INVALID_ARGS);
}

// Tests API violations for pager_create_vmo.
#[test]
fn invalid_pager_create_vmo() {
    use zircon_core_tests::userpager::rights::*;
    use zx::sys::*;
    let mut pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let mut vmo = 0;
    let ps = page_size();
    // SAFETY: `vmo` is valid for writes.
    let create = |pager: zx_handle_t,
                  options: u32,
                  port: zx_handle_t,
                  size: u64,
                  vmo: &mut zx_handle_t| unsafe {
        zx_pager_create_vmo(pager, options, port, 0, size, vmo)
    };

    // bad options
    assert_eq!(create(raw(&pager), !0u32, raw(&port), ps, &mut vmo), ZX_ERR_INVALID_ARGS);

    // bad handles for pager and port
    assert_eq!(create(ZX_HANDLE_INVALID, 0, raw(&port), ps, &mut vmo), ZX_ERR_BAD_HANDLE);
    assert_eq!(create(raw(&pager), 0, ZX_HANDLE_INVALID, ps, &mut vmo), ZX_ERR_BAD_HANDLE);

    // missing write right on port
    let ro_port = duplicate!(port, ZX_DEFAULT_PORT_RIGHTS & !ZX_RIGHT_WRITE);
    assert_eq!(create(raw(&pager), 0, raw(&ro_port), ps, &mut vmo), ZX_ERR_ACCESS_DENIED);

    // bad handle types for pager and port
    assert_eq!(create(raw(&port), 0, raw(&port), ps, &mut vmo), ZX_ERR_WRONG_TYPE);
    // writability handle 2 is checked before the type, so use a new vmo
    let tmp_vmo = zx::Vmo::create(ps).unwrap();
    assert_eq!(create(raw(&pager), 0, raw(&tmp_vmo), ps, &mut vmo), ZX_ERR_WRONG_TYPE);

    // invalid size
    let bad_size = (u64::MAX / ps * ps).wrapping_add(1);
    assert_eq!(create(raw(&pager), 0, raw(&port), bad_size, &mut vmo), ZX_ERR_OUT_OF_RANGE);

    // missing pager rights
    pager = pager
        .replace_handle(zx::Rights::from_bits_retain(
            ZX_DEFAULT_PAGER_RIGHTS & !ZX_RIGHT_ATTACH_VMO,
        ))
        .unwrap();
    assert_eq!(create(raw(&pager), 0, raw(&port), ps, &mut vmo), ZX_ERR_ACCESS_DENIED);
}

// Tests API violations for pager_detach_vmo.
#[test]
fn invalid_pager_detach_vmo() {
    use zircon_core_tests::userpager::rights::*;
    use zx::sys::*;
    let mut pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let ps = page_size();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, ps).unwrap();
    // SAFETY: Detaching takes handle values.
    let detach = |pager: zx_handle_t, vmo: zx_handle_t| unsafe { zx_pager_detach_vmo(pager, vmo) };

    // bad handles
    assert_eq!(detach(ZX_HANDLE_INVALID, raw(&vmo)), ZX_ERR_BAD_HANDLE);
    assert_eq!(detach(raw(&pager), ZX_HANDLE_INVALID), ZX_ERR_BAD_HANDLE);

    // wrong handle types
    assert_eq!(detach(raw(&vmo), raw(&vmo)), ZX_ERR_WRONG_TYPE);
    assert_eq!(detach(raw(&pager), raw(&pager)), ZX_ERR_WRONG_TYPE);

    // missing rights
    let ro_vmo = duplicate!(vmo, ZX_DEFAULT_VMO_RIGHTS & !ZX_RIGHT_WRITE);
    assert_eq!(detach(raw(&pager), raw(&ro_vmo)), ZX_ERR_ACCESS_DENIED);

    // detaching a non-paged vmo
    let tmp_vmo = zx::Vmo::create(ps).unwrap();
    assert_eq!(detach(raw(&pager), raw(&tmp_vmo)), ZX_ERR_INVALID_ARGS);

    // detaching with the wrong pager
    let mut pager2 = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    assert_eq!(detach(raw(&pager2), raw(&vmo)), ZX_ERR_INVALID_ARGS);

    // missing pager rights
    pager = pager
        .replace_handle(zx::Rights::from_bits_retain(
            ZX_DEFAULT_PAGER_RIGHTS & !ZX_RIGHT_ATTACH_VMO,
        ))
        .unwrap();
    assert_eq!(detach(raw(&pager), raw(&vmo)), ZX_ERR_ACCESS_DENIED);

    let vmo = pager2.create_vmo(zx::VmoOptions::empty(), &port, 0, ps).unwrap();
    pager2 = pager2
        .replace_handle(zx::Rights::from_bits_retain(
            ZX_DEFAULT_PAGER_RIGHTS & !ZX_RIGHT_MANAGE_VMO,
        ))
        .unwrap();
    assert_eq!(detach(raw(&pager2), raw(&vmo)), ZX_ERR_ACCESS_DENIED);
}

// Tests API violations for supply_pages.
#[test]
fn invalid_pager_supply_pages() {
    use zircon_core_tests::userpager::rights::*;
    use zx::sys::*;
    let mut pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let ps = page_size();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, ps).unwrap();
    let mut aux_vmo = zx::Vmo::create(ps).unwrap();
    let supply = |pager: zx_handle_t,
                  vmo: zx_handle_t,
                  offset: u64,
                  len: u64,
                  aux: zx_handle_t,
                  aux_offset: u64| {
        // SAFETY: Supplying takes handle values.
        unsafe { zx_pager_supply_pages(pager, vmo, offset, len, aux, aux_offset) }
    };

    // bad handles
    assert_eq!(supply(ZX_HANDLE_INVALID, raw(&vmo), 0, 0, raw(&aux_vmo), 0), ZX_ERR_BAD_HANDLE);
    assert_eq!(supply(raw(&pager), ZX_HANDLE_INVALID, 0, 0, raw(&aux_vmo), 0), ZX_ERR_BAD_HANDLE);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 0, ZX_HANDLE_INVALID, 0), ZX_ERR_BAD_HANDLE);

    // wrong handle types
    assert_eq!(supply(raw(&vmo), raw(&vmo), 0, 0, raw(&aux_vmo), 0), ZX_ERR_WRONG_TYPE);
    assert_eq!(supply(raw(&pager), raw(&pager), 0, 0, raw(&aux_vmo), 0), ZX_ERR_WRONG_TYPE);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 0, raw(&port), 0), ZX_ERR_WRONG_TYPE);

    // using a non-paged vmo
    assert_eq!(supply(raw(&pager), raw(&aux_vmo), 0, 0, raw(&aux_vmo), 0), ZX_ERR_INVALID_ARGS);

    // using a pager vmo from another pager
    let pager2 = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    assert_eq!(supply(raw(&pager2), raw(&vmo), 0, 0, ZX_HANDLE_INVALID, 0), ZX_ERR_INVALID_ARGS);

    // missing permissions on the aux vmo
    let ro_vmo = duplicate!(aux_vmo, ZX_DEFAULT_VMO_RIGHTS & !ZX_RIGHT_WRITE);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 0, raw(&ro_vmo), 0), ZX_ERR_ACCESS_DENIED);
    let wo_vmo = duplicate!(aux_vmo, ZX_DEFAULT_VMO_RIGHTS & !ZX_RIGHT_READ);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 0, raw(&wo_vmo), 0), ZX_ERR_ACCESS_DENIED);

    // missing permissions on the pager vmo
    let ro_pager_vmo = duplicate!(vmo, ZX_DEFAULT_VMO_RIGHTS & !ZX_RIGHT_WRITE);
    assert_eq!(
        supply(raw(&pager), raw(&ro_pager_vmo), 0, 0, raw(&aux_vmo), 0),
        ZX_ERR_ACCESS_DENIED
    );

    // misaligned offset, size, or aux alignment
    assert_eq!(supply(raw(&pager), raw(&vmo), 1, 0, raw(&aux_vmo), 0), ZX_ERR_INVALID_ARGS);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 1, raw(&aux_vmo), 0), ZX_ERR_INVALID_ARGS);
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, 0, raw(&aux_vmo), 1), ZX_ERR_INVALID_ARGS);

    // There is no system resource, so physical VMOs and pinning are not exercised.

    // violations of conditions for taking pages from a vmo: taking them from a pager vmo.
    {
        let aux_vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, ps).unwrap();
        let alt_vmo = zx::Vmo::create(ps).unwrap();
        alt_vmo.op_range(zx::VmoOp::COMMIT, 0, ps).unwrap();
        assert_eq!(supply(raw(&pager), raw(&aux_vmo), 0, ps, raw(&alt_vmo), 0), ZX_OK);
        assert_eq!(supply(raw(&pager), raw(&vmo), 0, ps, raw(&aux_vmo), 0), ZX_ERR_NOT_SUPPORTED);
    }

    // out of range pager_vmo region
    aux_vmo.op_range(zx::VmoOp::COMMIT, 0, ps).unwrap();
    assert_eq!(supply(raw(&pager), raw(&vmo), ps, ps, raw(&aux_vmo), 0), ZX_ERR_OUT_OF_RANGE);

    // out of range aux_vmo region
    aux_vmo = zx::Vmo::create(ps).unwrap();
    aux_vmo.op_range(zx::VmoOp::COMMIT, 0, ps).unwrap();
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, ps, raw(&aux_vmo), ps), ZX_ERR_OUT_OF_RANGE);

    // missing pager rights
    pager = pager
        .replace_handle(zx::Rights::from_bits_retain(
            ZX_DEFAULT_PAGER_RIGHTS & !ZX_RIGHT_MANAGE_VMO,
        ))
        .unwrap();
    assert_eq!(supply(raw(&pager), raw(&vmo), 0, ps, raw(&aux_vmo), 0), ZX_ERR_ACCESS_DENIED);
}

// Tests that supply_pages works when the source is mapped.
#[test]
fn mapped_supply_pages() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, page_size()).unwrap();
    let aux_vmo = zx::Vmo::create(page_size()).unwrap();

    // Map the aux vmo.
    let addr = root_vmar()
        .map(
            0,
            &aux_vmo,
            0,
            page_size_usize(),
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        )
        .unwrap();

    // Write something to the aux vmo that can be verified later.
    const DATA: u8 = 0xcc;
    // SAFETY: The page is mapped.
    unsafe { (addr as *mut u8).write_volatile(DATA) };

    pager.supply_pages(&vmo, 0..page_size(), &aux_vmo, 0).unwrap();

    // Verify that the right page was moved over.
    let mut buf = [0u8; 1];
    vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf[0], DATA);

    // The mapped address should now read zero.
    // SAFETY: The page is mapped.
    assert_eq!(unsafe { (addr as *const u8).read_volatile() }, 0);
    // SAFETY: Nothing refers to the mapping any more.
    unsafe { root_vmar().unmap(addr, page_size_usize()) }.unwrap();
}

// Tests that resizing a non-resizable pager vmo fails.
#[test]
fn resize_nonresizable_vmo() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, page_size()).unwrap();
    assert_eq!(vmo.set_size(2 * page_size()), Err(zx::Status::UNAVAILABLE));
}

// Tests that decommiting fails. The part of the test decommiting a clone is not ported.
#[test]
fn decommit_test() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, page_size()).unwrap();
    assert_eq!(vmo.op_range(zx::VmoOp::DECOMMIT, 0, page_size()), Err(zx::Status::NOT_SUPPORTED));
}

// Test that supplying uncommitted pages prevents faults.
#[test]
fn uncommitted_supply() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, page_size()).unwrap();
    let empty = zx::Vmo::create(page_size()).unwrap();
    pager.supply_pages(&vmo, 0..page_size(), &empty, 0).unwrap();

    // A read should not fault and give zeros.
    let mut val = 42u32.to_ne_bytes();
    vmo.read(&mut val, 0).unwrap();
    assert_eq!(u32::from_ne_bytes(val), 0);
}

// Simple test for a ZX_PAGER_OP_FAIL on a single page, accessed from a single thread.
// Tests both cases, where the client accesses the vmo directly, and where the client has the vmo
// mapped in a vmar.
vmo_vmar_test!(fail_single_page, |check_vmar| {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(1).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || check_buffer(&v, 0, 1, check_vmar));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.fail_pages(&vmo, 0, 1, zx::Status::IO));

    if check_vmar {
        // Verify that the thread crashes if the page was accessed via a vmar.
        assert!(t.wait_for_crash(vmo.base_addr(), zx::Status::IO));
    } else {
        // Verify that the vmo read fails if the thread directly accessed the vmo.
        assert!(t.wait_for_failure());
    }
    // Make sure there are no extra requests.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
});

// Tests failing the exact range requested.
#[test]
fn fail_exact_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    assert!(pager.fail_pages(&vmo, 0, NUM_PAGES, zx::Status::IO));

    // Failing the pages will cause the COMMIT to fail.
    assert!(t.wait_for_failure());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests that multiple page requests can be failed at once.
#[test]
fn fail_multiple_commits() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    // Multiple threads requesting disjoint ranges.
    let mut threads = Vec::new();
    for i in 0..NUM_PAGES {
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.commit(i, 1));
        assert!(t.start());
        threads.push(t);
    }
    for (i, t) in threads.iter().enumerate() {
        assert!(t.wait_for_blocked());
        assert!(pager.wait_for_page_read(&vmo, i as u64, 1, INFINITE));
    }

    // Fail the entire range.
    assert!(pager.fail_pages(&vmo, 0, NUM_PAGES, zx::Status::IO));
    for t in &mut threads {
        assert!(t.wait_for_failure());
    }
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Multiple threads requesting the same range.
    let mut threads = Vec::new();
    for _ in 0..NUM_PAGES {
        let v = vmo.clone();
        let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
        assert!(t.start());
        threads.push(t);
    }
    for t in &threads {
        assert!(t.wait_for_blocked());
    }

    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    // No more requests seen as after the first all the others should overlap.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Fail the entire range.
    assert!(pager.fail_pages(&vmo, 0, NUM_PAGES, zx::Status::IO));
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
    for t in &mut threads {
        assert!(t.wait_for_failure());
    }
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests failing multiple vmos.
#[test]
fn fail_multiple_vmos() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let vmo1 = pager.create_vmo(1).unwrap();
    let v = vmo1.clone();
    let mut t1 = TestThread::new(move || v.commit(0, 1));
    let vmo2 = pager.create_vmo(1).unwrap();
    let v = vmo2.clone();
    let mut t2 = TestThread::new(move || v.commit(0, 1));

    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo1, 0, 1, INFINITE));

    // No page requests for vmo2 yet.
    assert!(pager.get_page_read_request(&vmo2, INFINITE_PAST).is_none());

    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo2, 0, 1, INFINITE));

    // Fail vmo1.
    assert!(pager.fail_pages(&vmo1, 0, 1, zx::Status::IO));
    assert!(t1.wait_for_failure());

    // No more requests for vmo1.
    assert!(pager.get_page_read_request(&vmo1, INFINITE_PAST).is_none());

    // Fail vmo2.
    assert!(pager.fail_pages(&vmo2, 0, 1, zx::Status::IO));
    assert!(t2.wait_for_failure());

    // No more requests for either vmo1 or vmo2.
    assert!(pager.get_page_read_request(&vmo1, INFINITE_PAST).is_none());
    assert!(pager.get_page_read_request(&vmo2, INFINITE_PAST).is_none());
}

// Tests failing a range overlapping with a page request when using commit.
#[test]
fn fail_overlapping_range_commit() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    // End of the request range overlaps with the failed range.
    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.commit(0, 2));
    // The start of the request range overlaps with the failed range.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.commit(9, 2));
    // The entire request range overlaps with the failed range.
    let v = vmo.clone();
    let mut t3 = TestThread::new(move || v.commit(5, 2));

    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 2, INFINITE));
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 9, 2, INFINITE));
    assert!(t3.start());
    assert!(pager.wait_for_page_read(&vmo, 5, 2, INFINITE));

    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(pager.fail_pages(&vmo, 1, 9, zx::Status::IO));
    assert!(pager.supply_pages(&vmo, 10, 1));

    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.fail_pages(&vmo, 1, 9, zx::Status::IO));

    assert!(t1.wait_for_failure());
    assert!(t2.wait_for_failure());
    assert!(t3.wait_for_failure());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests failing a range overlapping with a page request when using vmo_read.
#[test]
fn fail_overlapping_range_read() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let read_helper = |vmo: Arc<Vmo>, page_offset: u64, page_count: u64| {
        move || {
            let mut data = vec![0u8; (page_count * page_size()) as usize];
            vmo.vmo().read(&mut data, page_offset * page_size()).is_ok()
        }
    };

    // End of the request range overlaps with the failed range.
    let mut t1 = TestThread::new(read_helper(vmo.clone(), 0, 2));
    // The start of the request range overlaps with the failed range.
    let mut t2 = TestThread::new(read_helper(vmo.clone(), 9, 2));
    // The entire request range overlaps with the failed range.
    let mut t3 = TestThread::new(read_helper(vmo.clone(), 5, 2));

    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 2, INFINITE));
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 9, 2, INFINITE));
    assert!(t3.start());
    assert!(pager.wait_for_page_read(&vmo, 5, 2, INFINITE));

    assert!(pager.fail_pages(&vmo, 1, 9, zx::Status::IO));
    assert!(pager.supply_pages(&vmo, 0, 1));
    assert!(pager.supply_pages(&vmo, 10, 1));

    assert!(pager.wait_for_page_read(&vmo, 1, 1, INFINITE));
    assert!(pager.fail_pages(&vmo, 1, 9, zx::Status::IO));

    assert!(t1.wait_for_failure());
    assert!(t2.wait_for_failure());
    assert!(t3.wait_for_failure());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests failing the requested range via multiple pager_op_range calls - after the first one, the
// rest are redundant.
#[test]
fn fail_redundant() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

    for i in 0..NUM_PAGES {
        // The first call with i = 0 should cause the thread to cause.
        // The following calls are no-ops.
        assert!(pager.fail_pages(&vmo, i, 1, zx::Status::IO));
    }

    assert!(t.wait_for_failure());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests that failing a range after the vmo is detached fails.
#[test]
fn fail_after_detach() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

    assert!(pager.detach_vmo(&vmo));
    // Detaching the vmo should cause the COMMIT to fail.
    assert!(t.wait_for_failure());

    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
    assert!(!pager.fail_pages(&vmo, 0, NUM_PAGES, zx::Status::IO));
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

#[test]
fn fail_early_wake() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(3).unwrap();

    // To account for pages potentially getting evicted start a page fault handler to re-supply
    // anything.
    vmo.set_page_fault_supply_limit(0);
    assert!(pager.start_tagged_page_fault_handler());

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.check_vmo(0, 3, None));
    assert!(t.start());
    assert!(t.wait_for_blocked());

    // Supply the first page, this should wake up the internal kernel request and process that
    // page.
    vmo.set_page_fault_supply_limit(1);
    assert!(pager.supply_pages_at(&vmo, 0, 1, 0));

    // Wait for the thread to have completed processing that page and be blocked again. This
    // ensures that we do not race by failing the pages before it blocks, causing it to keep
    // waiting.
    assert!(t.wait_for_blocked());

    // Fail the second page, although this isn't the first page in the request, due to the early
    // wake from the first request it is the next page to be processed and so should complete and
    // fail the request, even though we have not supplied or failed the third page.
    assert!(pager.fail_pages(&vmo, 1, 1, zx::Status::IO));

    assert!(t.wait_for_failure());
}

// Fail overlap request
#[test]
fn fail_overlap_request() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    let vmo = pager.create_vmo(4).unwrap();

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.check_vmo(0, 4, None));
    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 4, INFINITE));

    // Start another read, this one should be considered overlapping and not generate a request.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.check_vmo(1, 1, None));
    assert!(t2.start());
    assert!(t2.wait_for_blocked());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Fail the second page, this will set the failure status for the overlap, but it will not
    // wake up till the primary request wakes.
    assert!(pager.fail_pages(&vmo, 1, 1, zx::Status::IO));

    // Now succeed the first page, this will early wake the first the request and cause it to
    // process, but then continue waiting for the rest of the request.
    assert!(pager.supply_pages_at(&vmo, 0, 1, 0));
    assert!(t1.wait_for_blocked());
    // The overlap is still blocked
    assert!(t2.wait_for_blocked());

    // Succeed the last two pages.
    assert!(pager.supply_pages_at(&vmo, 2, 2, 2));

    // Both requests should wake up, the overlap gets its failure and the primary, having forgotten
    // where the error was, generates another request.
    assert!(t2.wait_for_failure());
    // The exact amount the will be waited for is slightly arbitrary, it will be at least one page
    // but the two end pages previously supplied could also have been evicted.
    let (offset, length) = pager.get_page_read_request(&vmo, INFINITE).unwrap();
    assert_eq!(offset, 1);
    assert!(length >= 1);

    // Succeed it this time.
    assert!(pager.supply_pages_at(&vmo, 1, 3, 1));

    // Should be complete now.
    assert!(t1.wait());
}

// Test that supply_pages fails after detach.
#[test]
fn supply_after_detach() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    assert!(pager.detach_vmo(&vmo));

    // No page requests seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
    assert!(!pager.supply_pages(&vmo, 0, NUM_PAGES));
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests that a supply_pages succeeds after failing i.e. a fail is not fatal.
#[test]
fn supply_after_fail() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 11;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let v = vmo.clone();
    let mut t1 = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));
    assert!(pager.fail_pages(&vmo, 0, NUM_PAGES, zx::Status::IO));
    assert!(t1.wait_for_failure());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    // Try to COMMIT the failed range again.
    let v = vmo.clone();
    let mut t2 = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t2.start());
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

    // This should supply the pages as expected.
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));
    assert!(t2.wait());
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

// Tests that the error code passed in when failing is correctly propagated.
#[test]
fn fail_error_code() {
    let valid_errors = [
        zx::Status::IO,
        zx::Status::IO_DATA_INTEGRITY,
        zx::Status::BAD_STATE,
        zx::Status::NO_SPACE,
        zx::Status::BUFFER_TOO_SMALL,
    ];
    for valid_error in valid_errors {
        let mut pager = UserPager::new();
        assert!(pager.init());
        const NUM_PAGES: u64 = 11;
        let vmo = pager.create_vmo(NUM_PAGES).unwrap();

        let status_commit = Arc::new(parking_lot::Mutex::new(Ok(())));
        let (v, status) = (vmo.clone(), status_commit.clone());
        let mut t_commit = TestThread::new(move || {
            // |status_commit| should get set to the error code passed in via FailPages.
            let result = v.vmo().op_range(zx::VmoOp::COMMIT, 0, NUM_PAGES * page_size());
            *status.lock() = result;
            result.is_ok()
        });

        let status_read = Arc::new(parking_lot::Mutex::new(Ok(())));
        let (v, status) = (vmo.clone(), status_read.clone());
        let mut t_read = TestThread::new(move || {
            let len = (NUM_PAGES * page_size()) as usize;
            let Ok(tmp_vmo) = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, len as u64)
            else {
                return false;
            };
            let Ok(buf) = root_vmar().map(
                0,
                &tmp_vmo,
                0,
                len,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            ) else {
                return false;
            };
            // SAFETY: The buffer is mapped until it is unmapped below.
            let data = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, len) };
            // |status_read| should get set to the error code passed in via FailPages.
            let result = v.vmo().read(data, 0);
            *status.lock() = result;
            // SAFETY: Nothing refers to the mapping any more.
            let _ = unsafe { root_vmar().unmap(buf, len) };
            result.is_ok()
        });

        assert!(t_commit.start());
        assert!(t_commit.wait_for_blocked());
        assert!(t_read.start());
        assert!(t_read.wait_for_blocked());
        assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

        // Fail with a specific valid error code.
        assert!(pager.fail_pages(&vmo, 0, NUM_PAGES, valid_error));

        assert!(t_commit.wait_for_failure());
        // Verify that op_range(ZX_VMO_OP_COMMIT) returned the provided error code.
        assert_eq!(*status_commit.lock(), Err(valid_error));

        assert!(t_read.wait_for_failure());
        // Verify that vmo_read() returned the provided error code.
        assert_eq!(*status_read.lock(), Err(valid_error));

        assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
    }
}

// Tests that the ZX_VMO_OP_ALWAYS_NEED hint works as expected.
#[test]
fn eviction_hint_always_need() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 30;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    // Hint ALWAYS_NEED on 5 pages starting at page 10. This will commit those pages and we should
    // see pager requests.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        v.vmo().op_range(zx::VmoOp::ALWAYS_NEED, 10 * page_size(), 5 * page_size()).is_ok()
    });
    assert!(t.start());

    // Verify read requests for pages [10,15).
    assert!(pager.wait_for_page_read(&vmo, 10, 5, INFINITE));
    assert!(pager.supply_pages(&vmo, 10, 5));

    // The thread should now successfully terminate.
    assert!(t.wait());
}

// Tests that the zx_vmo_op_range() API succeeds and fails as expected for hints.
#[test]
fn eviction_hints_op_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 20;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 10));
    let ps = page_size();

    // Trivial success cases.
    vmo.vmo().op_range(zx::VmoOp::ALWAYS_NEED, 0, 10 * ps).unwrap();
    vmo.vmo().op_range(zx::VmoOp::DONT_NEED, 0, 20 * ps).unwrap();

    // Verify that offsets get aligned to page boundaries.
    let v = vmo.clone();
    let mut t =
        TestThread::new(move || v.vmo().op_range(zx::VmoOp::ALWAYS_NEED, 15 * ps - 8, 16).is_ok());
    assert!(t.start());

    // We should see read requests for pages 14 and 15.
    assert!(pager.wait_for_page_read(&vmo, 14, 2, INFINITE));
    assert!(pager.supply_pages(&vmo, 14, 2));
    assert!(t.wait());

    vmo.vmo().op_range(zx::VmoOp::DONT_NEED, 32, 20 * ps - 64).unwrap();

    // Hinting an invalid range should fail.
    assert_eq!(
        vmo.vmo().op_range(zx::VmoOp::ALWAYS_NEED, 15 * ps, 10 * ps),
        Err(zx::Status::OUT_OF_RANGE)
    );
    assert_eq!(
        vmo.vmo().op_range(zx::VmoOp::DONT_NEED, NUM_PAGES * ps, 20 * ps),
        Err(zx::Status::OUT_OF_RANGE)
    );
    assert_eq!(
        vmo.vmo().op_range(zx::VmoOp::ALWAYS_NEED, ps, u64::MAX),
        Err(zx::Status::OUT_OF_RANGE)
    );
    assert_eq!(
        vmo.vmo().op_range(zx::VmoOp::DONT_NEED, ps, u64::MAX),
        Err(zx::Status::OUT_OF_RANGE)
    );

    // Hinting trivially succeeds for non-pager VMOs too. It will have no effect internally.
    let vmo2 = zx::Vmo::create(NUM_PAGES).unwrap();
    vmo2.op_range(zx::VmoOp::ALWAYS_NEED, 0, NUM_PAGES * ps).unwrap();
    vmo2.op_range(zx::VmoOp::DONT_NEED, 0, NUM_PAGES * ps).unwrap();
}

// Tests that the ALWAYS_NEED hint works as expected with a racing VMO resize.
#[test]
fn eviction_hints_with_resize() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 20;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::RESIZABLE).unwrap();

    // Hint ALWAYS_NEED on 10 pages starting at page 10. This will try to commit those pages and we
    // should see pager requests.
    let v = vmo.clone();
    let mut t = TestThread::new(move || {
        v.vmo().op_range(zx::VmoOp::ALWAYS_NEED, 10 * page_size(), 10 * page_size()).is_ok()
    });
    assert!(t.start());

    // Supply a couple of pages, and then resize down across the hinted range, cutting it short.
    assert!(pager.wait_for_page_read(&vmo, 10, 10, INFINITE));
    assert!(pager.supply_pages(&vmo, 10, 2));
    vmo.vmo().set_size(12 * page_size()).unwrap();

    // The hinting range should terminate now.
    assert!(t.wait());

    // No more page requests are seen.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
}

/// A VMAR destroyed when dropped.
struct TempVmar {
    vmar: Arc<zx::Vmar>,
    base: usize,
}

impl TempVmar {
    fn allocate(size: usize) -> Self {
        let (vmar, base) = root_vmar()
            .allocate(
                0,
                size,
                zx::VmarFlags::CAN_MAP_SPECIFIC
                    | zx::VmarFlags::CAN_MAP_READ
                    | zx::VmarFlags::CAN_MAP_WRITE,
            )
            .unwrap();
        Self { vmar: Arc::new(vmar), base }
    }
}

impl Drop for TempVmar {
    fn drop(&mut self) {
        // SAFETY: Nothing refers to the mappings any more.
        let _ = unsafe { self.vmar.destroy() };
    }
}

// Tests that hints work as expected via zx_vmar_op_range().
#[test]
fn eviction_hints_vmar() {
    // Create a temporary VMAR to work with.
    let vmar_size = 15 * page_size_usize();
    let temp = TempVmar::allocate(vmar_size);
    let (vmar, base_addr) = (temp.vmar.clone(), temp.base);

    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create two pager VMOs.
    const NUM_PAGES: u64 = 3;
    let vmo1 = pager.create_vmo(NUM_PAGES).unwrap();
    let vmo2 = pager.create_vmo(NUM_PAGES).unwrap();

    // Map the two VMOs with no gap in between.
    let vmo_size = (NUM_PAGES * page_size()) as usize;
    let flags = zx::VmarFlags::SPECIFIC | zx::VmarFlags::PERM_READ;
    let addr1 = vmar.map(0, vmo1.vmo(), 0, vmo_size, flags).unwrap();
    assert_eq!(addr1, base_addr);
    let addr2 = vmar.map(vmo_size, vmo2.vmo(), 0, vmo_size, flags).unwrap();
    assert_eq!(addr2, base_addr + vmo_size);

    // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted
    // pages.
    assert!(pager.supply_pages(&vmo1, 1, 1));
    assert!(pager.supply_pages(&vmo2, 1, 1));

    // Also map in a non pager-backed VMO to work with.
    let anon_vmo = zx::Vmo::create(vmo_size as u64).unwrap();
    let addr3 = vmar.map(2 * vmo_size, &anon_vmo, 0, vmo_size, flags).unwrap();
    assert_eq!(addr3, base_addr + 2 * vmo_size);

    vmar.op_range(zx::VmarOp::DONT_NEED, base_addr, 3 * vmo_size).unwrap();

    let v = vmar.clone();
    let mut t1 = TestThread::new(move || {
        v.op_range(zx::VmarOp::ALWAYS_NEED, base_addr, 3 * vmo_size).is_ok()
    });
    assert!(t1.start());

    // We should see page requests for both VMOs, however we do not know if the pages we marked
    // DONT_NEED got evicted or not. As such there are three page request outcomes we might see
    //  1. Single request for all three pages (page was evicted before our ALWAYS_NEED op)
    //  2. Request for page 1 then request for pages 2 and 3 (page was evicted while servicing
    //     request for page 1)
    //  3. Request for page 1 then request for page 3 (page was not evicted at all)
    let wait_for_pages = |vmo: &Vmo| {
        let (req_offset, req_count) = pager.get_page_read_request(vmo, INFINITE).unwrap();
        // First page is always absent, so our request should be for that.
        assert_eq!(req_offset, 0);
        // 1 or 3 pages
        assert!([1, 3].contains(&req_count));
        pager.supply_pages(vmo, 0, req_count);
        if req_count == 3 {
            return;
        }
        // The next request is either just the last page, or the last two pages.
        let (req_offset, req_count) = pager.get_page_read_request(vmo, INFINITE).unwrap();
        if req_offset == 1 {
            assert_eq!(req_count, 2);
            pager.supply_pages(vmo, 1, 2);
        } else {
            assert_eq!((req_offset, req_count), (2, 1));
            pager.supply_pages(vmo, 2, 1);
        }
    };
    wait_for_pages(&vmo1);
    wait_for_pages(&vmo2);

    assert!(t1.wait());

    // This is redundant, but hinting again is harmless and should succeed.
    vmar.op_range(zx::VmarOp::ALWAYS_NEED, base_addr, 3 * vmo_size).unwrap();
    vmar.op_range(zx::VmarOp::DONT_NEED, base_addr, 3 * vmo_size).unwrap();

    // Can't hint on gaps in the VMAR.
    assert_eq!(
        vmar.op_range(zx::VmarOp::DONT_NEED, base_addr, vmar_size),
        Err(zx::Status::BAD_STATE)
    );
}

// Tests that hints work as expected via zx_vmar_op_range(), when working with a nested VMAR tree.
#[test]
fn eviction_hints_nested_vmar() {
    // Create a temporary VMAR to work with.
    let vmar_size = 10 * page_size_usize();
    let temp = TempVmar::allocate(vmar_size);
    let (vmar, base_addr) = (temp.vmar.clone(), temp.base);

    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create two pager VMOs.
    const NUM_PAGES: u64 = 3;
    let vmo_size = (NUM_PAGES * page_size()) as usize;
    let vmo1 = pager.create_vmo(NUM_PAGES).unwrap();
    let vmo2 = pager.create_vmo(NUM_PAGES).unwrap();

    // Create two sub-VMARs to hold the mappings, with no gap between them.
    let sub_flags = zx::VmarFlags::CAN_MAP_SPECIFIC
        | zx::VmarFlags::CAN_MAP_READ
        | zx::VmarFlags::CAN_MAP_WRITE
        | zx::VmarFlags::SPECIFIC;
    let (sub_vmar1, base_addr1) = vmar.allocate(0, vmo_size, sub_flags).unwrap();
    assert_eq!(base_addr1, base_addr);
    let (sub_vmar2, base_addr2) = vmar.allocate(vmo_size, vmo_size, sub_flags).unwrap();
    assert_eq!(base_addr2, base_addr + vmo_size);

    // Map the two VMOs in the two sub-VMARs.
    let flags = zx::VmarFlags::SPECIFIC | zx::VmarFlags::PERM_READ;
    assert_eq!(sub_vmar1.map(0, vmo1.vmo(), 0, vmo_size, flags).unwrap(), base_addr1);
    assert_eq!(sub_vmar2.map(0, vmo2.vmo(), 0, vmo_size, flags).unwrap(), base_addr2);

    // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted
    // pages.
    assert!(pager.supply_pages(&vmo1, 1, 1));
    assert!(pager.supply_pages(&vmo2, 1, 1));

    vmar.op_range(zx::VmarOp::DONT_NEED, base_addr, 2 * vmo_size).unwrap();

    let v = vmar.clone();
    let mut t1 = TestThread::new(move || {
        v.op_range(zx::VmarOp::ALWAYS_NEED, base_addr, 2 * vmo_size).is_ok()
    });
    assert!(t1.start());

    // We should see page requests for both VMOs.
    for vmo in [&vmo1, &vmo2] {
        assert!(pager.wait_for_page_read(vmo, 0, 1, INFINITE));
        assert!(pager.supply_pages(vmo, 0, 1));
        // The next page was committed and then marked DONT_NEED and could have been evicted
        // already, so get the next request manually and see where we're at.
        let (req_offset, _) = pager.get_page_read_request(vmo, INFINITE).unwrap();
        if req_offset == 1 {
            pager.supply_pages(vmo, 1, 1);
            assert!(pager.wait_for_page_read(vmo, 2, 1, INFINITE));
        } else {
            assert_eq!(req_offset, 2);
        }
        assert!(pager.supply_pages(vmo, 2, 1));
    }
    assert!(t1.wait());
}

// Limits a test loop to a number of iterations or a time, as `vmo_test::TestLimiter` does.
struct TestLimiter {
    iterations: u32,
    max_iterations: u32,
    deadline: zx::MonotonicInstant,
}

impl TestLimiter {
    fn new(max_iterations: u32, max_duration: zx::MonotonicDuration) -> Self {
        Self { iterations: 0, max_iterations, deadline: zx::MonotonicInstant::after(max_duration) }
    }

    fn finished(&self) -> bool {
        self.iterations >= self.max_iterations || zx::MonotonicInstant::get() >= self.deadline
    }

    fn next(&mut self) {
        self.iterations += 1;
    }
}

#[test]
fn zero_handles_race() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    let mut pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();

    let running = Arc::new(AtomicBool::new(true));
    // Keep the most recent pager handle stashed in an atomic. This lets the test synchronize the
    // handle value without causing undefined behavior with racy memory accesses.
    let pager_handle = Arc::new(AtomicU32::new(raw(&pager)));

    let thread = std::thread::spawn({
        let (running, pager_handle) = (running.clone(), pager_handle.clone());
        move || {
            let port = zx::Port::create();
            while running.load(Ordering::Relaxed) {
                let mut vmo = 0;
                // Load the most recent pager handle value and attempt to create a vmo. This handle
                // might have already been closed, so this call could fail, so just ignore any
                // errors.
                // SAFETY: `vmo` is valid for writes.
                let result = unsafe {
                    zx::sys::zx_pager_create_vmo(
                        pager_handle.load(Ordering::Relaxed),
                        0,
                        raw(&port),
                        0,
                        page_size(),
                        &mut vmo,
                    )
                };
                if result == zx::sys::ZX_OK {
                    // If the call succeeded make sure to close the vmo to not leak the handle.
                    // SAFETY: The handle was just created.
                    unsafe { zx::sys::zx_handle_close(vmo) };
                }
            }
        }
    });

    // Create and close pager handles in a loop. This is intended to trigger any race conditions
    // that might exist between on_zero_handles getting called, and an in-progress pager_create_vmo
    // call.
    let mut limiter = TestLimiter::new(10000, zx::MonotonicDuration::from_seconds(60));
    while !limiter.finished() {
        drop(pager);
        pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        pager_handle.store(raw(&pager), Ordering::Relaxed);
        limiter.next();
    }

    running.store(false, Ordering::Relaxed);
    thread.join().unwrap();
}

// Tests that OP_COMMIT works as expected via zx_vmar_op_range().
#[test]
fn op_commit_vmar() {
    // Create a temporary VMAR to work with.
    let vmar_size = 15 * page_size_usize();
    let temp = TempVmar::allocate(vmar_size);
    let (vmar, base_addr) = (temp.vmar.clone(), temp.base);

    let mut pager = UserPager::new();
    assert!(pager.init());

    // Create two pager VMOs.
    const NUM_PAGES: u64 = 3;
    let vmo1 = pager.create_vmo(NUM_PAGES).unwrap();
    let vmo2 = pager.create_vmo(NUM_PAGES).unwrap();

    // Map the two VMOs with no gap in between.
    let vmo_size = (NUM_PAGES * page_size()) as usize;
    let flags = zx::VmarFlags::SPECIFIC | zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
    assert_eq!(vmar.map(0, vmo1.vmo(), 0, vmo_size, flags).unwrap(), base_addr);
    assert_eq!(vmar.map(vmo_size, vmo2.vmo(), 0, vmo_size, flags).unwrap(), base_addr + vmo_size);

    // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted
    // pages.
    assert!(pager.supply_pages(&vmo1, 1, 1));
    assert!(pager.supply_pages(&vmo2, 1, 1));

    // Also map in a non pager-backed VMO to work with.
    let anon_vmo = zx::Vmo::create(vmo_size as u64).unwrap();
    assert_eq!(
        vmar.map(2 * vmo_size, &anon_vmo, 0, vmo_size, flags).unwrap(),
        base_addr + 2 * vmo_size
    );

    let v = vmar.clone();
    let mut t1 =
        TestThread::new(move || v.op_range(zx::VmarOp::COMMIT, base_addr, 3 * vmo_size).is_ok());
    assert!(t1.start());

    // We should see page requests for both VMOs.
    for vmo in [&vmo1, &vmo2] {
        assert!(pager.wait_for_page_read(vmo, 0, 1, INFINITE));
        assert!(pager.supply_pages(vmo, 0, 1));
        assert!(pager.wait_for_page_read(vmo, 2, 1, INFINITE));
        assert!(pager.supply_pages(vmo, 2, 1));
    }
    assert!(t1.wait());

    // Can't commit with gaps in the VMAR.
    assert_eq!(vmar.op_range(zx::VmarOp::COMMIT, base_addr, vmar_size), Err(zx::Status::BAD_STATE));
}

// Regression test for https://fxbug.dev/42173905. Tests that a port dequeue racing with pager
// destruction on a detached VMO does not result in use-after-frees.
#[test]
fn racy_port_dequeue() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use zx::sys::*;
    // Repeat multiple times so we can hit the race. 1000 is a good balance between trying to
    // reproduce the race without drastically increasing the test runtime.
    let mut limiter = TestLimiter::new(1000, zx::MonotonicDuration::from_seconds(60));
    while !limiter.finished() {
        let (mut pager, mut port, mut vmo) = (0, 0, 0);
        // SAFETY: The out parameters are valid for writes.
        unsafe {
            assert_eq!(zx_pager_create(0, &mut pager), ZX_OK);
            assert_eq!(zx_port_create(0, &mut port), ZX_OK);
            assert_eq!(zx_pager_create_vmo(pager, 0, port, 0, page_size(), &mut vmo), ZX_OK);
        }

        let ready = Arc::new(AtomicBool::new(false));
        let r = ready.clone();
        let mut t1 = TestThread::new(move || {
            while !r.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
            // Destroy the pager.
            // SAFETY: The pager handle is owned by this test.
            unsafe { zx_handle_close(pager) == ZX_OK }
        });
        let r = ready.clone();
        let mut t2 = TestThread::new(move || {
            while !r.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
            // Dequeue the complete packet from the port.
            let mut packet = std::mem::MaybeUninit::<zx_port_packet_t>::uninit();
            // SAFETY: The packet is valid for writes.
            let status = unsafe { zx_port_wait(port, 0, packet.as_mut_ptr()) };
            // We can time out if the queued packet was successfully cancelled and taken back from
            // the port during pager destruction.
            status == ZX_OK || status == ZX_ERR_TIMED_OUT
        });

        // Destroy the vmo so that the complete packet is queued, and the page source is closed.
        // SAFETY: The VMO handle is owned by this test.
        assert_eq!(unsafe { zx_handle_close(vmo) }, ZX_OK);

        // Start both the threads.
        assert!(t1.start());
        assert!(t2.start());

        // Try to race the pager destruction with the port dequeue.
        ready.store(true, Ordering::Relaxed);

        // Wait for both threads to exit.
        assert!(t1.wait());
        assert!(t2.wait());

        // Destroy the port.
        // SAFETY: The port handle is owned by this test.
        assert_eq!(unsafe { zx_handle_close(port) }, ZX_OK);
        limiter.next();
    }
}

#[test]
fn present_page_splits_range() {
    let mut pager = UserPager::new();
    assert!(pager.init());
    const NUM_PAGES: u64 = 5;
    const SUPPLIED_PAGE_INDEX: u64 = 2;
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    // Supply page in the middle of the VMO.
    assert!(pager.supply_pages(&vmo, SUPPLIED_PAGE_INDEX, 1));

    let v = vmo.clone();
    let mut t = TestThread::new(move || v.commit(0, NUM_PAGES));
    assert!(t.start());

    // Expect the first request to be the lower half of the VMO.
    assert!(t.wait_for_blocked());
    assert!(pager.wait_for_page_read(&vmo, 0, SUPPLIED_PAGE_INDEX, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, SUPPLIED_PAGE_INDEX));

    // Expect the second request to be the upper half of the VMO.
    assert!(t.wait_for_blocked());
    let upper = NUM_PAGES - SUPPLIED_PAGE_INDEX - 1;
    assert!(pager.wait_for_page_read(&vmo, SUPPLIED_PAGE_INDEX + 1, upper, INFINITE));
    assert!(pager.supply_pages(&vmo, SUPPLIED_PAGE_INDEX + 1, upper));

    assert!(t.wait());
}

/// A buffer a test thread reads into while the test watches it fill.
struct SharedBuffer {
    data: Box<[std::cell::UnsafeCell<u8>]>,
}

// SAFETY: The buffer is only written by the kernel on behalf of one thread, and read volatile.
unsafe impl Sync for SharedBuffer {}
unsafe impl Send for SharedBuffer {}

impl SharedBuffer {
    fn new(len: usize) -> Arc<Self> {
        Arc::new(Self { data: (0..len).map(|_| std::cell::UnsafeCell::new(0)).collect() })
    }

    fn ptr(&self, offset: usize) -> *mut u8 {
        self.data[offset].get()
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    /// The buffer, once the thread writing it has been joined.
    fn contents(&self) -> Vec<u8> {
        // SAFETY: Nothing writes the buffer any more.
        (0..self.len()).map(|i| unsafe { *self.ptr(i) }).collect()
    }
}

/// Reads `len` bytes at `offset` of `vmo` into `buffer` at `buffer_offset`.
fn read_into(
    vmo: &Vmo,
    buffer: &SharedBuffer,
    buffer_offset: usize,
    offset: u64,
    len: usize,
) -> bool {
    // SAFETY: The range is within the buffer, which only this read writes.
    let data = unsafe { std::slice::from_raw_parts_mut(buffer.ptr(buffer_offset), len) };
    vmo.vmo().read(data, offset).is_ok()
}

// After supply the pages the target thread remains logically blocked on a page request until it
// actually gets scheduled and makes progress out of the pager code. As such we cannot wait for
// it to block again, since we do not know whether it is in the previous or next block event.
// Therefore we just spin waiting to see the data.
fn wait_for_data(expected: &[u8], data: *const u8) {
    // data is volatile since we are intentionally not synchronizing the kernel, who could be
    // performing modifications to it as we are reading it.
    let cmp = || {
        // SAFETY: The data has `expected.len()` bytes.
        (0..expected.len()).all(|i| expected[i] == unsafe { data.add(i).read_volatile() })
    };
    // Just keep spinning until we either see the data or the test times out.
    while !cmp() {
        zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(10)).sleep();
    }
}

#[test]
fn early_wake_on_supply() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 128;
    const PAGE_STEPS: u64 = 32;
    // Start the read at an offset to ensure there is a difference between the page requests VMO
    // offset, and the offset from the start of the request.
    const START_OFFSET: u64 = 2;
    const VMO_PAGES: u64 = NUM_PAGES + START_OFFSET;
    let ps = page_size_usize();
    let vmo = pager.create_vmo(VMO_PAGES).unwrap();

    let mut expected = vec![0u8; NUM_PAGES as usize * ps];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, START_OFFSET);
    let data = SharedBuffer::new(NUM_PAGES as usize * ps);

    let (v, d) = (vmo.clone(), data.clone());
    let mut t = TestThread::new(move || read_into(&v, &d, 0, START_OFFSET * page_size(), d.len()));
    assert!(t.start());

    // Expect a request for all the pages.
    assert!(pager.wait_for_page_read(&vmo, START_OFFSET, NUM_PAGES, INFINITE));

    // Gradually supply all the pages, validating that data gets populated.
    for i in (0..NUM_PAGES).step_by(PAGE_STEPS as usize) {
        // Thread should be blocked waiting.
        assert!(t.wait_for_blocked());
        assert!(pager.supply_pages(&vmo, i + START_OFFSET, PAGE_STEPS));
        let start = i as usize * ps;
        wait_for_data(&expected[start..start + PAGE_STEPS as usize * ps], data.ptr(start));
    }
    assert!(t.wait());
    assert_eq!(expected, data.contents());
}

#[test]
fn early_wake_non_linear_supply() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 128;
    let ps = page_size_usize();
    let vmo = pager.create_vmo(NUM_PAGES).unwrap();

    let mut expected = vec![0u8; NUM_PAGES as usize * ps];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);
    let data = SharedBuffer::new(NUM_PAGES as usize * ps);

    let (v, d) = (vmo.clone(), data.clone());
    let mut t = TestThread::new(move || read_into(&v, &d, 0, 0, d.len()));
    assert!(t.start());

    // Expect a request for all the pages.
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

    // To account for pages potentially getting evicted start a page fault handler to re-supply
    // anything.
    vmo.set_page_fault_supply_limit(0);
    assert!(pager.start_tagged_page_fault_handler());

    // Supply some blocks of pages not at the start of the request.
    assert!(pager.supply_pages(&vmo, 16, 16));
    assert!(pager.supply_pages(&vmo, 48, 16));

    // Now supply the first pages.
    vmo.set_page_fault_supply_limit(32);
    assert!(pager.supply_pages(&vmo, 0, 16));

    // Expect everything to get copied all the way till the gap.
    wait_for_data(&expected[..32 * ps], data.ptr(0));

    // Supply the gap.
    vmo.set_page_fault_supply_limit(64);
    assert!(pager.supply_pages(&vmo, 32, 16));

    // Expect everything till the next gap to be supplied.
    wait_for_data(&expected[..64 * ps], data.ptr(0));

    // Finally supply the rest of the data.
    vmo.set_page_fault_supply_limit(NUM_PAGES);
    assert!(pager.supply_pages(&vmo, 64, NUM_PAGES - 64));
    assert!(t.wait());
    // No more page requests.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());
    assert_eq!(expected, data.contents());
}

#[test]
fn early_wake_overlapping_requests() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const VMO_SIZE: u64 = 128;
    let ps = page_size_usize();
    let vmo = pager.create_vmo(VMO_SIZE).unwrap();

    // Setup a collection of requests that overlap in different ways. The ranges listed here cannot
    // actually be changed arbitrarily, as the test code below hard codes various interesting
    // values and so while the test would not 'break', it would lose a lot of the desired coverage.
    struct Request {
        start: u64,
        end: u64,
        data: Arc<SharedBuffer>,
        thread: Option<TestThread>,
    }
    let mut requests: Vec<Request> = [(0, 92), (16, 42), (0, 32), (48, 64), (82, 128)]
        .into_iter()
        .map(|(start, end)| {
            let data = SharedBuffer::new((end - start) as usize * ps);
            let (v, d) = (vmo.clone(), data.clone());
            let thread =
                TestThread::new(move || read_into(&v, &d, 0, start * page_size(), d.len()));
            Request { start, end, data, thread: Some(thread) }
        })
        .collect();
    let mut expected = vec![0u8; VMO_SIZE as usize * ps];
    vmo.generate_buffer_contents(&mut expected, VMO_SIZE, 0);

    // Start the first request and wait for it to generate the page request.
    assert!(requests[0].thread.as_mut().unwrap().start());
    assert!(pager.wait_for_page_read(&vmo, 0, requests[0].end - requests[0].start, INFINITE));

    // Start all the other requests, should not see any additional page requests.
    for req in &mut requests[1..] {
        assert!(req.thread.as_mut().unwrap().start());
    }
    // No more requests.
    assert!(pager.get_page_read_request(&vmo, INFINITE_PAST).is_none());

    let validate_requests = |requests: &mut Vec<Request>, max_page: u64, cutoff: u64| {
        for req in requests.iter_mut() {
            if max_page < req.start || req.start >= cutoff {
                continue;
            }
            let start = req.start as usize * ps;
            let end = max_page.min(req.end) as usize * ps;
            wait_for_data(&expected[start..end], req.data.ptr(0));
            // If the whole range has been supplied, also check that the thread has completed,
            // unless we already checked.
            if max_page >= req.end
                && let Some(mut thread) = req.thread.take()
            {
                assert!(thread.wait());
            }
        }
    };

    // To account for pages potentially getting evicted, and the fact that the validate_requests
    // will be re-scanning the entire VMO start a page fault handler to re-supply anything.
    vmo.set_page_fault_supply_limit(0);
    assert!(pager.start_tagged_page_fault_handler());

    // Supply some early pages.
    vmo.set_page_fault_supply_limit(8);
    assert!(pager.supply_pages(&vmo, 0, 8));
    validate_requests(&mut requests, 8, u64::MAX);

    // Pre-supply a later page at the start of an overlap.
    assert!(pager.supply_pages(&vmo, 16, 1));

    // Supply the missing pages.
    vmo.set_page_fault_supply_limit(17);
    assert!(pager.supply_pages(&vmo, 8, 8));
    // Everything up till page 16 should be visible, but some of the overlap requests may not have
    // woken up due to the pre-supply.
    validate_requests(&mut requests, 17, 16);

    // Supply the next page in the line, finally waking up the overlap.
    vmo.set_page_fault_supply_limit(18);
    assert!(pager.supply_pages(&vmo, 17, 1));
    validate_requests(&mut requests, 18, u64::MAX);

    // Pre-supply a later page in this range before the end of an overlapping request.
    assert!(pager.supply_pages(&vmo, 30, 2));

    // Supply the gap.
    vmo.set_page_fault_supply_limit(32);
    assert!(pager.supply_pages(&vmo, 18, 30 - 18));
    validate_requests(&mut requests, 32, u64::MAX);

    // Supply into the range of the start of the next overlap.
    vmo.set_page_fault_supply_limit(50);
    assert!(pager.supply_pages(&vmo, 32, 50 - 32));
    validate_requests(&mut requests, 50, u64::MAX);

    // Supply all the way beyond the first request, completing the original sent page request.
    vmo.set_page_fault_supply_limit(100);
    assert!(pager.supply_pages(&vmo, 50, 100 - 50));
    validate_requests(&mut requests, 100, u64::MAX);

    // Should have another page request for the remaining portion. Unfortunately this request
    // gets received by the page fault handler we started previously. What we can do instead is
    // expect that any thread that did not complete, should end up waiting for a page request.
    for req in &requests {
        if let Some(thread) = &req.thread {
            assert!(thread.wait_for_blocked());
        }
    }

    // Supply the remaining pages.
    vmo.set_page_fault_supply_limit(VMO_SIZE);
    assert!(pager.supply_pages(&vmo, 100, VMO_SIZE - 100));

    validate_requests(&mut requests, VMO_SIZE, u64::MAX);
}

#[test]
fn early_wake_with_resize() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    const NUM_PAGES: u64 = 16;
    let ps = page_size_usize();
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::RESIZABLE).unwrap();
    let data = SharedBuffer::new(NUM_PAGES as usize * ps);

    let (v, d) = (vmo.clone(), data.clone());
    let mut t = TestThread::new(move || read_into(&v, &d, 0, 0, d.len()));
    assert!(t.start());

    // Expect a request for all the pages.
    assert!(pager.wait_for_page_read(&vmo, 0, NUM_PAGES, INFINITE));

    // Supply one page of the request.
    assert!(pager.supply_pages(&vmo, 0, 1));

    // Wait for the data to appear and then for the thread to block again.
    let mut expected = vec![0u8; NUM_PAGES as usize * ps];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);
    wait_for_data(&expected[..ps], data.ptr(0));
    // Wait for blocked again.
    assert!(t.wait_for_blocked());

    // Resize the VMO smaller.
    assert!(vmo.resize(NUM_PAGES / 2));

    // Supply the missing pages.
    assert!(pager.supply_pages(&vmo, 1, NUM_PAGES / 2 - 1));

    // Should end with failure.
    assert!(t.wait_for_failure());
}

// Prefetching directly on the root. Prefetching through slices and clones is not ported.
#[test]
fn prefetch() {
    let mut pager = UserPager::new();
    assert!(pager.init());

    let root_vmo = pager.create_vmo(1).unwrap();
    let v = root_vmo.clone();
    let mut t = TestThread::new(move || v.prefetch(0, 1));
    assert!(t.start());

    assert!(pager.wait_for_page_read(&root_vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&root_vmo, 0, 1));
    assert!(t.wait());
    pager.release_vmo(&root_vmo);
}

#[test]
fn create_vmo_unbounded_invalid_stream_size() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let mut vmo = 0;
    // The initial stream size of an unbounded vmo must be within the vmo size, and attempting to
    // set an overly large one shouldn't trigger a kernel panic.
    // SAFETY: `vmo` is valid for writes.
    let status = unsafe {
        zx::sys::zx_pager_create_vmo(
            raw(&pager),
            zx::sys::ZX_VMO_UNBOUNDED,
            raw(&port),
            0x1234,
            0xFFFFFFFFFFFFFFFF,
            &mut vmo,
        )
    };
    assert_eq!(status, zx::sys::ZX_ERR_OUT_OF_RANGE);
}

// This is a regression test for https://fxbug.dev/498512247 that attempts to trigger a
// use-after-free in PagerProxy.
#[test]
fn proxy_use_after_free() {
    let mut pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 0, page_size()).unwrap();

    // Close the pager, which will free the PagerDispatcher.
    drop(pager);

    // Create a new pager and query the stats in an attempt to trigger a use-after-free.
    pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let mut stats = [0u8; 4];
    // SAFETY: The buffer has room for the stats.
    let status = unsafe {
        zx::sys::zx_pager_query_vmo_stats(
            raw(&pager),
            raw(&vmo),
            0,
            stats.as_mut_ptr(),
            stats.len(),
        )
    };
    assert_eq!(status, zx::sys::ZX_ERR_INVALID_ARGS);
}

// Test that ZX_VMO_OP_ALWAYS_NEED can be applied to a pager-backed VMO with the pager detached.
// Regression test for https://fxbug.dev/515718774.
#[test]
fn detach_and_always_need() {
    let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
    let port = zx::Port::create();
    let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 1, 2 * page_size()).unwrap();
    pager.detach_vmo(&vmo).unwrap();
    vmo.op_range(zx::VmoOp::ALWAYS_NEED, 0, 2 * page_size()).unwrap();
}
