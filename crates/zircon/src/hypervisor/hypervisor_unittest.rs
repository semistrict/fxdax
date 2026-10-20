// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/hypervisor/hypervisor_unittest.cc`.
//!
//! The tests run on every host, as the guest physical address space is the host's own memory.
//! Not ported: the interrupt controller, cache policy, interrupt bitmap and trap map tests, as
//! those are not ported.

use super::aspace::GuestPhysicalAspace;
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_EXECUTE, ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ArchMmuFlags,
};
use crate::vm::pmm;
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_address_region::{
    VMAR_FLAG_SPECIFIC, VmAddressRegion, VmAddressRegionOpChildren, VmAddressRegionOrMapping,
};
use crate::vm::vm_object_paged::VmObjectPaged;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

const MMU_FLAGS: ArchMmuFlags =
    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE;

/// The size of the guest physical address spaces the tests create.
const GUEST_PHYSICAL_ASPACE_SIZE: usize = 1 << 30;

fn create_gpas() -> Result<GuestPhysicalAspace, Status> {
    GuestPhysicalAspace::create(GUEST_PHYSICAL_ASPACE_SIZE)
}

fn create_vmo(vmo_size: usize) -> Result<Arc<VmObjectPaged>, Status> {
    VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, vmo_size as u64)
}

fn commit_vmo(vmo: &VmObjectPaged) -> Result<(), Status> {
    vmo.commit_range(0, vmo.size())
}

fn create_mapping(
    vmar: &VmAddressRegion,
    vmo: &Arc<VmObjectPaged>,
    addr: usize,
    mmu_flags: ArchMmuFlags,
) -> Result<(), Status> {
    vmar.create_vm_mapping(
        addr,
        vmo.size() as usize,
        0,
        VMAR_FLAG_SPECIFIC,
        vmo.clone(),
        0,
        mmu_flags,
        "vmo",
    )
    .map(|_| ())
}

fn create_sub_vmar(
    vmar: &VmAddressRegion,
    offset: usize,
    size: usize,
) -> Result<Arc<VmAddressRegion>, Status> {
    vmar.create_sub_vmar(offset, size, 0, vmar.flags() | VMAR_FLAG_SPECIFIC, "vmar")
}

#[test]
fn guest_physical_aspace_unmap_range() {
    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");
    let vmo = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    create_mapping(&gpa.root_vmar(), &vmo, 0, MMU_FLAGS).expect("Failed to create mapping");

    // Unmap page.
    gpa.unmap_range(0, PAGE_SIZE).expect("Failed to unmap page from GuestPhysicalAspace");

    // Verify IsMapped for unmapped address fails.
    assert!(!gpa.is_mapped(0), "Expected address to be unmapped");
}

#[test]
fn guest_physical_aspace_unmap_range_outside_of_mapping() {
    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");
    let vmo = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    create_mapping(&gpa.root_vmar(), &vmo, 0, MMU_FLAGS).expect("Failed to create mapping");

    // Unmap page.
    gpa.unmap_range(PAGE_SIZE * 8, PAGE_SIZE)
        .expect("Failed to unmap page from GuestPhysicalAspace");
}

#[test]
fn guest_physical_aspace_unmap_range_multiple_mappings() {
    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");

    let vmo1 = create_vmo(PAGE_SIZE * 2).expect("Failed to create VMO");
    create_mapping(&gpa.root_vmar(), &vmo1, 0, MMU_FLAGS).expect("Failed to create mapping");

    let vmo2 = create_vmo(PAGE_SIZE * 2).expect("Failed to create VMO");
    create_mapping(&gpa.root_vmar(), &vmo2, PAGE_SIZE * 3, MMU_FLAGS)
        .expect("Failed to create mapping");

    // Unmap pages.
    gpa.unmap_range(PAGE_SIZE, PAGE_SIZE * 3)
        .expect("Failed to multiple unmap pages from GuestPhysicalAspace");

    // Verify IsMapped for unmapped addresses fails.
    for addr in (PAGE_SIZE..PAGE_SIZE * 4).step_by(PAGE_SIZE) {
        assert!(!gpa.is_mapped(addr), "Expected address to be unmapped");
    }

    // Verify IsMapped for mapped addresses succeeds.
    assert!(gpa.is_mapped(0), "Expected address to be mapped");
    assert!(gpa.is_mapped(PAGE_SIZE * 4), "Expected address to be mapped");
}

