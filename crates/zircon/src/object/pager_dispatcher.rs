//! Pager objects, which provide the pages of the VMOs they create.
//!
//! Ported from `zircon/kernel/object/pager_dispatcher.cc` and
//! `zircon/kernel/object/include/object/pager_dispatcher.h`.
//!
//! Deviation: user buffers are plain pointers into the caller's memory, so copying out cannot
//! fault and there are no faults to resolve before retrying.

use super::dispatcher::{Dispatcher, DispatcherBase};
use super::handle::KernelHandle;
use super::pager_proxy::PagerProxy;
use super::port_dispatcher::PortDispatcher;
use super::rights::ZX_DEFAULT_PAGER_RIGHTS;
use crate::vm::page_source::PageSource;
use crate::vm::vm_object_paged::VmObjectPaged;
use parking_lot::Mutex;
use std::sync::Arc;
use zx_status::Status;
use zx_types::*;

/// A range of dirty content, as `zx_pager_query_dirty_ranges` returns them, from
/// `zircon/system/public/zircon/syscalls-next.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct zx_vmo_dirty_range_t {
    /// Represents the range [offset, offset + length).
    pub offset: u64,
    pub length: u64,
    /// Any options applicable to the range. ZX_VMO_DIRTY_RANGE_IS_ZERO indicates that the range
    /// contains all zeros.
    pub options: u64,
}

/// Statistics about a pager VMO, from `zircon/system/public/zircon/types.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct zx_pager_vmo_stats_t {
    /// Will be set to ZX_PAGER_VMO_STATS_MODIFIED if the VMO was modified, or 0 otherwise. Note
    /// that this can be set to 0 if a previous zx_pager_query_vmo_stats() call specified the
    /// ZX_PAGER_RESET_VMO_STATS option, which resets the modified state.
    pub modified: u32,
}

pub const ZX_PAGER_VMO_STATS_MODIFIED: u32 = 1;
pub const ZX_PAGER_RESET_VMO_STATS: u32 = 1;

struct PagerDispatcherLocked {
    proxies: Vec<Arc<PagerProxy>>,
    /// Tracks whether on_zero_handles has been called, after which no more sources may be created.
    triggered_zero_handles: bool,
}

pub struct PagerDispatcher {
    base: DispatcherBase,
    debug_name: Mutex<[u8; ZX_MAX_NAME_LEN]>,
    lock: Mutex<PagerDispatcherLocked>,
}

impl Dispatcher for PagerDispatcher {
    fn base(&self) -> &DispatcherBase {
        &self.base
    }

    fn get_type(&self) -> zx_obj_type_t {
        ZX_OBJ_TYPE_PAGER
    }

    fn default_rights(&self) -> zx_rights_t {
        ZX_DEFAULT_PAGER_RIGHTS
    }

    fn on_zero_handles(&self) {
        let mut locked = self.lock.lock();
        debug_assert!(!locked.triggered_zero_handles);
        // Set triggered_zero_handles to true before starting to release proxies, so that a racy
        // call to release_proxy knows it's not incorrect to not find the proxy in the list.
        locked.triggered_zero_handles = true;
        while let Some(proxy) = locked.proxies.pop() {
            // Call unlocked to prevent a double-lock if release_proxy is called, and to preserve
            // the lock order that PagerProxy locks are acquired before the list lock.
            parking_lot::MutexGuard::unlocked(&mut locked, || proxy.on_dispatcher_close());
        }
    }
}

impl PagerDispatcher {
    pub fn create() -> Result<(KernelHandle<PagerDispatcher>, zx_rights_t), Status> {
        let pager = Arc::new(PagerDispatcher {
            base: DispatcherBase::new(0),
            debug_name: Mutex::new([0; ZX_MAX_NAME_LEN]),
            lock: Mutex::new(PagerDispatcherLocked {
                proxies: Vec::new(),
                triggered_zero_handles: false,
            }),
        });
        Ok((KernelHandle::new(pager), ZX_DEFAULT_PAGER_RIGHTS))
    }

    pub fn get_koid(&self) -> zx_koid_t {
        self.base.get_koid()
    }

    pub fn debug_name(&self) -> [u8; ZX_MAX_NAME_LEN] {
        *self.debug_name.lock()
    }

    pub fn set_debug_name(&self, name: &[u8]) {
        let mut stored = [0; ZX_MAX_NAME_LEN];
        let len = name.len().min(ZX_MAX_NAME_LEN - 1);
        stored[..len].copy_from_slice(&name[..len]);
        *self.debug_name.lock() = stored;
    }

    /// Creates a page source whose requests are delivered to `port` with `key`.
    pub fn create_source(
        self: &Arc<Self>,
        port: Arc<PortDispatcher>,
        key: u64,
        mut options: u32,
    ) -> Result<Arc<PageSource>, Status> {
        let mut locked = self.lock.lock();
        // Make sure on_zero_handles has not been called. This could happen if a call to
        // pager_create_vmo races with closing the last handle, as pager_create_vmo does not hold
        // the handle table lock over this operation.
        if locked.triggered_zero_handles {
            return Err(Status::BAD_STATE);
        }
        // Process any options relevant to creation of the PagerProxy.
        let mut proxy_options = 0;
        if options & ZX_VMO_TRAP_DIRTY != 0 {
            proxy_options = PagerProxy::TRAP_DIRTY;
            options &= !ZX_VMO_TRAP_DIRTY;
        }
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        // We are going to setup two objects that both need to point to each other. As such one of
        // the pointers must be bound 'late' and not in the constructor.
        let proxy = PagerProxy::new(self, port, key, proxy_options);
        let src = PageSource::new(proxy.clone());
        // Setting this creates a reference cycle between the PagerProxy and PageSource, however we
        // guarantee we will call proxy.on_dispatcher_close at some point to break the cycle.
        proxy.set_page_source_unchecked(src.clone());
        locked.proxies.push(proxy);
        Ok(src)
    }

