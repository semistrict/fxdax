//! Stream objects.
//!
//! Ported from `zircon/kernel/object/stream_dispatcher.rs`, `zircon/kernel/object/stream_dispatcher.cc`
//! and `zircon/kernel/object/stream_dispatcher_ffi.cc`.
//!
//! Deviations:
//! - The state is the dispatcher's own fields, behind `parking_lot` mutexes, rather than state the
//!   C++ dispatcher embeds.
//! - The VMO's vector reads and writes return how much they transferred as `vm_object_paged.rs`'s
//!   do (see `transferred`), and progress is reported through a closure rather than a C callback
//!   and cookie.
//! - No kernel counters.

use super::dispatcher::{Dispatcher, DispatcherBase};
use super::handle::KernelHandle;
use super::vm_object_dispatcher::VmObjectDispatcher;
use crate::user_copy::{UserInIovec, UserOutIovec};
use crate::vm::stream_size_manager::{Operation as StreamSizeManagerOperation, StreamSizeManager};
use crate::vm::vm_object_paged::VmObjectPaged;
use parking_lot::Mutex;
use std::sync::Arc;
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_STREAM, ZX_RIGHT_DUPLICATE, ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_READ,
    ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE,
    zx_info_stream_t, zx_obj_type_t, zx_off_t, zx_rights_t, zx_stream_seek_origin_t,
};

/// Options and modes for a stream dispatcher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamOptions(u32);

impl StreamOptions {
    /// Read mode is enabled.
    pub const READ_MASK: u32 = 1 << 0;
    /// Write mode is enabled.
    pub const WRITE_MASK: u32 = 1 << 1;
    /// Append mode is enabled.
    pub const APPEND_MASK: u32 = 1 << 2;
    /// Resizing the VMO is permitted.
    pub const CAN_RESIZE_VMO_MASK: u32 = 1 << 3;

    /// Mask of valid stream mode bits for stream creation (`READ`, `WRITE`, `APPEND`).
    pub const MODE_MASK: u32 = Self::READ_MASK | Self::WRITE_MASK | Self::APPEND_MASK;

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub fn read(self) -> bool {
        self.0 & Self::READ_MASK != 0
    }

    pub fn write(self) -> bool {
        self.0 & Self::WRITE_MASK != 0
    }

    pub fn append(self) -> bool {
        self.0 & Self::APPEND_MASK != 0
    }

    pub fn can_resize_vmo(self) -> bool {
        self.0 & Self::CAN_RESIZE_VMO_MASK != 0
    }

    fn set(&mut self, mask: u32, value: bool) {
        if value {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }

    pub fn set_append(&mut self, value: bool) {
        self.set(Self::APPEND_MASK, value);
    }

    pub fn set_can_resize_vmo(&mut self, value: bool) {
        self.set(Self::CAN_RESIZE_VMO_MASK, value);
    }

    /// Converts the stream mode flags into raw `zx_types::ZX_STREAM_MODE_*` bits.
    pub const fn to_stream_modes(self) -> u32 {
        self.bits() & Self::MODE_MASK
    }
}

impl From<u32> for StreamOptions {
    fn from(bits: u32) -> Self {
        Self(bits)
    }
}

const _: () = {
    assert!(StreamOptions::READ_MASK == zx_types::ZX_STREAM_MODE_READ);
    assert!(StreamOptions::WRITE_MASK == zx_types::ZX_STREAM_MODE_WRITE);
    assert!(StreamOptions::APPEND_MASK == zx_types::ZX_STREAM_MODE_APPEND);
};

const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_TRANSFER
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_SIGNAL;

/// How much a VMO read or write transferred, or its error if it transferred nothing, as the
/// results of `vm_object_paged.rs`'s `read_user_vector` and `write_user_vector` are.
fn transferred((status, actual): (Result<(), Status>, usize)) -> Result<usize, Status> {
    if actual > 0 {
        Ok(actual)
    } else {
        status?;
        Ok(0)
    }
}

pub struct StreamDispatcher {
    base: DispatcherBase,

    options: Mutex<StreamOptions>,

    seek: Mutex<u64>,

    vmo: Arc<VmObjectPaged>,

    stream_size_mgr: Arc<StreamSizeManager>,
}

impl Dispatcher for StreamDispatcher {
    fn base(&self) -> &DispatcherBase {
        &self.base
    }

