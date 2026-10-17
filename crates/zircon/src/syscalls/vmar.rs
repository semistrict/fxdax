//! VMAR syscalls, ported from `zircon/kernel/lib/syscalls/vmar.cc`.
//!
//! Deviation: no clock or IOB mappings, and no fault-beyond-stream-size mappings.

use super::copy_out;
use crate::ensure;
use crate::object::handle::handle_table;
use crate::object::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::object::vm_object_dispatcher::VmObjectDispatcher;
use crate::vm::vm_address_region::VmAddressRegionOrMapping;
use crate::vm::vm_mapping::VmMapping;
use std::sync::Arc;
use zircon_macros::syscall;
use zx_status::Status;
use zx_types::*;

#[syscall]
pub unsafe extern "C" fn zx_vmar_allocate(
    parent_vmar: zx_handle_t,
    options: zx_vm_option_t,
    offset: usize,
    size: usize,
    child_vmar: *mut zx_handle_t,
    child_addr: *mut zx_vaddr_t,
) -> zx_status_t {
    // Compute needed rights from requested mapping protections.
    let mut vmar_rights = 0;
    if options & ZX_VM_CAN_MAP_READ != 0 {
        vmar_rights |= ZX_RIGHT_READ;
    }
    if options & ZX_VM_CAN_MAP_WRITE != 0 {
        vmar_rights |= ZX_RIGHT_WRITE;
    }
    if options & ZX_VM_CAN_MAP_EXECUTE != 0 {
        vmar_rights |= ZX_RIGHT_EXECUTE;
    }
    // lookup the dispatcher from handle
    let vmar = handle_table()
        .get_dispatcher_with_rights::<VmAddressRegionDispatcher>(parent_vmar, vmar_rights)?;
    // Create the new VMAR
    let (handle, new_rights) = vmar.allocate(offset, size, options)?;
    let vmar_dispatcher = handle.dispatcher().clone();
    let result = (|| {
        let value = handle.make_and_add_handle(new_rights)?;
        // SAFETY: The pointers are the caller's.
        unsafe {
            copy_out(child_vmar, value)?;
            copy_out(child_addr, vmar_dispatcher.vmar().base())
        }
    })();
    // Setting up the handle failed, so destroy the new VMAR.
    if result.is_err() {
        let _ = vmar_dispatcher.destroy();
    }
    result
}

#[syscall]
pub unsafe extern "C" fn zx_vmar_destroy(handle: zx_handle_t) -> zx_status_t {
    let vmar = handle_table()
        .get_dispatcher_with_rights::<VmAddressRegionDispatcher>(handle, ZX_RIGHT_OP_CHILDREN)?;
    vmar.destroy()
}

#[allow(clippy::too_many_arguments)]
fn vmar_map_common(
    mut options: zx_vm_option_t,
    vmar: &VmAddressRegionDispatcher,
    vmar_offset: usize,
    vmar_rights: zx_rights_t,
    vmo: &VmObjectDispatcher,
    vmo_offset: u64,
    vmo_rights: zx_rights_t,
    len: usize,
) -> Result<(Arc<VmMapping>, zx_vaddr_t), Status> {
    // Test to see if we should even be able to map this.
    ensure!(vmo_rights & ZX_RIGHT_MAP != 0, Status::ACCESS_DENIED);
    // Execute only mappings are not supported, so readable ones stand in for them.
    if options & ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED != 0 {
        options |= ZX_VM_PERM_READ;
    }
    ensure!(VmAddressRegionDispatcher::is_valid_mapping_protection(options), Status::INVALID_ARGS);
    let do_map_range = options & ZX_VM_MAP_RANGE != 0;
    options &= !ZX_VM_MAP_RANGE;
    ensure!(!do_map_range || options & ZX_VM_SPECIFIC_OVERWRITE == 0, Status::INVALID_ARGS);
    // Usermode is not allowed to specify these flags on mappings, though we may set them below.
    ensure!(
        options & (ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE) == 0,
        Status::INVALID_ARGS
    );
    // Permissions allowed by both the VMO and the VMAR.
    let can_read = vmo_rights & ZX_RIGHT_READ != 0 && vmar_rights & ZX_RIGHT_READ != 0;
    let can_write = vmo_rights & ZX_RIGHT_WRITE != 0 && vmar_rights & ZX_RIGHT_WRITE != 0;
    let can_exec = vmo_rights & ZX_RIGHT_EXECUTE != 0 && vmar_rights & ZX_RIGHT_EXECUTE != 0;
    // Test to see if the requested mapping protections are allowed.
    ensure!(options & ZX_VM_PERM_READ == 0 || can_read, Status::ACCESS_DENIED);
    ensure!(options & ZX_VM_PERM_WRITE == 0 || can_write, Status::ACCESS_DENIED);
    ensure!(options & ZX_VM_PERM_EXECUTE == 0 || can_exec, Status::ACCESS_DENIED);
    // If a permission is allowed by both the VMO and the VMAR, add it to the flags for the new
    // mapping, so that the VMO's rights as of now can be used to constrain future permission
    // changes via Protect().
    if can_read {
        options |= ZX_VM_CAN_MAP_READ;
    }
    if can_write {
        options |= ZX_VM_CAN_MAP_WRITE;
    }
    if can_exec {
        options |= ZX_VM_CAN_MAP_EXECUTE;
    }
    if options & ZX_VM_FAULT_BEYOND_STREAM_SIZE != 0 {
        // Streams are not supported, so VMOs have no user stream size to fault beyond.
        return Err(Status::INVALID_ARGS);
    }
    let (mapping, base) = vmar.map(vmar_offset, vmo.vmo().clone(), vmo_offset, len, options)?;
    // Setup a handler to destroy the new mapping if the syscall is unsuccessful.
    if do_map_range && let Err(status) = mapping.map_range(0, len, false, true) {
        let _ = VmAddressRegionOrMapping::Mapping(mapping).destroy();
        return Err(status);
    }
    Ok((mapping, base))
}

