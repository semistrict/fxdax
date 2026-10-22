// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of what maps a mapping's pages once the mapping is destroyed: nothing does, as nothing
//! would unmap them as its VMO changes.

use crate::hypervisor::aspace::GuestPhysicalAspace;
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ArchMmuFlags};
use crate::vm::fault::flag;
use crate::vm::page_source::MultiPageRequest;
use crate::vm::pmm;
use crate::vm::vm_address_region::VMAR_FLAG_SPECIFIC;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::VmObjectPaged;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

/// The pages a test maps.
const PAGES: usize = 4;

/// A guest physical address space, and a mapping in it of `PAGES` committed pages of an anonymous
/// VMO, readable and writable, none of whose pages is mapped yet, and the VMO.
fn mapping() -> (GuestPhysicalAspace, Arc<VmMapping>, Arc<VmObjectPaged>) {
    let gpas = GuestPhysicalAspace::create(2 * PAGES * PAGE_SIZE).unwrap();
    let len = PAGES * PAGE_SIZE;
    let vmo = VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, len as u64).unwrap();
    vmo.commit_range(0, len as u64).unwrap();
    let flags: ArchMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
    let root = gpas.root_vmar();
    let (mapping, _) = root
        .create_vm_mapping(0, len, 0, VMAR_FLAG_SPECIFIC, vmo.clone(), 0, flags, "vmo")
        .unwrap();
    (gpas, mapping, vmo)
}

/// Asserts that none of the mapping's pages is mapped.
fn assert_unmapped(gpas: &GuestPhysicalAspace) {
    for page in 0..PAGES {
        let mapped = gpas.arch_aspace().query(page * PAGE_SIZE);
        assert_eq!(mapped, Err(Status::NOT_FOUND), "page {page} mapped");
    }
}

/// A fault that found a mapping that has since been destroyed maps nothing, and is to be retried,
/// to find what is mapped there now.
#[test]
fn faults_on_destroyed_mappings_are_retried() {
    let (gpas, mapping, vmo) = mapping();
    gpas.unmap_range(0, PAGES * PAGE_SIZE).unwrap();
    let mut page_request = MultiPageRequest::default();
    let faulted = mapping.page_fault(0, flag::GUEST, 0, &vmo, &mut page_request);
    assert_eq!(faulted, (Err(Status::UNAVAILABLE), 0));
    assert_unmapped(&gpas);
}

/// Mapping the pages of a destroyed mapping fails, and maps nothing.
#[test]
fn destroyed_mappings_map_nothing() {
    let (gpas, mapping, _vmo) = mapping();
    gpas.unmap_range(0, PAGES * PAGE_SIZE).unwrap();
    assert_eq!(mapping.map_range(0, PAGES * PAGE_SIZE, false, true), Err(Status::BAD_STATE));
    assert_unmapped(&gpas);
}