    fn get_type(&self) -> zx_obj_type_t {
        ZX_OBJ_TYPE_STREAM
    }

    fn default_rights(&self) -> zx_rights_t {
        DEFAULT_RIGHTS
    }
}

impl StreamDispatcher {
    /// Parses and validates syscall flag options for stream creation.
    pub fn parse_create_syscall_flags(
        options: u32,
    ) -> Result<(StreamOptions, zx_rights_t), Status> {
        if (options & !StreamOptions::MODE_MASK) != 0 {
            return Err(Status::INVALID_ARGS);
        }

        let stream_options = StreamOptions::from(options);
        let mut required_vmo_rights = 0;
        if stream_options.read() {
            required_vmo_rights |= ZX_RIGHT_READ;
        }
        if stream_options.write() || stream_options.append() {
            required_vmo_rights |= ZX_RIGHT_WRITE;
        }
        Ok((stream_options, required_vmo_rights))
    }

    /// Creates a new StreamDispatcher and returns its kernel handle and rights.
    pub fn create(
        options: StreamOptions,
        vmo_dispatcher: &VmObjectDispatcher,
        seek: zx_off_t,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let mut rights = DEFAULT_RIGHTS;
        if options.read() {
            rights |= ZX_RIGHT_READ;
        }
        if options.write() {
            rights |= ZX_RIGHT_WRITE;
        }
        let dispatcher = Arc::new(Self {
            base: DispatcherBase::new(0),
            options: Mutex::new(options),
            seek: Mutex::new(seek),
            vmo: vmo_dispatcher.vmo().clone(),
            stream_size_mgr: vmo_dispatcher.stream_size_manager(),
        });
        Ok((KernelHandle::new(dispatcher), rights))
    }

    fn create_write_op(
        &self,
        total_capacity: usize,
        offset: zx_off_t,
        op: &StreamSizeManagerOperation<'_>,
    ) -> Result<(u64, Option<u64>), Status> {
        let (length, prev_stream_size) = {
            let mut stream_size_guard = self.stream_size_mgr.lock();

            let requested_stream_size =
                offset.checked_add(total_capacity as u64).ok_or(Status::FILE_BIG)?;

            let prev_stream_size = self.stream_size_mgr.begin_write_locked(
                &mut stream_size_guard,
                requested_stream_size,
                op,
            );

            let vmo_size = self.vmo.size();
            if vmo_size <= offset {
                // We can't even perform a partial write
                op.cancel_locked(&mut stream_size_guard);
                return Err(Status::OUT_OF_RANGE);
            }

            // Allow writing up to the minimum of the VMO size and requested stream size, since we
            // want to write at most the requested size but don't want to write beyond the VMO size.
            let target_stream_size = core::cmp::min(vmo_size, requested_stream_size);
            let length = target_stream_size - offset;

            if target_stream_size != requested_stream_size {
                op.shrink_size_locked(&mut stream_size_guard, target_stream_size);
            }

            (length, prev_stream_size)
        };

        // Zero content between the previous stream size and the start of the write.
        if let Some(prev) = prev_stream_size
            && prev < offset
        {
            let status = self.vmo.zero_range(prev, offset - prev);
            if let Err(err) = status {
                let mut stream_size_guard = self.stream_size_mgr.lock();
                op.cancel_locked(&mut stream_size_guard);
                return Err(err);
            }
        }

        Ok((length, prev_stream_size))
    }

    /// Writes `user_data` at `offset`, reporting to `op` the progress beyond the stream size
    /// `prev_stream_size` if the write expands the stream.
    fn write_vmo(
        &self,
        user_data: UserInIovec,
        offset: u64,
        length: usize,
        prev_stream_size: Option<u64>,
        op: &StreamSizeManagerOperation<'_>,
    ) -> Result<usize, Status> {
        transferred(if let Some(prev) = prev_stream_size {
            self.vmo.write_user_vector_progress(
                user_data,
                offset,
                length,
                prev,
                &|write_offset, len| {
                    op.update_stream_size_from_progress(write_offset + len as u64);
                },
            )
        } else {
            self.vmo.write_user_vector(user_data, offset, length, None)
        })
    }

