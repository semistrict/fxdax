//! VMO objects.
//!
//! Ported from `zircon/kernel/object/vm_object_dispatcher.cc`,
//! `zircon/kernel/object/vm_object_dispatcher.rs` and
//! `zircon/kernel/object/include/object/vm_object_dispatcher.h`.
//!
//! Deviations:
//! - The `StreamSizeManager` is not shared with the VMO, as there are no mappings that fault beyond
//!   the stream size (`ZX_VM_FAULT_BEYOND_STREAM_SIZE`), so setting the stream size doesn't unmap
//!   the pages beyond it.
//! - No children, so `ZX_VMO_ZERO_CHILDREN` stays asserted.
//! - No lock, try lock or unlock operations, as there are no discardable VMOs.

use super::dispatcher::{Dispatcher, DispatcherBase};
use super::handle::KernelHandle;
use super::rights::ZX_DEFAULT_VMO_RIGHTS;
use crate::boot_options::BootOptions;
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::vm::stream_size_manager::{
    Operation as StreamSizeManagerOperation, StreamSizeGuard, StreamSizeManager,
};
use crate::vm::vm_object_paged::{EvictionHint, VmObjectPaged, VmObjectReadWriteOptions};
use parking_lot::Mutex;
use std::sync::Arc;
use zx_status::Status;
use zx_types::*;

const PAGE_SIZE: u64 = page::SIZE as u64;

/// Specifies initial mutability for `VmObjectDispatcher`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitialMutability {
    Mutable,
    Immutable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateStats {
    pub flags: u32,
    pub size: u64,
}

pub struct VmObjectDispatcher {
    base: DispatcherBase,
    vmo: Arc<VmObjectPaged>,
    /// Created when the stream size first differs from the VMO size or a stream is created.
    stream_size_mgr: Mutex<Option<Arc<StreamSizeManager>>>,
    initial_mutability: InitialMutability,
}

impl Dispatcher for VmObjectDispatcher {
    fn base(&self) -> &DispatcherBase {
        &self.base
    }

    fn get_type(&self) -> zx_obj_type_t {
        ZX_OBJ_TYPE_VMO
    }

    fn default_rights(&self) -> zx_rights_t {
        ZX_DEFAULT_VMO_RIGHTS
    }

    fn get_name(&self) -> Result<[u8; ZX_MAX_NAME_LEN], Status> {
        Ok(self.vmo.get_name())
    }

    fn set_name(&self, name: &[u8]) -> Result<(), Status> {
        self.vmo.set_name(name);
        Ok(())
    }
}

fn round_up_page_size(val: u64) -> Option<u64> {
    val.checked_next_multiple_of(PAGE_SIZE)
}

impl VmObjectDispatcher {
    /// Parses create syscall flags for VMOs.
    ///
    /// Rounds up the size to the nearest page, or sets the size to the maximum possible VMO size
    /// if `ZX_VMO_UNBOUNDED` is used.
    pub fn parse_create_syscall_flags(mut flags: u32, size: u64) -> Result<CreateStats, Status> {
        let mut res = CreateStats { flags: 0, size };
        if (flags & ZX_VMO_RESIZABLE) != 0 {
            if (flags & ZX_VMO_UNBOUNDED) != 0 {
                return Err(Status::INVALID_ARGS);
            }
            res.flags |= VmObjectPaged::RESIZABLE;
            flags &= !ZX_VMO_RESIZABLE;
        }
        if (flags & ZX_VMO_DISCARDABLE) != 0 {
            res.flags |= VmObjectPaged::DISCARDABLE;
            flags &= !ZX_VMO_DISCARDABLE;
        }
        if (flags & ZX_VMO_UNBOUNDED) != 0 {
            flags &= !ZX_VMO_UNBOUNDED;
            res.size = VmObjectPaged::MAX_SIZE;
        } else {
            if size > VmObjectPaged::MAX_SIZE {
                return Err(Status::OUT_OF_RANGE);
            }
            res.size = round_up_page_size(size).ok_or(Status::OUT_OF_RANGE)?;
        }
        if flags != 0 {
            return Err(Status::INVALID_ARGS);
        }
        // The initial stream size should not end up larger than the vmo size, as this is a state
        // that cannot exist.
        if size > res.size {
            return Err(Status::OUT_OF_RANGE);
        }
        Ok(res)
    }

    /// Creates a `VmObjectDispatcher` wrapping a VMO.
    pub fn create(
        vmo: Arc<VmObjectPaged>,
        stream_size: u64,
        initial_mutability: InitialMutability,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let vmo_size = vmo.size();
        let mut ssm = None;
        if stream_size != vmo_size && vmo.is_stream_compatible() {
            debug_assert!(stream_size <= vmo_size);
            ssm = Some(StreamSizeManager::create(stream_size));
            let aligned_stream_size =
                round_up_page_size(stream_size).ok_or(Status::OUT_OF_RANGE)?;
            if aligned_stream_size < vmo_size {
                vmo.zero_range_untracked(aligned_stream_size, vmo_size - aligned_stream_size)?;
            }
        }
        let dispatcher = Arc::new(Self {
            base: DispatcherBase::new(ZX_VMO_ZERO_CHILDREN),
            vmo,
            stream_size_mgr: Mutex::new(ssm),
            initial_mutability,
        });
        dispatcher.vmo.set_user_id(dispatcher.base.get_koid());
        let rights =
            ZX_DEFAULT_VMO_RIGHTS | if dispatcher.vmo.is_resizable() { ZX_RIGHT_RESIZE } else { 0 };
        Ok((KernelHandle::new(dispatcher), rights))
    }

