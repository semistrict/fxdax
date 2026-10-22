// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Mappings under Shuttle (awslabs/shuttle, the `shuttle` feature): mapping a mapping's pages
//! (`VmMapping::map_range`) on a thread of its own, racing a change to the mapping on another, in
//! the interleavings Shuttle's schedulers choose.

use crate::hypervisor::aspace::GuestPhysicalAspace;
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ArchMmuFlags};
use crate::vm::pmm;
use crate::vm::vm_address_region::{VMAR_FLAG_SPECIFIC, VmAddressRegionOpChildren};
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::VmObjectPaged;
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Config, Runner, thread};
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

/// The pages a test maps.
const PAGES: usize = 4;

/// The schedules a test runs with each scheduler.
const RUNS: usize = 100;

/// Runs `test` in schedules of each scheduler's, with threads' stacks as large as the kernel's
/// calls need.
fn check(test: fn()) {
    let config = || {
        let mut config = Config::new();
        config.stack_size = 4 << 20;
        config
    };
    Runner::new(RandomScheduler::new(RUNS), config()).run(test);
    Runner::new(PctScheduler::new(3, RUNS), config()).run(test);
}

/// A guest physical address space, and a mapping in it of `PAGES` committed pages of an anonymous
/// VMO, readable and writable, none of whose pages is mapped yet.
fn mapping() -> (GuestPhysicalAspace, Arc<VmMapping>) {
    let gpas = GuestPhysicalAspace::create(2 * PAGES * PAGE_SIZE).unwrap();
    let len = PAGES * PAGE_SIZE;
    let vmo = VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, len as u64).unwrap();
    vmo.commit_range(0, len as u64).unwrap();
    let flags: ArchMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
    let root = gpas.root_vmar();
    let (mapping, _) =
        root.create_vm_mapping(0, len, 0, VMAR_FLAG_SPECIFIC, vmo, 0, flags, "vmo").unwrap();
    (gpas, mapping)
}

/// Maps the pages of `mapping` on a thread of its own while `change` runs on this one.
fn map_pages_racing(mapping: &Arc<VmMapping>, change: impl FnOnce()) {
    let mapping = mapping.clone();
    let mapper = thread::spawn(move || {
        // It may find the mapping gone, and fail: what it leaves mapped is what the test checks.
        let _ = mapping.map_range(0, PAGES * PAGE_SIZE, false, true);
    });
    change();
    mapper.join().unwrap();
}

/// Mapping a mapping's pages racing its unmapping leaves none of them mapped. A page mapped after
/// the unmapping would stay mapped once the VMO frees it, as the mapping no longer hears of its
/// VMO's changes.
fn mapping_pages_races_unmapping() {
    let (gpas, mapping) = mapping();
    map_pages_racing(&mapping, || gpas.unmap_range(0, PAGES * PAGE_SIZE).unwrap());
    for page in 0..PAGES {
        let mapped = gpas.arch_aspace().query(page * PAGE_SIZE);
        assert_eq!(mapped, Err(Status::NOT_FOUND), "page {page} left mapped");
    }
}

#[test]
fn mapping_pages_races_unmapping_in_any_order() {
    check(mapping_pages_races_unmapping);
}

/// Mapping a mapping's pages racing a protect that takes write access away maps each of them
/// read-only: mapped before, the protect changes it; after, it is mapped as the protect left the
/// mapping.
fn mapping_pages_races_protecting() {
    let (gpas, mapping) = mapping();
    let root = gpas.root_vmar();
    let read_only = || {
        let children = VmAddressRegionOpChildren::No;
        root.protect(0, PAGES * PAGE_SIZE, ARCH_MMU_FLAG_PERM_READ, children).unwrap();
    };
    map_pages_racing(&mapping, read_only);
    for page in 0..PAGES {
        let flags = gpas.arch_aspace().query(page * PAGE_SIZE).map(|(_, flags)| flags);
        assert_eq!(flags, Ok(ARCH_MMU_FLAG_PERM_READ), "page {page}");
    }
}

#[test]
fn mapping_pages_races_protecting_in_any_order() {
    check(mapping_pages_races_protecting);
}
