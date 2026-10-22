// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of pager-backed VMOs mapped into a guest's physical address space, as the windows a
//! extent-store device gives a guest for direct access (DAX) to extents are.
//!
//! The VMOs are mapped with the VMAR syscalls through a handle to the guest physical address
//! space's root VMAR, and the guest's accesses are its faults, which a VMM would report on exits.
//! Accesses by the host through the window take faults like the guest's.

use std::sync::Arc;
use zircon::hypervisor::aspace::GuestPhysicalAspace;
use zircon_core_tests::test_thread::TestThread;
use zircon_core_tests::userpager::*;

const INFINITE: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE;
const INFINITE_PAST: zx::MonotonicInstant = zx::MonotonicInstant::INFINITE_PAST;

fn range(offset: u64, length: u64) -> zx_vmo_dirty_range_t {
    zx_vmo_dirty_range_t { offset, length, options: 0 }
}

/// Creates a guest physical address space of `num_pages`, returning it and a handle to its root
/// VMAR.
fn create_gpas(num_pages: u64) -> (Arc<GuestPhysicalAspace>, zx::Vmar) {
    let gpas = GuestPhysicalAspace::create((num_pages * page_size()) as usize).unwrap();
    let (vmar, rights) = gpas.create_vmar_dispatcher().unwrap();
    let handle = vmar.make_and_add_handle(rights).unwrap();
    // SAFETY: The handle was just added to the handle table.
    let vmar = zx::Vmar::from(unsafe { zx::Handle::from_raw(handle) }.unwrap());
    (Arc::new(gpas), vmar)
}

fn assert_no_requests(pager: &UserPager, vmo: &Vmo) {
    assert!(pager.get_page_dirty_request(vmo, INFINITE_PAST).is_none());
    assert!(pager.get_page_read_request(vmo, INFINITE_PAST).is_none());
}

// Reads and writes a trap-dirty pager-backed VMO through a guest physical window, checking the
// pager sees the reads and dirtying, and that writeback write-protects the window again.
#[test]
fn guest_window_read_write_writeback() {
    let mut pager = UserPager::initialized();

    const NUM_PAGES: u64 = 4;
    let vmo = pager.create_vmo_with_options(NUM_PAGES, zx::VmoOptions::TRAP_DIRTY).unwrap();

    // Map the VMO a few pages into the window.
    const WINDOW_OFFSET: u64 = 4;
    let (gpas, vmar) = create_gpas(16);
    let gpa = (WINDOW_OFFSET * page_size()) as usize;
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::SPECIFIC;
    let mapped = vmar.map(gpa, vmo.vmo(), 0, (NUM_PAGES * page_size()) as usize, flags).unwrap();
    assert_eq!(mapped, vmar.info().unwrap().base + gpa);
    let window = gpas.host_base().expect("a window without a second stage") + gpa;

    let mut expected = vec![0u8; (NUM_PAGES * page_size()) as usize];
    vmo.generate_buffer_contents(&mut expected, NUM_PAGES, 0);

    // The guest reads the first page, which the pager is asked to supply.
    let g = gpas.clone();
    let mut t1 = TestThread::new(move || g.page_fault(gpa, false).is_ok());
    assert!(t1.start());
    assert!(pager.wait_for_page_read(&vmo, 0, 1, INFINITE));
    assert!(pager.supply_pages(&vmo, 0, NUM_PAGES));
    assert!(t1.wait());

    // The page is in the window, and reading it did not dirty it.
    // SAFETY: The first page of the window is mapped.
    let page = unsafe { std::slice::from_raw_parts(window as *const u8, page_size() as usize) };
    assert_eq!(page, &expected[..page_size() as usize]);
    assert!(pager.verify_dirty_ranges(&vmo, &[]));
    assert_no_requests(&pager, &vmo);

    // The guest writes the first page, which the pager is asked to dirty.
    let g = gpas.clone();
    let mut t2 = TestThread::new(move || g.page_fault(gpa, true).is_ok());
    assert!(t2.start());
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t2.wait());

    // The page is now writable in the window, and writes to it are writes to the VMO.
    // SAFETY: The first page of the window is mapped writable.
    unsafe { (window as *mut u8).write_volatile(0xaa) };
    expected[0] = 0xaa;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, false));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Writing the page back cleans it, which write-protects it in the window again.
    assert!(pager.writeback_begin_pages(&vmo, 0, 1));
    assert!(pager.writeback_end_pages(&vmo, 0, 1));
    assert!(pager.verify_dirty_ranges(&vmo, &[]));

    // So writing it through the window faults, and the pager is asked to dirty it again.
    let mut t3 = TestThread::new(move || {
        // SAFETY: The first page of the window is mapped.
        unsafe { (window as *mut u8).write_volatile(0xbb) };
        true
    });
    assert!(t3.start());
    assert!(pager.wait_for_page_dirty(&vmo, 0, 1, INFINITE));
    assert!(pager.dirty_pages(&vmo, 0, 1));
    assert!(t3.wait());
    expected[0] = 0xbb;
    assert!(check_buffer_data(&vmo, 0, 1, &expected, false));
    assert!(pager.verify_dirty_ranges(&vmo, &[range(0, 1)]));

    // Faults outside the VMO's mapping are not the kernel's to resolve.
    assert_eq!(gpas.page_fault(0, false), Err(zx::Status::NOT_FOUND));

    assert_no_requests(&pager, &vmo);
}

// Unmapping a VMO from the window unmaps its pages, so the guest faults on them again.
#[test]
fn guest_window_unmap() {
    let mut pager = UserPager::initialized();

    let vmo = pager.create_vmo(1).unwrap();
    assert!(pager.supply_pages(&vmo, 0, 1));

    let (gpas, vmar) = create_gpas(4);
    let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::SPECIFIC;
    vmar.map(0, vmo.vmo(), 0, page_size() as usize, flags).unwrap();
    gpas.page_fault(0, false).unwrap();
    assert!(gpas.is_mapped(0));

    gpas.unmap_range(0, page_size() as usize).unwrap();
    assert!(!gpas.is_mapped(0));
    assert_eq!(gpas.page_fault(0, false), Err(zx::Status::NOT_FOUND));
    // A write fault on a read-only mapping is denied.
    vmar.map(0, vmo.vmo(), 0, page_size() as usize, flags).unwrap();
    assert_eq!(gpas.page_fault(0, true), Err(zx::Status::ACCESS_DENIED));
}