#[test]
fn guest_physical_aspace_unmap_range_sub_region() {
    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");
    let root_vmar = gpa.root_vmar();
    // To test partial unmapping within sub-VMAR:
    // Sub-VMAR from [0, PAGE_SIZE * 2).
    // Map within sub-VMAR from [PAGE_SIZE, PAGE_SIZE * 2).
    let sub_vmar1 =
        create_sub_vmar(&root_vmar, 0, PAGE_SIZE * 2).expect("Failed to create sub-VMAR");
    assert!(sub_vmar1.has_parent(), "Sub-VMAR does not have a parent");
    let vmo1 = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    create_mapping(&sub_vmar1, &vmo1, PAGE_SIZE, MMU_FLAGS).expect("Failed to create mapping");
    // To test destroying of sub-VMAR:
    // Sub-VMAR from [PAGE_SIZE * 2, PAGE_SIZE * 3).
    // Map within sub-VMAR from [0, PAGE_SIZE).
    let sub_vmar2 =
        create_sub_vmar(&root_vmar, PAGE_SIZE * 2, PAGE_SIZE).expect("Failed to create sub-VMAR");
    assert!(sub_vmar2.has_parent(), "Sub-VMAR does not have a parent");
    let vmo2 = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    create_mapping(&sub_vmar2, &vmo2, 0, MMU_FLAGS).expect("Failed to create mapping");
    // To test partial unmapping within root-VMAR:
    // Map within root-VMAR from [PAGE_SIZE * 3, PAGE_SIZE * 5).
    let vmo3 = create_vmo(PAGE_SIZE * 2).expect("Failed to create VMO");
    create_mapping(&root_vmar, &vmo3, PAGE_SIZE * 3, MMU_FLAGS).expect("Failed to create mapping");

    // Cannot unmap spanning across two different sub-vmars.
    assert_eq!(gpa.unmap_range(PAGE_SIZE, PAGE_SIZE * 3), Err(Status::INVALID_ARGS));

    // Verify IsMapped for mapped addresses succeeds.
    assert!(gpa.is_mapped(PAGE_SIZE * 4), "Expected address to be mapped");

    // Verify that sub-VMARs still have a parent.
    assert!(sub_vmar1.has_parent(), "Sub-VMAR does not have a parent");
    assert!(sub_vmar2.has_parent(), "Sub-VMAR does not have a parent");
}

#[test]
fn guest_phyiscal_address_space_single_vmo_multiple_mappings() {
    let _scanner_disable = AutoVmScannerDisable::new();

    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");

    let vmo = create_vmo(PAGE_SIZE * 4).expect("Failed to create VMO");

    // Map a single page of this four page VMO at offset 0x1000 and offset 0x3000.
    gpa.root_vmar()
        .create_vm_mapping(
            PAGE_SIZE,
            PAGE_SIZE,
            0,
            VMAR_FLAG_SPECIFIC,
            vmo.clone(),
            PAGE_SIZE as u64,
            MMU_FLAGS,
            "vmo",
        )
        .expect("Failed to create first mapping");
    gpa.root_vmar()
        .create_vm_mapping(
            PAGE_SIZE * 3,
            PAGE_SIZE,
            0,
            VMAR_FLAG_SPECIFIC,
            vmo.clone(),
            PAGE_SIZE as u64 * 3,
            MMU_FLAGS,
            "vmo",
        )
        .expect("Failed to create second mapping");

    commit_vmo(&vmo).expect("Failed to commit VMO");

    // No mapping at 0x0 or 0x2000.
    assert!(!gpa.is_mapped(0), "Expected address to be unmapped");
    assert!(!gpa.is_mapped(PAGE_SIZE * 2), "Expected address to be unmapped");

    // There is a mapping at 0x1000 and 0x3000.
    assert!(gpa.is_mapped(PAGE_SIZE), "Expected address to be mapped");
    assert!(gpa.is_mapped(PAGE_SIZE * 3), "Expected address to be mapped");
}

