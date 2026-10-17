// Copyright 2018 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pager syscalls, ported from `zircon/kernel/lib/syscalls/pager.cc`.
//!
//! Deviations:
//! - `zx_pager_supply_shared_pages` is not in Zircon; see `VmCowPages::lend_pages`.

use super::copy_out;
use crate::ensure;
use crate::object::handle::handle_table;
use crate::object::pager_dispatcher::PagerDispatcher;
use crate::object::port_dispatcher::PortDispatcher;
use crate::object::vm_object_dispatcher::{InitialMutability, VmObjectDispatcher};
use crate::user_copy::UserOutPtr;
use crate::vm::vm_cow_pages::SupplyOptions;
use crate::vm::vm_object_paged::VmObjectPaged;
use crate::vm::vm_page_list::VmPageSpliceList;
use std::sync::Arc;
use zircon_macros::syscall;
use zx_status::Status;
use zx_types::*;

/// Splits out the pager vmo creation flags between PageSource and VmObjectPaged flags.
fn split_syscall_flags(flags: u32) -> (u32, u32) {
    // Extract any option flags relevant to creation of the page source.
    let src_flags = flags & ZX_VMO_TRAP_DIRTY;
    // Mask out any source flags. The remaining flags are the vmo creation flags; vmo creation will
    // perform validation on them.
    (src_flags, flags & !ZX_VMO_TRAP_DIRTY)
}

/// Returns the pager and a VMO it created, checking the handles have `pager_rights` and
/// `vmo_rights`.
fn get_pager_and_vmo(
    pager: zx_handle_t,
    pager_rights: zx_rights_t,
    pager_vmo: zx_handle_t,
    vmo_rights: zx_rights_t,
) -> Result<(Arc<PagerDispatcher>, Arc<VmObjectDispatcher>), Status> {
    let pager_dispatcher =
        handle_table().get_dispatcher_with_rights::<PagerDispatcher>(pager, pager_rights)?;
    let vmo_dispatcher =
        handle_table().get_dispatcher_with_rights::<VmObjectDispatcher>(pager_vmo, vmo_rights)?;
    ensure!(vmo_dispatcher.pager_koid() == pager_dispatcher.get_koid(), Status::INVALID_ARGS);
    Ok((pager_dispatcher, vmo_dispatcher))
}

#[syscall]
pub unsafe extern "C" fn zx_pager_create(options: u32, out: *mut zx_handle_t) -> zx_status_t {
    ensure!(options == 0, Status::INVALID_ARGS);
    let (handle, rights) = PagerDispatcher::create()?;
    // Stash the name of the creating process for debugging.
    let name = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default();
    handle.dispatcher().set_debug_name(name.as_bytes());
    let value = handle.make_and_add_handle(rights)?;
    // SAFETY: `out` is the caller's.
    unsafe { copy_out(out, value) }
}

#[syscall]
pub unsafe extern "C" fn zx_pager_create_vmo(
    pager: zx_handle_t,
    options: u32,
    port: zx_handle_t,
    key: u64,
    size: u64,
    out: *mut zx_handle_t,
) -> zx_status_t {
    let pager_dispatcher =
        handle_table().get_dispatcher_with_rights::<PagerDispatcher>(pager, ZX_RIGHT_ATTACH_VMO)?;
    let port_dispatcher =
        handle_table().get_dispatcher_with_rights::<PortDispatcher>(port, ZX_RIGHT_WRITE)?;
    let (source_flags, vmo_flags) = split_syscall_flags(options);
    let src = pager_dispatcher.create_source(port_dispatcher, key, source_flags)?;
    let stats = VmObjectDispatcher::parse_create_syscall_flags(vmo_flags, size)?;
    let vmo = VmObjectPaged::create_external(src, stats.flags, stats.size)?;
    let (kernel_handle, rights) =
        VmObjectDispatcher::create(vmo.clone(), size, InitialMutability::Mutable)?;
    // As part of VMO creation some initial zeroing might have happened, placing the VMO in a
    // modified state. However, as the zeroing as part of the initialization, we do not want to
    // consider it modified and so reset any modification information.
    vmo.reset_pager_vmo_stats();
    let value = kernel_handle.make_and_add_handle(rights)?;
    // SAFETY: `out` is the caller's.
    unsafe { copy_out(out, value) }
}

#[syscall]
pub unsafe extern "C" fn zx_pager_detach_vmo(pager: zx_handle_t, vmo: zx_handle_t) -> zx_status_t {
    let (_, vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_ATTACH_VMO | ZX_RIGHT_MANAGE_VMO, vmo, ZX_RIGHT_WRITE)?;
    vmo_dispatcher.vmo().detach_source();
    Ok(())
}