    /// Reads data from the stream using an output iovec array.
    pub fn read_vector(&self, user_data: UserOutIovec) -> Result<usize, Status> {
        let total_capacity = user_data.get_total_capacity()?;
        if total_capacity == 0 {
            return Ok(0);
        }

        let op = StreamSizeManagerOperation::new(&self.stream_size_mgr);

        let mut seek_guard = self.seek.lock();
        let (offset, length) = {
            let mut stream_size_guard = self.stream_size_mgr.lock();

            let seek = *seek_guard;
            let size_limit = self.stream_size_mgr.begin_read_locked(
                &mut stream_size_guard,
                seek.saturating_add(total_capacity as u64),
                &op,
            );
            if size_limit <= seek {
                // Return |ZX_OK| since there is nothing to be read.
                op.cancel_locked(&mut stream_size_guard);
                return Ok(0);
            }

            (seek, (size_limit - seek) as usize)
        };

        let result = transferred(self.vmo.read_user_vector(user_data, offset, length));
        let read_bytes = result.as_ref().copied().unwrap_or(0);
        *seek_guard += read_bytes as u64;

        // Reacquire the lock to commit the operation.
        let mut stream_size_guard = self.stream_size_mgr.lock();
        op.commit_locked(&mut stream_size_guard);

        if read_bytes > 0 { Ok(read_bytes) } else { result }
    }

    /// Reads data from the stream at a specific offset.
    pub fn read_vector_at(
        &self,
        user_data: UserOutIovec,
        offset: zx_off_t,
    ) -> Result<usize, Status> {
        let total_capacity = user_data.get_total_capacity()?;
        if total_capacity == 0 {
            return Ok(0);
        }

        let op = StreamSizeManagerOperation::new(&self.stream_size_mgr);

        let length = {
            let mut stream_size_guard = self.stream_size_mgr.lock();

            let size_limit = self.stream_size_mgr.begin_read_locked(
                &mut stream_size_guard,
                offset.saturating_add(total_capacity as u64),
                &op,
            );
            if size_limit <= offset {
                // Return |ZX_OK| since there is nothing to be read.
                op.cancel_locked(&mut stream_size_guard);
                return Ok(0);
            }

            (size_limit - offset) as usize
        };

        let result = transferred(self.vmo.read_user_vector(user_data, offset, length));
        let read_bytes = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to commit the operation.
        let mut stream_size_guard = self.stream_size_mgr.lock();
        op.commit_locked(&mut stream_size_guard);

        if read_bytes > 0 { Ok(read_bytes) } else { result }
    }

    /// Writes data to the stream using an input iovec array.
    pub fn write_vector(&self, user_data: UserInIovec) -> Result<usize, Status> {
        if self.is_in_append_mode() {
            return self.append_vector(user_data);
        }

        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes since there's nothing to do.
        if total_capacity == 0 {
            return Ok(0);
        }

        let op = StreamSizeManagerOperation::new(&self.stream_size_mgr);

        let mut seek_guard = self.seek.lock();
        let seek = *seek_guard;

        let (length, prev_stream_size) = self.create_write_op(total_capacity, seek, &op)?;

        let result = self.write_vmo(user_data, seek, length as usize, prev_stream_size, &op);
        let written = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to potentially shrink and commit the operation.
        let mut stream_size_guard = self.stream_size_mgr.lock();

        // Update the stream size operation if operation was partially successful.
        if (written as u64) < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(&mut stream_size_guard);
                return result;
            } else {
                op.shrink_size_locked(&mut stream_size_guard, seek + written as u64);
            }
        }

        *seek_guard += written as u64;

