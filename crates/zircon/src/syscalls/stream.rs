//! Stream syscalls, ported from `zircon/kernel/lib/syscalls/stream.rs`.
//!
//! Deviation: `stream_readv_at_async` and `stream_writev_at_async` are what `zx_stream_readv_at`
//! and `zx_stream_writev_at` do, for host memory, as futures that wait rather than blocking their
//! thread (see `kernel::wakers`). They are Rust functions, not syscalls.

use super::{copy_out, copy_out_optional, status};
use crate::ensure;
use crate::object::handle::handle_table;
use crate::object::stream_dispatcher::StreamDispatcher;
use crate::object::vm_object_dispatcher::VmObjectDispatcher;
use crate::user_copy::{
    HostBuffer, UserInPtr, UserOutPtr, make_user_in_iovec, make_user_out_iovec,
};
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
        ensure!((options & !ZX_STREAM_CREATE_MASK) == 0, Status::INVALID_ARGS);

        let (mut stream_options, desired_vmo_rights) =
            StreamDispatcher::parse_create_syscall_flags(options)?;

        let (vmo_disp, actual_vmo_rights) =
            handle_table().get_dispatcher_and_rights_as::<VmObjectDispatcher>(vmo_handle)?;
        ensure!(
            (actual_vmo_rights & desired_vmo_rights) == desired_vmo_rights,
            Status::ACCESS_DENIED
        );

        // Cannot create a stream from a physical or contiguous VMO.
        ensure!(!vmo_disp.vmo().is_contiguous(), Status::WRONG_TYPE);

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
        ensure!((options & !ZX_STREAM_APPEND) == 0, Status::INVALID_ARGS);
        ensure!(!vector.is_null(), Status::INVALID_ARGS);

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
        ensure!(options == 0, Status::INVALID_ARGS);
        ensure!(!vector.is_null(), Status::INVALID_ARGS);

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
        ensure!(options == 0, Status::INVALID_ARGS);
        ensure!(!vector.is_null(), Status::INVALID_ARGS);

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
        ensure!(options == 0, Status::INVALID_ARGS);
        ensure!(!vector.is_null(), Status::INVALID_ARGS);

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
        ensure!((rights & (ZX_RIGHT_READ | ZX_RIGHT_WRITE)) != 0, Status::ACCESS_DENIED);

        let seek = stream.seek(whence, offset)?;
        // SAFETY: `out_seek` is the caller's.
        unsafe { copy_out_optional(out_seek, seek) }
    })())
}

/// Reads from the stream `handle` at `offset` into the host buffers `bufs`, as `zx_stream_readv_at` does, as a
/// future that waits for pages rather than blocking the calling thread. Dropping the future
/// before it completes leaves the stream as if the read had finished with what it read so far.
pub async fn stream_readv_at_async(
    handle: zx_handle_t,
    offset: zx_off_t,
    bufs: &[HostBuffer],
) -> Result<usize, Status> {
    let stream =
        handle_table().get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_READ)?;
    stream.read_vectored_at_async(bufs, offset).await
}

/// Writes the host buffers `bufs` to the stream `handle` at `offset`, as `zx_stream_writev_at` does, as a future
/// that waits for pages and for the writes ahead of it rather than blocking the calling thread.
/// Dropping the future before it completes leaves the stream as if the write had finished with
/// what it wrote so far.
pub async fn stream_writev_at_async(
    handle: zx_handle_t,
    offset: zx_off_t,
    bufs: &[HostBuffer],
) -> Result<usize, Status> {
    let stream =
        handle_table().get_dispatcher_with_rights::<StreamDispatcher>(handle, ZX_RIGHT_WRITE)?;
    stream.write_vectored_at_async(bufs, offset).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The futures are `Send`, so that they may run on any executor's threads.
    #[test]
    fn async_stream_io_is_send() {
        fn send<T: Send>(_: &T) {}
        let bufs = [HostBuffer { addr: 0, len: 1 }];
        send(&stream_readv_at_async(0, 0, &bufs));
        send(&stream_writev_at_async(0, 0, &bufs));
    }
}
