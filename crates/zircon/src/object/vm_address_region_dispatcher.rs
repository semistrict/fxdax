//! VMAR objects.
//!
//! Ported from `zircon/kernel/object/vm_address_region_dispatcher.cc` and
//! `zircon/kernel/object/include/object/vm_address_region_dispatcher.h`.

use super::dispatcher::{Dispatcher, DispatcherBase, solo_dispatcher};
use super::handle::KernelHandle;
use super::rights::ZX_DEFAULT_VMAR_RIGHTS;
use crate::ensure;
use crate::vm::arch_vm_aspace::*;
use crate::vm::vm_address_region::*;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::VmObjectPaged;
use std::sync::Arc;
use zx_status::Status;
use zx_types::*;

/// Extracts `from` from `flags`, returning `to` if it was set.
fn extract_flag<T: Default>(flags: &mut u32, from: u32, to: T) -> T {
    let flag_set = *flags & from != 0;
    *flags &= !from;
    if flag_set { to } else { T::default() }
}

/// Splits the syscall flags into VMAR flags, MMU flags and alignment.
fn split_syscall_flags(mut flags: u32) -> Result<(u32, ArchMmuFlags, u8), Status> {
    // Figure out arch_mmu_flags
    let mut mmu_flags = 0;
    mmu_flags |= extract_flag(&mut flags, ZX_VM_PERM_READ, ARCH_MMU_FLAG_PERM_READ);
    mmu_flags |= extract_flag(&mut flags, ZX_VM_PERM_WRITE, ARCH_MMU_FLAG_PERM_WRITE);
    mmu_flags |= extract_flag(&mut flags, ZX_VM_PERM_EXECUTE, ARCH_MMU_FLAG_PERM_EXECUTE);
    // This flag is no longer needed and should have already been acted upon.
    extract_flag(&mut flags, ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED, 0u8);

    // Figure out vmar flags
    let mut vmar = 0;
    vmar |= extract_flag(&mut flags, ZX_VM_COMPACT, VMAR_FLAG_COMPACT);
    vmar |= extract_flag(&mut flags, ZX_VM_SPECIFIC, VMAR_FLAG_SPECIFIC);
    vmar |= extract_flag(&mut flags, ZX_VM_SPECIFIC_OVERWRITE, VMAR_FLAG_SPECIFIC_OVERWRITE);
    vmar |= extract_flag(&mut flags, ZX_VM_CAN_MAP_SPECIFIC, VMAR_FLAG_CAN_MAP_SPECIFIC);
    vmar |= extract_flag(&mut flags, ZX_VM_CAN_MAP_READ, VMAR_FLAG_CAN_MAP_READ);
    vmar |= extract_flag(&mut flags, ZX_VM_CAN_MAP_WRITE, VMAR_FLAG_CAN_MAP_WRITE);
    vmar |= extract_flag(&mut flags, ZX_VM_CAN_MAP_EXECUTE, VMAR_FLAG_CAN_MAP_EXECUTE);
    vmar |= extract_flag(&mut flags, ZX_VM_REQUIRE_NON_RESIZABLE, VMAR_FLAG_REQUIRE_NON_RESIZABLE);
    vmar |= extract_flag(&mut flags, ZX_VM_ALLOW_FAULTS, VMAR_FLAG_ALLOW_FAULTS);
    vmar |= extract_flag(&mut flags, ZX_VM_OFFSET_IS_UPPER_LIMIT, VMAR_FLAG_OFFSET_IS_UPPER_LIMIT);
    vmar |= extract_flag(
        &mut flags,
        ZX_VM_FAULT_BEYOND_STREAM_SIZE,
        VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE,
    );

    ensure!(flags & ((1u32 << ZX_VM_ALIGN_BASE) - 1) == 0, Status::INVALID_ARGS);
    // Figure out alignment.
    let alignment = (flags >> ZX_VM_ALIGN_BASE) as u8;
    ensure!((alignment >= 10 || alignment == 0) && alignment <= 32, Status::INVALID_ARGS);
    Ok((vmar, mmu_flags, alignment))
}

pub struct VmAddressRegionDispatcher {
    base: DispatcherBase,
    vmar: Arc<VmAddressRegion>,
    base_arch_mmu_flags: ArchMmuFlags,
}

solo_dispatcher!(VmAddressRegionDispatcher, ZX_OBJ_TYPE_VMAR, ZX_DEFAULT_VMAR_RIGHTS);

impl Dispatcher for VmAddressRegionDispatcher {
    fn is_waitable(&self) -> bool {
        false
    }
}

impl VmAddressRegionDispatcher {
    pub fn create(
        vmar: Arc<VmAddressRegion>,
        base_arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // The initial rights should match the VMAR's creation permissions
        let mut vmar_rights = ZX_DEFAULT_VMAR_RIGHTS;
        let vmar_flags = vmar.flags();
        if vmar_flags & VMAR_FLAG_CAN_MAP_READ != 0 {
            vmar_rights |= ZX_RIGHT_READ;
        }
        if vmar_flags & VMAR_FLAG_CAN_MAP_WRITE != 0 {
            vmar_rights |= ZX_RIGHT_WRITE;
        }
        if vmar_flags & VMAR_FLAG_CAN_MAP_EXECUTE != 0 {
            vmar_rights |= ZX_RIGHT_EXECUTE;
        }
        let dispatcher = Arc::new(Self { base: DispatcherBase::new(0), vmar, base_arch_mmu_flags });
        Ok((KernelHandle::new(dispatcher), vmar_rights))
    }

