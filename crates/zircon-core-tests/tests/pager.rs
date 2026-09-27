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
//! - VMOs have no children: `DecommitOnDetachTest` and the `Clone` tests.

use std::sync::Arc;
use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

macro_rules! vmo_vmar_test {
    ($name:ident, |$check_vmar:ident| $body:block) => {
        mod $name {
            #[allow(unused_imports)]
            use super::*;

            fn run($check_vmar: bool) $body

            #[test]
            fn vmar() {
                run(true)
            }

            #[test]
            fn vmo() {
                run(false)
            }
        }
    };
}

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
