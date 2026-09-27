//! VMO syscalls, ported from `zircon/kernel/lib/syscalls/vmo.rs`.
//!
//! Deviation: no children, contiguous or physical VMOs, and no cache policies.

use super::{copy_out, status};
use crate::object::dispatcher::Dispatcher;
use crate::object::handle::handle_table;
use crate::object::vm_object_dispatcher::{InitialMutability, VmObjectDispatcher};
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::vm::pmm::{ALLOC_FLAG_ANY, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_cow_pages::SupplyOptions;
use crate::vm::vm_object_paged::VmObjectPaged;
use crate::vm::vm_page_list::VmPageSpliceList;
use zx_status::Status;
use zx_types::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_create(
    size: u64,
    options: u32,
    out: *mut zx_handle_t,
) -> zx_status_t {
    status((|| {
        let stats = VmObjectDispatcher::parse_create_syscall_flags(options, size)?;
        // create a vm object
        let vmo =
            VmObjectPaged::create(ALLOC_FLAG_ANY | ALLOC_FLAG_CAN_WAIT, stats.flags, stats.size)?;
        // create a Vm Object dispatcher
        let (kernel_handle, rights) =
            VmObjectDispatcher::create(vmo, size, InitialMutability::Mutable)?;
        // create a handle and attach the dispatcher to it
        let value = kernel_handle.make_and_add_handle(rights)?;
        // SAFETY: `out` is the caller's.
        unsafe { copy_out(out, value) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_read(
    handle: zx_handle_t,
    data: *mut u8,
    offset: u64,
    len: usize,
) -> zx_status_t {
    status((|| {
        let vmo = handle_table()
            .get_dispatcher_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_READ)?;
        vmo.read(UserOutPtr::new(data), offset, len).0
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_write(
    handle: zx_handle_t,
    data: *const u8,
    offset: u64,
    len: usize,
) -> zx_status_t {
    status((|| {
        let vmo = handle_table()
            .get_dispatcher_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_WRITE)?;
        vmo.write(UserInPtr::new(data), offset, len).0
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_transfer_data(
    dst_vmo_handle: zx_handle_t,
    options: u32,
    offset: u64,
    length: u64,
    src_vmo_handle: zx_handle_t,
    src_offset: u64,
) -> zx_status_t {
    status((|| {
        // Currently, there are no supported options. This may change in the future.
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        if !page::is_aligned(offset as usize)
            || !page::is_aligned(length as usize)
            || !page::is_aligned(src_offset as usize)
        {
            return Err(Status::INVALID_ARGS);
        }
        let dst_vmo_dispatcher = handle_table()
            .get_dispatcher_with_rights::<VmObjectDispatcher>(dst_vmo_handle, ZX_RIGHT_WRITE)?;
        let src_vmo_dispatcher = handle_table().get_dispatcher_with_rights::<VmObjectDispatcher>(
            src_vmo_handle,
            ZX_RIGHT_READ | ZX_RIGHT_WRITE,
        )?;
        // Short circuit out if src_vmo and dst_vmo are identical and the src_offset is the same
        // as the destination offset.
        if src_vmo_dispatcher.base().get_koid() == dst_vmo_dispatcher.base().get_koid()
            && src_offset == offset
        {
            return Ok(());
        }
        let mut pages = VmPageSpliceList::new();
        src_vmo_dispatcher.vmo().take_pages(src_offset, length, &mut pages)?;
        dst_vmo_dispatcher.vmo().supply_pages(
            offset,
            length,
            &mut pages,
            SupplyOptions::TransferData,
        )
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_get_size(handle: zx_handle_t, size: *mut u64) -> zx_status_t {
    status((|| {
        // no rights check, anyone should be able to get the size
        let vmo = handle_table().get_dispatcher::<VmObjectDispatcher>(handle)?;
        // SAFETY: `size` is the caller's.
        unsafe { copy_out(size, vmo.get_size()) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_get_stream_size(
    handle: zx_handle_t,
    size: *mut u64,
) -> zx_status_t {
    status((|| {
        // no rights required to get stream size
        let vmo = handle_table().get_dispatcher::<VmObjectDispatcher>(handle)?;
        // SAFETY: `size` is the caller's.
        unsafe { copy_out(size, vmo.get_stream_size()) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_set_size(handle: zx_handle_t, size: u64) -> zx_status_t {
    status((|| {
        let (vmo, rights) =
            handle_table().get_dispatcher_and_rights_as::<VmObjectDispatcher>(handle)?;
        if rights & ZX_RIGHT_WRITE == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        // VMOs that are not resizable should fail with ZX_ERR_UNAVAILABLE for backwards
        // compatibility, which will be handled by the set_size call below. Only validate the RESIZE
        // right if the VMO is resizable.
        if vmo.vmo().is_resizable() && (rights & ZX_RIGHT_RESIZE) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        vmo.set_size(size)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_set_stream_size(handle: zx_handle_t, size: u64) -> zx_status_t {
    status((|| {
        let vmo = handle_table()
            .get_dispatcher_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_WRITE)?;
        vmo.set_stream_size(size)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_vmo_op_range(
    handle: zx_handle_t,
    op: u32,
    offset: u64,
    size: u64,
    _buffer: *mut u8,
    _buffer_size: usize,
) -> zx_status_t {
    status((|| {
        // save the rights and pass down into the dispatcher for further testing
        let (vmo, rights) =
            handle_table().get_dispatcher_and_rights_as::<VmObjectDispatcher>(handle)?;
        vmo.range_op(op, offset, size, rights)
    })())
}
