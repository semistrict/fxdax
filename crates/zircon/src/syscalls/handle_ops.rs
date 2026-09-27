//! Handle syscalls, ported from `zircon/kernel/lib/syscalls/handle_ops.cc`.

use super::{copy_out, status};
use crate::object::handle::handle_table;
use zx_status::Status;
use zx_types::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_handle_close(handle: zx_handle_t) -> zx_status_t {
    // Closing the "never a handle" invalid handle is not an error. It's like free(NULL).
    if handle == ZX_HANDLE_INVALID {
        return ZX_OK;
    }
    match handle_table().remove_handle(handle) {
        Some(_) => ZX_OK,
        None => ZX_ERR_BAD_HANDLE,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_handle_close_many(
    handles: *const zx_handle_t,
    num_handles: usize,
) -> zx_status_t {
    if num_handles == 0 {
        return ZX_OK;
    }
    if handles.is_null() {
        return ZX_ERR_INVALID_ARGS;
    }
    // SAFETY: The caller passes `num_handles` handles.
    let handles = unsafe { std::slice::from_raw_parts(handles, num_handles) };
    let mut result = ZX_OK;
    for &handle in handles {
        // SAFETY: No pointers.
        if unsafe { zx_handle_close(handle) } != ZX_OK {
            result = ZX_ERR_BAD_HANDLE;
        }
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_handle_duplicate(
    handle: zx_handle_t,
    rights: zx_rights_t,
    out: *mut zx_handle_t,
) -> zx_status_t {
    status((|| {
        let value = handle_table().dup_replace(false, handle, rights)?;
        // SAFETY: `out` is the caller's.
        unsafe { copy_out(out, value) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_handle_replace(
    handle: zx_handle_t,
    rights: zx_rights_t,
    out: *mut zx_handle_t,
) -> zx_status_t {
    status((|| {
        let value = handle_table().dup_replace(true, handle, rights)?;
        // SAFETY: `out` is the caller's.
        unsafe { copy_out(out, value) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_handle_check_valid(handle: zx_handle_t) -> zx_status_t {
    status(handle_table().with_handle(handle, |_| ()).map_err(|_: Status| Status::BAD_HANDLE))
}