        op.commit_locked(&mut stream_size_guard);
        if written > 0 { Ok(written) } else { result }
    }

    /// Writes data to the stream at a specific offset.
    pub fn write_vector_at(
        &self,
        user_data: UserInIovec,
        offset: zx_off_t,
    ) -> Result<usize, Status> {
        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes
        if total_capacity == 0 {
            return Ok(0);
        }

        let op = StreamSizeManagerOperation::new(&self.stream_size_mgr);

        let (length, prev_stream_size) = self.create_write_op(total_capacity, offset, &op)?;

        let result = self.write_vmo(user_data, offset, length as usize, prev_stream_size, &op);
        let written = result.as_ref().copied().unwrap_or(0);

        // Reacquire the lock to potentially shrink and commit the operation.
        let mut stream_size_guard = self.stream_size_mgr.lock();

        // Update the stream size operation if operation was partially successful.
        if (written as u64) < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(&mut stream_size_guard);
                return result;
            } else {
                op.shrink_size_locked(&mut stream_size_guard, offset + written as u64);
            }
        }

        op.commit_locked(&mut stream_size_guard);
        if written > 0 { Ok(written) } else { result }
    }

    /// Appends data to the end of the stream.
    pub fn append_vector(&self, user_data: UserInIovec) -> Result<usize, Status> {
        let total_capacity = user_data.get_total_capacity()?;
        // Return early if writing zero bytes since there's nothing to do.
        if total_capacity == 0 {
            return Ok(0);
        }

        let length: usize;
        let offset: u64;
        let op = StreamSizeManagerOperation::new(&self.stream_size_mgr);
        let mut seek_guard = self.seek.lock();

        // This section expands the VMO if necessary and bumps the |seek_| pointer if successful.
        {
            let mut stream_size_guard = self.stream_size_mgr.lock();

            self.stream_size_mgr.begin_append_locked(
                &mut stream_size_guard,
                total_capacity,
                &op,
            )?;

            let new_stream_size = op.get_size_locked(&stream_size_guard);
            offset = new_stream_size - total_capacity as u64;

            let vmo_size = self.vmo.size();
            if vmo_size <= offset {
                // We can't even perform a partial write
                op.cancel_locked(&mut stream_size_guard);
                return Err(Status::OUT_OF_RANGE);
            }

            if vmo_size < new_stream_size {
                // Unable to expand to requested size but able to perform a partial write.
                op.shrink_size_locked(&mut stream_size_guard, vmo_size);
            }

            length = (core::cmp::min(vmo_size, new_stream_size) - offset) as usize;
        }

        let result = self.write_vmo(user_data, offset, length, Some(offset), &op);
        let written = result.as_ref().copied().unwrap_or(0);
        *seek_guard = offset + written as u64;

        // Reacquire the lock to potentially shrink and commit the operation.
        let mut stream_size_guard = self.stream_size_mgr.lock();

        // Update the stream size operation if operation was partially successful.
        if written < length {
            if written == 0 {
                // Do not commit the operation if nothing was written.
                op.cancel_locked(&mut stream_size_guard);
                return result;
            } else {
                op.shrink_size_locked(&mut stream_size_guard, offset + written as u64);
            }
        }

        op.commit_locked(&mut stream_size_guard);
        if written > 0 { Ok(written) } else { result }
    }

    /// Sets the seek offset of the stream.
    pub fn seek(&self, whence: zx_stream_seek_origin_t, offset: i64) -> Result<zx_off_t, Status> {
        let mut guard = self.seek.lock();
        let current_seek = *guard;
        let target = match whence {
            zx_types::ZX_STREAM_SEEK_ORIGIN_START => {
                if offset < 0 {
                    return Err(Status::INVALID_ARGS);
                }
                offset as u64
            }
            zx_types::ZX_STREAM_SEEK_ORIGIN_CURRENT => {
                let Some(value) = current_seek.checked_add_signed(offset) else {
                    return Err(Status::INVALID_ARGS);
                };
                value
            }
            zx_types::ZX_STREAM_SEEK_ORIGIN_END => {
                let stream_size = self.stream_size_mgr.get_stream_size();
                let Some(value) = stream_size.checked_add_signed(offset) else {
                    return Err(Status::INVALID_ARGS);
                };
                value
            }
            _ => return Err(Status::INVALID_ARGS),
        };
        *guard = target;
        Ok(target)
    }

    /// Sets whether the stream is in append mode.
    pub fn set_append_mode(&self, value: bool) {
        self.options.lock().set_append(value);
    }

    /// Returns whether the stream is in append mode.
    pub fn is_in_append_mode(&self) -> bool {
        self.options.lock().append()
    }

    /// Returns whether the stream can resize the underlying VMO.
    pub fn can_resize_vmo(&self) -> bool {
        self.options.lock().can_resize_vmo()
    }

    /// Returns diagnostic information about the stream.
    pub fn get_info(&self) -> zx_info_stream_t {
        let options_guard = self.options.lock();
        let seek_guard = self.seek.lock();

        zx_info_stream_t {
            options: options_guard.to_stream_modes(),
            padding1: [0; 4],
            seek: *seek_guard,
            // |content_size| is the legacy name for the stream size
            content_size: self.stream_size_mgr.get_stream_size(),
        }
    }
}