#[syscall]
pub unsafe extern "C" fn zx_pager_supply_pages(
    pager: zx_handle_t,
    pager_vmo: zx_handle_t,
    offset: u64,
    length: u64,
    aux_vmo: zx_handle_t,
    aux_offset: u64,
) -> zx_status_t {
    if !page::is_aligned(offset as usize)
        || !page::is_aligned(length as usize)
        || !page::is_aligned(aux_offset as usize)
    {
        return Err(Status::INVALID_ARGS);
    }
    let (_, pager_vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_MANAGE_VMO, pager_vmo, ZX_RIGHT_WRITE)?;
    let aux_vmo_dispatcher = handle_table().get_dispatcher_with_rights::<VmObjectDispatcher>(
        aux_vmo,
        ZX_RIGHT_READ | ZX_RIGHT_WRITE,
    )?;
    let mut pages = VmPageSpliceList::new();
    aux_vmo_dispatcher.vmo().take_pages(aux_offset, length, &mut pages)?;
    pager_vmo_dispatcher.vmo().supply_pages(offset, length, &mut pages, SupplyOptions::PagerSupply)
}

/// Beyond Zircon: like `zx_pager_supply_pages`, but the pages of `src_vmo`, a pager-backed VMO,
/// are shared with `pager_vmo` rather than moved, until either changes them.
#[syscall]
pub unsafe extern "C" fn zx_pager_supply_shared_pages(
    pager: zx_handle_t,
    pager_vmo: zx_handle_t,
    offset: u64,
    length: u64,
    src_vmo: zx_handle_t,
    src_offset: u64,
) -> zx_status_t {
    if !page::is_aligned(offset as usize)
        || !page::is_aligned(length as usize)
        || !page::is_aligned(src_offset as usize)
    {
        return Err(Status::INVALID_ARGS);
    }
    let (_, pager_vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_MANAGE_VMO, pager_vmo, ZX_RIGHT_WRITE)?;
    let src_vmo_dispatcher =
        handle_table().get_dispatcher_with_rights::<VmObjectDispatcher>(src_vmo, ZX_RIGHT_READ)?;
    pager_vmo_dispatcher.vmo().supply_shared_pages(
        offset,
        length,
        src_vmo_dispatcher.vmo(),
        src_offset,
    )
}

#[syscall]
pub unsafe extern "C" fn zx_pager_op_range(
    pager: zx_handle_t,
    op: u32,
    pager_vmo: zx_handle_t,
    offset: u64,
    length: u64,
    data: u64,
) -> zx_status_t {
    ensure!(
        page::is_aligned(offset as usize) && page::is_aligned(length as usize),
        Status::INVALID_ARGS
    );
    let (pager_dispatcher, pager_vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_MANAGE_VMO, pager_vmo, ZX_RIGHT_WRITE)?;
    pager_dispatcher.range_op(op, pager_vmo_dispatcher.vmo(), offset, length, data)
}

#[syscall]
pub unsafe extern "C" fn zx_pager_query_dirty_ranges(
    pager: zx_handle_t,
    pager_vmo: zx_handle_t,
    offset: u64,
    length: u64,
    buffer: *mut u8,
    buffer_size: usize,
    actual: *mut usize,
    avail: *mut usize,
) -> zx_status_t {
    let (pager_dispatcher, pager_vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_MANAGE_VMO, pager_vmo, ZX_RIGHT_READ)?;
    pager_dispatcher.query_dirty_ranges(
        pager_vmo_dispatcher.vmo(),
        offset,
        length,
        UserOutPtr::new(buffer),
        buffer_size,
        UserOutPtr::new(actual),
        UserOutPtr::new(avail),
    )
}

#[syscall]
pub unsafe extern "C" fn zx_pager_query_vmo_stats(
    pager: zx_handle_t,
    pager_vmo: zx_handle_t,
    options: u32,
    buffer: *mut u8,
    buffer_size: usize,
) -> zx_status_t {
    let (pager_dispatcher, pager_vmo_dispatcher) =
        get_pager_and_vmo(pager, ZX_RIGHT_MANAGE_VMO, pager_vmo, ZX_RIGHT_READ)?;
    pager_dispatcher.query_pager_vmo_stats(
        pager_vmo_dispatcher.vmo(),
        options,
        UserOutPtr::new(buffer),
        buffer_size,
    )
}