    pub fn vmo(&self) -> &Arc<VmObjectPaged> {
        &self.vmo
    }

    /// The koid of the pager this VMO's pages come from, or `ZX_KOID_INVALID`.
    pub fn pager_koid(&self) -> zx_koid_t {
        self.vmo.get_page_source_koid().unwrap_or(ZX_KOID_INVALID)
    }

    /// Reads into user memory, returning the status and how much was read.
    pub fn read(
        &self,
        user_data: UserOutPtr<u8>,
        offset: u64,
        length: usize,
    ) -> (Result<(), Status>, usize) {
        self.vmo.read_user(user_data, offset, length, VmObjectReadWriteOptions::None)
    }

    /// Writes from user memory, returning the status and how much was written.
    pub fn write(
        &self,
        user_data: UserInPtr<u8>,
        offset: u64,
        length: usize,
    ) -> (Result<(), Status>, usize) {
        self.vmo.write_user(user_data, offset, length, VmObjectReadWriteOptions::None, None)
    }

    pub fn get_size(&self) -> u64 {
        self.vmo.size()
    }

    /// Sets the size of the VMO, and the stream size to match.
    pub fn set_size(&self, size: u64) -> Result<(), Status> {
        if !self.vmo.is_stream_compatible() {
            debug_assert!(!self.vmo.is_resizable());
            return Err(Status::UNAVAILABLE);
        }

        // TODO(https://fxbug.dev/341218975) SetSize should only change stream size to maintain
        // invariant that stream size isn't larger than VMO size.

        let ssm = self.stream_size_manager();
        self.set_size_with_ssm(&ssm, size)
    }

    /// The stream size manager, created if the VMO has none yet.
    pub fn stream_size_manager(&self) -> Arc<StreamSizeManager> {
        self.stream_size_mgr
            .lock()
            .get_or_insert_with(|| StreamSizeManager::create(self.vmo.size()))
            .clone()
    }

    pub fn get_stream_size(&self) -> u64 {
        // Stream size is always reported as 0 if the VMO doesn't support streams.
        if !self.vmo.is_stream_compatible() {
            return 0;
        }

        // Retrieving the stream size needs to be a non-fallible operation, so we avoid allocating
        // one if it doesn't exist.
        let ssm = self.stream_size_mgr.lock().clone();
        match ssm {
            Some(ssm) => ssm.get_stream_size(),
            None => self.vmo.size(),
        }
    }

    /// Sets the stream size, zeroing the content beyond it.
    pub fn set_stream_size(&self, stream_size: u64) -> Result<(), Status> {
        if !self.vmo.is_stream_compatible() {
            return Err(Status::NOT_SUPPORTED);
        }

        let ssm = self.stream_size_manager();
        self.set_stream_size_with_ssm(&ssm, stream_size)
    }

    /// Sets the stream size of the underlying VMO, coordinating with `StreamSizeManager`.
    pub fn set_stream_size_with_ssm(
        &self,
        ssm: &StreamSizeManager,
        target_size: u64,
    ) -> Result<(), Status> {
        let paged = &self.vmo;

        let op = StreamSizeManagerOperation::new(ssm);
        let mut guard = ssm.lock();

        let vmo_size = paged.size();
        let old_stream_size = ssm.get_stream_size();

        if target_size == old_stream_size {
            return Ok(());
        }

        // Can't resize the stream beyond the VMO size.
        if target_size > vmo_size {
            return Err(Status::OUT_OF_RANGE);
        }

        ssm.begin_set_stream_size_locked(&mut guard, target_size, &op);

        // Zero the range from min(target size, old stream size) to the end of the VMO.
        let zero_start = core::cmp::min(target_size, old_stream_size);
        let Some(aligned_stream_size) = round_up_page_size(target_size) else {
            op.cancel_locked(&mut guard);
            return Err(Status::OUT_OF_RANGE);
        };
        debug_assert!(aligned_stream_size >= target_size);

        // Dropping the lock here is fine, as an Operation only needs to be locked when
        // initializing, committing, or cancelling.
        let res = StreamSizeGuard::unlocked(&mut guard, || {
            paged.zero_range(zero_start, aligned_stream_size - zero_start)?;
            paged.zero_range_untracked(aligned_stream_size, vmo_size - aligned_stream_size)
        });

        if let Err(status) = res {
            op.cancel_locked(&mut guard);
            return Err(status);
        }

        op.commit_locked(&mut guard);

        Ok(())
    }