    /// Removes `proxy` from the list of proxies, returning the list's reference to it.
    pub fn release_proxy(&self, proxy: &PagerProxy) -> Option<Arc<PagerProxy>> {
        let mut locked = self.lock.lock();
        // proxy might not be in the container since we could be racing with a call to
        // on_zero_handles, but that should only happen if we have triggered_zero_handles.
        let index = locked.proxies.iter().position(|p| std::ptr::eq(Arc::as_ptr(p), proxy));
        debug_assert!(index.is_some() || locked.triggered_zero_handles);
        index.map(|index| locked.proxies.swap_remove(index))
    }

    pub fn range_op(
        &self,
        op: u32,
        vmo: &VmObjectPaged,
        offset: u64,
        length: u64,
        data: u64,
    ) -> Result<(), Status> {
        match op {
            ZX_PAGER_OP_FAIL => {
                let signed_data = data as i64;
                if signed_data < i32::MIN as i64 || signed_data > i32::MAX as i64 {
                    return Err(Status::INVALID_ARGS);
                }
                let error_status = Status::try_from_raw(data as i32)
                    .filter(|status| PageSource::is_valid_external_failure_code(*status))
                    .ok_or(Status::INVALID_ARGS)?;
                vmo.fail_page_requests(offset, length, error_status)
            }
            ZX_PAGER_OP_DIRTY => {
                if data != 0 {
                    return Err(Status::INVALID_ARGS);
                }
                vmo.dirty_pages(offset, length)
            }
            ZX_PAGER_OP_WRITEBACK_BEGIN => {
                if data != 0 && data != ZX_VMO_DIRTY_RANGE_IS_ZERO {
                    return Err(Status::INVALID_ARGS);
                }
                vmo.writeback_begin(offset, length, data == ZX_VMO_DIRTY_RANGE_IS_ZERO)
            }
            ZX_PAGER_OP_WRITEBACK_END => {
                if data != 0 {
                    return Err(Status::INVALID_ARGS);
                }
                vmo.writeback_end(offset, length)
            }
            _ => Err(Status::NOT_SUPPORTED),
        }
    }

    /// Writes the dirty ranges of [offset, offset + length) to `buffer`, returning how many were
    /// written and, if `compute_total`, how many there are in total.
    ///
    /// # Safety
    ///
    /// `buffer` must be valid for writes of `buffer_size` bytes.
    pub unsafe fn query_dirty_ranges(
        &self,
        vmo: &VmObjectPaged,
        offset: u64,
        length: u64,
        buffer: *mut u8,
        buffer_size: usize,
        compute_total: bool,
    ) -> Result<(usize, usize), Status> {
        // Index into `buffer`, used to populate its entries.
        let mut index = 0usize;
        // Total number of dirty ranges discovered.
        let mut total = 0usize;
        let buffer = buffer.cast::<zx_vmo_dirty_range_t>();
        let buffer_full =
            |index: usize| (index + 1) * size_of::<zx_vmo_dirty_range_t>() > buffer_size;

        // Enumeration function that will be invoked on each dirty range found.
        vmo.enumerate_dirty_ranges(offset, length, |range_offset, range_len, range_is_zero| {
            // No more space in the buffer.
            if buffer_full(index) {
                // If we were not asked to compute the total, we can end termination early as there
                // is nothing more to copy out. Although we would have terminated at the bottom of
                // this loop if out of space, this could be our first iteration and so this check is
                // still needed.
                if !compute_total {
                    debug_assert_eq!(index, 0);
                    return Status::STOP;
                }
                // As there is no more space in the buffer, only update the total without trying to
                // copy out any more ranges.
                total += 1;
                return Status::NEXT;
            }
            let dirty_range = zx_vmo_dirty_range_t {
                offset: range_offset,
                length: range_len,
                options: if range_is_zero { ZX_VMO_DIRTY_RANGE_IS_ZERO } else { 0 },
            };
            if buffer.is_null() {
                return Status::INVALID_ARGS;
            }
            // SAFETY: The entry is within `buffer_size`, guaranteed by the caller.
            unsafe { buffer.add(index).write_unaligned(dirty_range) };
            // We were able to successfully copy out this dirty range. Advance the index and
            // continue with the enumeration if we need to consider more ranges.
            index += 1;
            total += 1;
            if !compute_total && buffer_full(index) {
                // No need to compute the total and the buffer is full, so can cease considering
                // additional ranges.
                return Status::STOP;
            }
            Status::NEXT
        })?;
        debug_assert!(total >= index);
        Ok((index, total))
    }

    /// Returns the pager stats of `vmo`, resetting them if asked to by `options`.
    pub fn query_pager_vmo_stats(
        &self,
        vmo: &VmObjectPaged,
        mut options: u32,
        buffer_size: usize,
    ) -> Result<zx_pager_vmo_stats_t, Status> {
        if buffer_size < size_of::<zx_pager_vmo_stats_t>() {
            return Err(Status::BUFFER_TOO_SMALL);
        }
        let reset = options & ZX_PAGER_RESET_VMO_STATS != 0;
        options &= !ZX_PAGER_RESET_VMO_STATS;
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        let stats = vmo.query_pager_vmo_stats(reset)?;
        Ok(zx_pager_vmo_stats_t {
            modified: if stats.modified { ZX_PAGER_VMO_STATS_MODIFIED } else { 0 },
        })
    }
}