    pub fn vmar(&self) -> &Arc<VmAddressRegion> {
        &self.vmar
    }

    pub fn op_children_from_rights(rights: zx_rights_t) -> VmAddressRegionOpChildren {
        if rights & ZX_RIGHT_OP_CHILDREN == 0 {
            VmAddressRegionOpChildren::No
        } else {
            VmAddressRegionOpChildren::Yes
        }
    }

    /// Returns whether the protection flags are valid: write or execute need read.
    pub fn is_valid_mapping_protection(flags: u32) -> bool {
        if flags & ZX_VM_PERM_READ == 0 && flags & (ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE) != 0 {
            return false;
        }
        true
    }

    /// Creates a child VMAR.
    pub fn allocate(
        &self,
        offset: usize,
        size: usize,
        flags: u32,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let (vmar_flags, arch_mmu_flags, alignment) = split_syscall_flags(flags)?;
        // Check if any MMU-related flags were requested.
        ensure!(arch_mmu_flags == 0, Status::INVALID_ARGS);
        let new_vmar =
            self.vmar.create_sub_vmar(offset, size, alignment, vmar_flags, "useralloc")?;
        Self::create(new_vmar, self.base_arch_mmu_flags)
    }

    pub fn destroy(&self) -> Result<(), Status> {
        ensure!(
            !self.vmar.aspace().root_vmar().is_some_and(|root| Arc::ptr_eq(&root, &self.vmar)),
            Status::NOT_SUPPORTED
        );
        self.vmar.destroy()
    }

    /// Maps `vmo`, returning the mapping and its base.
    pub fn map(
        &self,
        vmar_offset: usize,
        vmo: Arc<VmObjectPaged>,
        vmo_offset: u64,
        len: usize,
        flags: u32,
    ) -> Result<(Arc<VmMapping>, usize), Status> {
        ensure!(Self::is_valid_mapping_protection(flags), Status::INVALID_ARGS);
        let (mut vmar_flags, arch_mmu_flags, alignment) = split_syscall_flags(flags)?;
        let arch_mmu_flags = arch_mmu_flags | self.base_arch_mmu_flags;
        if vmar_flags & VMAR_FLAG_REQUIRE_NON_RESIZABLE != 0 {
            vmar_flags &= !VMAR_FLAG_REQUIRE_NON_RESIZABLE;
            ensure!(!vmo.is_resizable(), Status::NOT_SUPPORTED);
        }
        if vmar_flags & VMAR_FLAG_ALLOW_FAULTS != 0 {
            vmar_flags &= !VMAR_FLAG_ALLOW_FAULTS;
        } else if vmo.is_discardable() {
            // TODO(https://fxbug.dev/42109795): Add additional checks once all clients (resizable
            // and pager-backed VMOs) start using the VMAR_FLAG_ALLOW_FAULTS flag.
            return Err(Status::NOT_SUPPORTED);
        }
        self.vmar.create_vm_mapping(
            vmar_offset,
            len,
            alignment,
            vmar_flags,
            vmo,
            vmo_offset,
            arch_mmu_flags,
            "useralloc",
        )
    }

    pub fn protect(
        &self,
        base: usize,
        len: usize,
        flags: u32,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        ensure!(page::is_aligned(base), Status::INVALID_ARGS);
        ensure!(Self::is_valid_mapping_protection(flags), Status::INVALID_ARGS);
        let (vmar_flags, arch_mmu_flags, alignment) = split_syscall_flags(flags)?;
        // This request does not allow any VMAR flags or alignment flags to be set.
        ensure!(vmar_flags == 0 && alignment == 0, Status::INVALID_ARGS);
        self.vmar.protect(base, len, arch_mmu_flags | self.base_arch_mmu_flags, op_children)
    }

    pub fn range_op(
        &self,
        op: u32,
        base: usize,
        len: usize,
        rights: zx_rights_t,
    ) -> Result<(), Status> {
        let which_op = match op {
            ZX_VMAR_OP_COMMIT => RangeOpType::Commit,
            ZX_VMAR_OP_DECOMMIT => RangeOpType::Decommit,
            ZX_VMAR_OP_MAP_RANGE => RangeOpType::MapRange,
            ZX_VMAR_OP_ZERO => RangeOpType::Zero,
            ZX_VMAR_OP_DONT_NEED => RangeOpType::DontNeed,
            ZX_VMAR_OP_ALWAYS_NEED => RangeOpType::AlwaysNeed,
            ZX_VMAR_OP_PREFETCH => RangeOpType::Prefetch,
            _ => return Err(Status::INVALID_ARGS),
        };
        let allowed = match which_op {
            RangeOpType::Commit | RangeOpType::Decommit | RangeOpType::Zero => {
                rights & ZX_RIGHT_WRITE != 0
            }
            RangeOpType::Prefetch | RangeOpType::MapRange => rights & ZX_RIGHT_READ != 0,
            // just hints
            RangeOpType::DontNeed | RangeOpType::AlwaysNeed => true,
        };
        ensure!(allowed, Status::ACCESS_DENIED);
        self.vmar.range_op(which_op, base, len, Self::op_children_from_rights(rights))
    }

    pub fn unmap(
        &self,
        base: usize,
        len: usize,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        ensure!(page::is_aligned(base), Status::INVALID_ARGS);
        self.vmar.unmap(base, len, op_children)
    }

    pub fn get_vmar_info(&self) -> zx_info_vmar_t {
        zx_info_vmar_t { base: self.vmar.base(), len: self.vmar.size() }
    }
}