#[test]
fn guest_physical_aspace_page_fault() {
    // Setup.
    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");
    let vmo = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    let root_vmar = gpa.root_vmar();
    create_mapping(&root_vmar, &vmo, 0, MMU_FLAGS).expect("Failed to create mapping");
    create_mapping(&root_vmar, &vmo, PAGE_SIZE, ARCH_MMU_FLAG_PERM_READ)
        .expect("Failed to create mapping");
    create_mapping(
        &root_vmar,
        &vmo,
        PAGE_SIZE * 2,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
    )
    .expect("Failed to create mapping");
    create_mapping(
        &root_vmar,
        &vmo,
        PAGE_SIZE * 3,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE,
    )
    .expect("Failed to create mapping");

    // Fault in each page, writing where the mapping allows it as the fault upstream always does.
    for (page, write) in [(0usize, true), (1, false), (2, true), (3, false)] {
        gpa.page_fault(PAGE_SIZE * page, write).expect("Failed to fault page");
    }
}

#[test]
fn guest_physical_aspace_protect() {
    // Setup.
    let vmo = create_vmo(PAGE_SIZE).expect("Failed to create VMO");

    let gpa = create_gpas().expect("Failed to create GuestPhysicalAspace");
    create_mapping(&gpa.root_vmar(), &vmo, 0, MMU_FLAGS).expect("Failed to create mapping");

    gpa.root_vmar()
        .protect(0, PAGE_SIZE, ARCH_MMU_FLAG_PERM_WRITE, VmAddressRegionOpChildren::Yes)
        .expect("Failed to enable write access");
}

#[test]
fn guest_physical_aspace_query() {
    // Disable the VM scanner to ensure pages stay committed into the VMO, and hence stay mapped
    // into the aspace before we query.
    let _scanner_disable = AutoVmScannerDisable::new();

    // This test is arch independent so be conservative with the permission and assume that read
    // is needed for any other permission.
    const MMU_FLAG_TESTS: [ArchMmuFlags; 4] = [
        ARCH_MMU_FLAG_PERM_READ,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE,
    ];

    let vmo = create_vmo(PAGE_SIZE).unwrap();

    let gpa = create_gpas().unwrap();

    for flags in MMU_FLAG_TESTS {
        let (mapping, _) = gpa
            .root_vmar()
            .create_vm_mapping(0, PAGE_SIZE, 0, VMAR_FLAG_SPECIFIC, vmo.clone(), 0, flags, "vmo")
            .unwrap();
        mapping.map_range(0, PAGE_SIZE, true, false).unwrap();

        let (_, query_flags) = gpa.arch_aspace().query(0).unwrap();
        assert_eq!(flags, query_flags);

        // Cleanup the mapping for next iteration.
        VmAddressRegionOrMapping::Mapping(mapping).destroy().unwrap();
    }
}

/// fxdax (TASK-53): many mappings of one VMO, as a DAX window mapping a file in pieces has, are
/// removed in time linear in their number, in any order: last made first, here, and the rest as
/// the address space is destroyed. Each removal from the VMO's mappings finds the mapping at once,
/// where a scan of the others made it quadratic (70k took minutes).
#[test]
fn guest_physical_aspace_many_mappings_of_one_vmo() {
    const MAPPINGS: usize = 100_000;
    let _scanner_disable = AutoVmScannerDisable::new();
    let gpa = GuestPhysicalAspace::create(MAPPINGS * PAGE_SIZE).expect("Failed to create aspace");
    let vmo = create_vmo(PAGE_SIZE).expect("Failed to create VMO");
    let vmar = gpa.root_vmar();
    for mapping in 0..MAPPINGS {
        create_mapping(&vmar, &vmo, mapping * PAGE_SIZE, MMU_FLAGS)
            .expect("Failed to create mapping");
    }
    assert_eq!(vmo.num_mappings(), MAPPINGS as u32);
    for mapping in (MAPPINGS / 2..MAPPINGS).rev() {
        vmar.unmap(mapping * PAGE_SIZE, PAGE_SIZE, VmAddressRegionOpChildren::No)
            .expect("Failed to unmap");
    }
    assert_eq!(vmo.num_mappings(), (MAPPINGS / 2) as u32);
    drop(vmar);
    drop(gpa);
    assert_eq!(vmo.num_mappings(), 0);
}
