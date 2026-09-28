//! Stream syscalls, ported from `zircon/kernel/lib/syscalls/stream.rs`.

use super::{copy_out, copy_out_optional, status};
use crate::object::handle::handle_table;
use crate::object::stream_dispatcher::StreamDispatcher;
use crate::object::vm_object_dispatcher::VmObjectDispatcher;
use crate::user_copy::{UserInPtr, UserOutPtr, make_user_in_iovec, make_user_out_iovec};
use zx_status::Status;
use zx_types::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_create(
    options: u32,
    vmo_handle: zx_handle_t,
    seek: zx_off_t,
    out_stream: *mut zx_handle_t,
) -> zx_status_t {
    status((|| {
        if (options & !ZX_STREAM_CREATE_MASK) != 0 {
            return Err(Status::INVALID_ARGS);
        }

        let (mut stream_options, desired_vmo_rights) =
            StreamDispatcher::parse_create_syscall_flags(options)?;

        let (vmo_disp, actual_vmo_rights) =
            handle_table().get_dispatcher_and_rights_as::<VmObjectDispatcher>(vmo_handle)?;
        if (actual_vmo_rights & desired_vmo_rights) != desired_vmo_rights {
            return Err(Status::ACCESS_DENIED);
        }

        // Cannot create a stream from a physical or contiguous VMO.
        if vmo_disp.vmo().is_contiguous() {
            return Err(Status::WRONG_TYPE);
        }

        // Remember whether this stream can resize the underlying VMO when required. Note that it
        // might be possible for stream writes / appends to proceed by manipulating only the stream
        // size, without having to change the VMO size. This flag will be checked only for stream
        // operations that would require manipulating the VMO size, more specifically when expanding
        // the VMO size if the requested stream size needs to surpass the current VMO size.
        if (actual_vmo_rights & ZX_RIGHT_RESIZE) != 0 {
            stream_options.set_can_resize_vmo(true);
        }

        let (kernel_handle, rights) = StreamDispatcher::create(stream_options, &vmo_disp, seek)?;
        let user_handle = kernel_handle.make_and_add_handle(rights)?;
        // SAFETY: `out_stream` is the caller's.
        unsafe { copy_out(out_stream, user_handle) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_writev(
    handle: zx_handle_t,
    options: u32,
    vector: *const zx_iovec_t,
    vector_count: usize,
    out_actual: *mut usize,
) -> zx_status_t {
    status((|| {
        if (options & !ZX_STREAM_APPEND) != 0 {
            return Err(Status::INVALID_ARGS);
        }
        if vector.is_null() {
            return Err(Status::INVALID_ARGS);
        }

        let stream = handle_table()
            .get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_WRITE)?;
        let user_data = make_user_in_iovec(UserInPtr::new(vector), vector_count);
        let actual = if (options & ZX_STREAM_APPEND) != 0 {
            stream.append_vector(user_data)?
        } else {
            stream.write_vector(user_data)?
        };

        // SAFETY: `out_actual` is the caller's.
        unsafe { copy_out_optional(out_actual, actual) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_writev_at(
    handle: zx_handle_t,
    options: u32,
    offset: zx_off_t,
    vector: *const zx_iovec_t,
    vector_count: usize,
    out_actual: *mut usize,
) -> zx_status_t {
    status((|| {
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        if vector.is_null() {
            return Err(Status::INVALID_ARGS);
        }

        let stream = handle_table()
            .get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_WRITE)?;
        let actual = stream
            .write_vector_at(make_user_in_iovec(UserInPtr::new(vector), vector_count), offset)?;

        // SAFETY: `out_actual` is the caller's.
        unsafe { copy_out_optional(out_actual, actual) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_readv(
    handle: zx_handle_t,
    options: u32,
    vector: *mut zx_iovec_t,
    vector_count: usize,
    out_actual: *mut usize,
) -> zx_status_t {
    status((|| {
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        if vector.is_null() {
            return Err(Status::INVALID_ARGS);
        }

        let stream =
            handle_table().get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_READ)?;
        let actual =
            stream.read_vector(make_user_out_iovec(UserOutPtr::new(vector), vector_count))?;

        // SAFETY: `out_actual` is the caller's.
        unsafe { copy_out_optional(out_actual, actual) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_readv_at(
    handle: zx_handle_t,
    options: u32,
    offset: zx_off_t,
    vector: *mut zx_iovec_t,
    vector_count: usize,
    out_actual: *mut usize,
) -> zx_status_t {
    status((|| {
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        if vector.is_null() {
            return Err(Status::INVALID_ARGS);
        }

        let stream =
            handle_table().get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_READ)?;
        let actual = stream
            .read_vector_at(make_user_out_iovec(UserOutPtr::new(vector), vector_count), offset)?;

        // SAFETY: `out_actual` is the caller's.
        unsafe { copy_out_optional(out_actual, actual) }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_stream_seek(
    handle: zx_handle_t,
    whence: zx_stream_seek_origin_t,
    offset: i64,
    out_seek: *mut zx_off_t,
) -> zx_status_t {
    status((|| {
        let (stream, rights) =
            handle_table().get_dispatcher_and_rights_as::<StreamDispatcher>(handle)?;
        if (rights & (ZX_RIGHT_READ | ZX_RIGHT_WRITE)) == 0 {
            return Err(Status::ACCESS_DENIED);
        }

        let seek = stream.seek(whence, offset)?;
        // SAFETY: `out_seek` is the caller's.
        unsafe { copy_out_optional(out_seek, seek) }
    })())
}
