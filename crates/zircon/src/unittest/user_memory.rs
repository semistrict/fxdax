// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/lib/unittest/user_memory.rs`.
//!
//! Deviations:
//! - User memory is mapped in the process's address space (`runtime::aspace`), standing in for
//!   the current thread's active aspace.
//! - Only what the tests use: no pointer tags, protection changes, or disabling of the scanner, which upstream disables as
//!   the kernel touches the memory without resolving faults; here every access is a user copy.

use crate::runtime;
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_USER, ARCH_MMU_FLAG_PERM_WRITE,
};
use crate::vm::pmm;
use crate::vm::vm_address_region::{
    VMAR_FLAG_CAN_MAP_EXECUTE, VMAR_FLAG_CAN_MAP_READ, VMAR_FLAG_CAN_MAP_WRITE, VmAddressRegion,
    VmAddressRegionOrMapping,
};
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::VmObjectPaged;
use page::round_up;
use std::ops::Range;
use std::sync::Arc;
use zx_status::Status;

/// UserMemory facilitates testing code that requires user memory.
pub struct UserMemory {
    mapping: Arc<VmMapping>,
    vmo: Arc<VmObjectPaged>,
}

impl UserMemory {
    pub fn create(size: usize) -> Option<Self> {
        let size = round_up(size);
        match VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, size as u64) {
            Ok(vmo) => Self::create_from_vmo(vmo, 0),
            Err(status) => {
                eprintln!("VmObjectPaged::create failed: {}", status.into_raw());
                None
            }
        }
    }

    pub fn create_from_vmo(vmo: Arc<VmObjectPaged>, align_pow2: u8) -> Option<Self> {
        let root_vmar = runtime::aspace().root_vmar().expect("aspace has root vmar");
        Self::create_in_vmar(vmo, root_vmar, align_pow2)
    }

    pub fn create_in_vmar(
        vmo: Arc<VmObjectPaged>,
        vmar: Arc<VmAddressRegion>,
        align_pow2: u8,
    ) -> Option<Self> {
        let size = vmo.size() as usize;
        let vmar_flags =
            VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE;
        let arch_mmu_flags =
            ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

        let mapping_result = vmar.create_vm_mapping(
            /* mapping_offset= */ 0,
            size,
            align_pow2,
            vmar_flags,
            vmo.clone(),
            /* vmo_offset= */ 0,
            arch_mmu_flags,
            "unittest",
        );

        match mapping_result {
            Ok((mapping, _)) => Some(Self { mapping, vmo }),
            Err(status) => {
                eprintln!("create_vm_mapping failed: {}", status.into_raw());
                None
            }
        }
    }

    pub fn base(&self) -> usize {
        self.mapping.base()
    }

    pub fn user_out<T>(&self) -> UserOutPtr<T> {
        UserOutPtr::new(std::ptr::with_exposed_provenance_mut::<T>(self.base()))
    }

    pub fn user_in<T>(&self) -> UserInPtr<T> {
        UserInPtr::new(std::ptr::with_exposed_provenance::<T>(self.base()))
    }

    /// Ensures the mapping is committed and mapped such that usages will cause no faults.
    pub fn commit_and_map(&self, range: Range<usize>) -> Result<(), Status> {
        let offset = range.start;
        let len = range.end - range.start;
        self.mapping
            .map_range(offset, len, /* commit= */ true, /* ignore_existing= */ false)
    }

    /// Read the underlying VMO directly, bypassing the mapping.
    pub fn vmo_read(&self, data: &mut [u8], offset: u64) -> Result<(), Status> {
        self.vmo.read(data, offset)
    }

    /// Write to the underlying VMO directly, bypassing the mapping.
    pub fn vmo_write(&self, data: &[u8], offset: u64) -> Result<(), Status> {
        self.vmo.write(data, offset)
    }
}

impl Drop for UserMemory {
    fn drop(&mut self) {
        let status = VmAddressRegionOrMapping::Mapping(self.mapping.clone()).destroy();
        debug_assert_eq!(status, Ok(()));
    }
}