#[syscall]
pub unsafe extern "C" fn zx_vmar_map(
    handle: zx_handle_t,
    options: zx_vm_option_t,
    vmar_offset: usize,
    vmo: zx_handle_t,
    vmo_offset: u64,
    len: usize,
    mapped_addr: *mut zx_vaddr_t,
) -> zx_status_t {
    // lookup the VMAR dispatcher from handle
    let (vmar, vmar_rights) =
        handle_table().get_dispatcher_and_rights_as::<VmAddressRegionDispatcher>(handle)?;
    // lookup the VMO dispatcher from handle
    let (vmo, vmo_rights): (Arc<VmObjectDispatcher>, _) =
        handle_table().get_dispatcher_and_rights_as::<VmObjectDispatcher>(vmo)?;
    let (mapping, base) = vmar_map_common(
        options,
        &vmar,
        vmar_offset,
        vmar_rights,
        &vmo,
        vmo_offset,
        vmo_rights,
        len,
    )?;
    // SAFETY: `mapped_addr` is the caller's.
    let result = unsafe { copy_out(mapped_addr, base) };
    // Destroy the new mapping if the syscall is unsuccessful.
    if result.is_err() {
        let _ = VmAddressRegionOrMapping::Mapping(mapping).destroy();
    }
    result
}

#[syscall]
pub unsafe extern "C" fn zx_vmar_unmap(
    handle: zx_handle_t,
    addr: zx_vaddr_t,
    len: usize,
) -> zx_status_t {
    // lookup the dispatcher from handle
    let (vmar, vmar_rights) =
        handle_table().get_dispatcher_and_rights_as::<VmAddressRegionDispatcher>(handle)?;
    vmar.unmap(addr, len, VmAddressRegionDispatcher::op_children_from_rights(vmar_rights))
}

#[syscall]
pub unsafe extern "C" fn zx_vmar_protect(
    handle: zx_handle_t,
    mut options: zx_vm_option_t,
    addr: zx_vaddr_t,
    len: usize,
) -> zx_status_t {
    // Execute only mappings are not supported, so readable ones stand in for them.
    if options & ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED != 0 {
        options |= ZX_VM_PERM_READ;
    }
    let mut vmar_rights = 0;
    if options & ZX_VM_PERM_READ != 0 {
        vmar_rights |= ZX_RIGHT_READ;
    }
    if options & ZX_VM_PERM_WRITE != 0 {
        vmar_rights |= ZX_RIGHT_WRITE;
    }
    if options & ZX_VM_PERM_EXECUTE != 0 {
        vmar_rights |= ZX_RIGHT_EXECUTE;
    }
    // lookup the dispatcher from handle
    let (vmar, actual_rights) =
        handle_table().get_dispatcher_and_rights_as::<VmAddressRegionDispatcher>(handle)?;
    ensure!(actual_rights & vmar_rights == vmar_rights, Status::ACCESS_DENIED);
    ensure!(VmAddressRegionDispatcher::is_valid_mapping_protection(options), Status::INVALID_ARGS);
    vmar.protect(
        addr,
        len,
        options,
        VmAddressRegionDispatcher::op_children_from_rights(actual_rights),
    )
}

#[syscall]
pub unsafe extern "C" fn zx_vmar_op_range(
    handle: zx_handle_t,
    op: u32,
    address: zx_vaddr_t,
    size: usize,
    buffer: *mut u8,
    buffer_size: usize,
) -> zx_status_t {
    let (vmar, vmar_rights) =
        handle_table().get_dispatcher_and_rights_as::<VmAddressRegionDispatcher>(handle)?;
    ensure!(buffer.is_null() && buffer_size == 0, Status::INVALID_ARGS);
    vmar.range_op(op, address, size, vmar_rights)
}