    /// Sets the size of the underlying VMO, coordinating with `StreamSizeManager`.
    pub fn set_size_with_ssm(&self, ssm: &StreamSizeManager, size: u64) -> Result<(), Status> {
        let paged = &self.vmo;

        let op = StreamSizeManagerOperation::new(ssm);
        let mut guard = ssm.lock();

        ssm.begin_set_stream_size_locked(&mut guard, size, &op);

        let Some(size_aligned) = round_up_page_size(size) else {
            op.cancel_locked(&mut guard);
            return Err(Status::OUT_OF_RANGE);
        };

        if let Err(status) = paged.resize(size_aligned) {
            op.cancel_locked(&mut guard);
            return Err(status);
        }

        let remaining = size_aligned - size;
        if remaining > 0 {
            // TODO(https://fxbug.dev/42053728): Determine whether failure to ZeroRange here should
            // undo this operation.
            //
            // Dropping the lock here is fine, as an Operation only needs to be locked when
            // initializing, committing, or cancelling.
            let _ = StreamSizeGuard::unlocked(&mut guard, || paged.zero_range(size, remaining));
        }

        op.commit_locked(&mut guard);
        Ok(())
    }

    /// Returns information about this VMO.
    pub fn get_vmo_info(&self, rights: zx_rights_t) -> zx_info_vmo_t {
        let vmo = &self.vmo;
        let committed_bytes = vmo.committed_bytes();
        let mut info = zx_info_vmo_t::default();
        info.koid = vmo.user_id();
        info.name = vmo.get_name();
        info.size_bytes = vmo.size();
        info.num_mappings = vmo.num_mappings() as usize;
        info.share_count = 1;
        info.flags = ZX_INFO_VMO_TYPE_PAGED
            | if vmo.is_resizable() { ZX_INFO_VMO_RESIZABLE } else { 0 }
            | if vmo.is_discardable() { ZX_INFO_VMO_DISCARDABLE } else { 0 }
            | if vmo.is_user_pager_backed() { ZX_INFO_VMO_PAGER_BACKED } else { 0 }
            | if vmo.is_contiguous() { ZX_INFO_VMO_CONTIGUOUS } else { 0 }
            | ZX_INFO_VMO_VIA_HANDLE;
        info.committed_bytes = committed_bytes;
        info.handle_rights = rights;
        info.cache_policy = ZX_CACHE_POLICY_CACHED;
        info.committed_change_events = vmo.reclamation_event_count();
        info.populated_bytes = committed_bytes;
        info.committed_private_bytes = committed_bytes;
        info.populated_private_bytes = committed_bytes;
        info.committed_scaled_bytes = committed_bytes;
        info.populated_scaled_bytes = committed_bytes;
        if self.initial_mutability == InitialMutability::Immutable {
            info.flags |= ZX_INFO_VMO_IMMUTABLE;
        }
        info
    }

    /// Performs an operation on a range of the VMO.
    pub fn range_op(
        &self,
        op: u32,
        offset: u64,
        size: u64,
        rights: zx_rights_t,
    ) -> Result<(), Status> {
        match op {
            ZX_VMO_OP_COMMIT => {
                if (rights & ZX_RIGHT_WRITE) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.commit_range(offset, size)
            }
            ZX_VMO_OP_DECOMMIT => {
                if (rights & ZX_RIGHT_WRITE) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.decommit_range(offset, size)
            }
            ZX_VMO_OP_LOCK | ZX_VMO_OP_TRY_LOCK | ZX_VMO_OP_UNLOCK => {
                if (rights & (ZX_RIGHT_READ | ZX_RIGHT_WRITE)) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                // Only discardable VMOs can be locked, and they are not supported.
                Err(Status::NOT_SUPPORTED)
            }
            ZX_VMO_OP_CACHE_SYNC | ZX_VMO_OP_CACHE_CLEAN | ZX_VMO_OP_CACHE_CLEAN_INVALIDATE => {
                if (rights & ZX_RIGHT_READ) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.cache_op(offset, size)
            }
            ZX_VMO_OP_CACHE_INVALIDATE => {
                if !BootOptions::get().enable_debugging_syscalls {
                    return Err(Status::NOT_SUPPORTED);
                }
                if (rights & ZX_RIGHT_WRITE) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.cache_op(offset, size)
            }
            ZX_VMO_OP_ZERO => {
                if (rights & ZX_RIGHT_WRITE) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.zero_range(offset, size)
            }
            ZX_VMO_OP_ALWAYS_NEED => self.vmo.hint_range(offset, size, EvictionHint::AlwaysNeed),
            ZX_VMO_OP_DONT_NEED => self.vmo.hint_range(offset, size, EvictionHint::DontNeed),
            ZX_VMO_OP_PREFETCH => {
                if (rights & ZX_RIGHT_READ) == 0 {
                    return Err(Status::ACCESS_DENIED);
                }
                self.vmo.prefetch_range(offset, size)
            }
            _ => Err(Status::INVALID_ARGS),
        }
    }
}
