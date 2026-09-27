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
