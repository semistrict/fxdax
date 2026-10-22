// Copyright 2018 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pager objects, which provide the pages of the VMOs they create.
//!
//! Ported from `zircon/kernel/object/pager_dispatcher.cc` and
//! `zircon/kernel/object/include/object/pager_dispatcher.h`.
//!
//! Deviations:
//! - fxdax: a pager's proxies are kept by their addresses, rather than in an intrusive list, so
//!   that letting go of one of many is quick (`PagerDispatcherLocked::proxies`).

use super::dispatcher::{Dispatcher, DispatcherBase, solo_dispatcher};
use super::handle::KernelHandle;
use super::pager_proxy::PagerProxy;
use super::port_dispatcher::PortDispatcher;
use super::rights::ZX_DEFAULT_PAGER_RIGHTS;
use crate::arch::host::user_copy::FaultInfo;
use crate::ensure;
use crate::kernel::thread::Current;
use crate::user_copy::UserOutPtr;
use crate::vm::page_source::PageSource;
use crate::vm::vm_object_paged::VmObjectPaged;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;
use zerocopy::{Immutable, IntoBytes};
use zx_status::Status;
use zx_types::*;

/// A range of dirty content, as `zx_pager_query_dirty_ranges` returns them, from
/// `zircon/system/public/zircon/syscalls-next.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Immutable, IntoBytes)]
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
#[derive(Clone, Copy, Debug, Default, Immutable, IntoBytes)]
pub struct zx_pager_vmo_stats_t {
    /// Will be set to ZX_PAGER_VMO_STATS_MODIFIED if the VMO was modified, or 0 otherwise. Note
    /// that this can be set to 0 if a previous zx_pager_query_vmo_stats() call specified the
    /// ZX_PAGER_RESET_VMO_STATS option, which resets the modified state.
    pub modified: u32,
}

pub const ZX_PAGER_VMO_STATS_MODIFIED: u32 = 1;
pub const ZX_PAGER_RESET_VMO_STATS: u32 = 1;

struct PagerDispatcherLocked {
    /// The proxies of the pager's VMOs. Zircon keeps them in an intrusive list. fxdax: by their
    /// addresses, so that one of many is let go of at once, on the thread reading the port that
    /// delivers every page request of the pager's VMOs: the extent cache's pager has a VMO for each
    /// extent it caches (TASK-92.34).
    proxies: BTreeMap<usize, Arc<PagerProxy>>,
    /// Tracks whether on_zero_handles has been called, after which no more sources may be created.
    triggered_zero_handles: bool,
}

/// A proxy's key among its pager's: its address, which is its own while it lives.
fn proxy_key(proxy: &PagerProxy) -> usize {
    std::ptr::from_ref(proxy) as usize
}

pub struct PagerDispatcher {
    base: DispatcherBase,
    debug_name: Mutex<[u8; ZX_MAX_NAME_LEN]>,
    lock: Mutex<PagerDispatcherLocked>,
}

solo_dispatcher!(PagerDispatcher, ZX_OBJ_TYPE_PAGER, ZX_DEFAULT_PAGER_RIGHTS);

impl Dispatcher for PagerDispatcher {
    fn on_zero_handles(&self) {
        let mut locked = self.lock.lock();
        debug_assert!(!locked.triggered_zero_handles);
        // Set triggered_zero_handles to true before starting to release proxies, so that a racy
        // call to release_proxy knows it's not incorrect to not find the proxy in the list.
        locked.triggered_zero_handles = true;
        while let Some((_, proxy)) = locked.proxies.pop_first() {
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
                proxies: BTreeMap::new(),
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
        ensure!(!locked.triggered_zero_handles, Status::BAD_STATE);
        // Process any options relevant to creation of the PagerProxy.
        let mut proxy_options = 0;
        if options & ZX_VMO_TRAP_DIRTY != 0 {
            proxy_options = PagerProxy::TRAP_DIRTY;
            options &= !ZX_VMO_TRAP_DIRTY;
        }
        ensure!(options == 0, Status::INVALID_ARGS);
        // We are going to setup two objects that both need to point to each other. As such one of
        // the pointers must be bound 'late' and not in the constructor.
        let proxy = PagerProxy::new(self, port, key, proxy_options);
        let src = PageSource::new(proxy.clone());
        // Setting this creates a reference cycle between the PagerProxy and PageSource, however we
        // guarantee we will call proxy.on_dispatcher_close at some point to break the cycle.
        proxy.set_page_source_unchecked(src.clone());
        let added = locked.proxies.insert(proxy_key(&proxy), proxy);
        debug_assert!(added.is_none());
        Ok(src)
    }

    /// Removes `proxy` from the list of proxies, returning the list's reference to it.
    pub fn release_proxy(&self, proxy: &PagerProxy) -> Option<Arc<PagerProxy>> {
        let mut locked = self.lock.lock();
        // proxy might not be in the container since we could be racing with a call to
        // on_zero_handles, but that should only happen if we have triggered_zero_handles.
        let released = locked.proxies.remove(&proxy_key(proxy));
        debug_assert!(released.is_some() || locked.triggered_zero_handles);
        released
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
                ensure!(
                    signed_data >= i32::MIN as i64 && signed_data <= i32::MAX as i64,
                    Status::INVALID_ARGS
                );
                let error_status = Status::try_from_raw(data as i32)
                    .filter(|status| PageSource::is_valid_external_failure_code(*status))
                    .ok_or(Status::INVALID_ARGS)?;
                vmo.fail_page_requests(offset, length, error_status)
            }
            ZX_PAGER_OP_DIRTY => {
                ensure!(data == 0, Status::INVALID_ARGS);
                vmo.dirty_pages(offset, length)
            }
            ZX_PAGER_OP_WRITEBACK_BEGIN => {
                ensure!(data == 0 || data == ZX_VMO_DIRTY_RANGE_IS_ZERO, Status::INVALID_ARGS);
                vmo.writeback_begin(offset, length, data == ZX_VMO_DIRTY_RANGE_IS_ZERO)
            }
            ZX_PAGER_OP_WRITEBACK_END => {
                ensure!(data == 0, Status::INVALID_ARGS);
                vmo.writeback_end(offset, length)
            }
            _ => Err(Status::NOT_SUPPORTED),
        }
    }

    /// Writes the dirty ranges of [offset, offset + length) to `buffer`, and how many were written
    /// and how many there are in total to `actual` and `avail`.
    #[allow(clippy::too_many_arguments)]
    pub fn query_dirty_ranges(
        &self,
        vmo: &VmObjectPaged,
        offset: u64,
        length: u64,
        buffer: UserOutPtr<u8>,
        buffer_size: usize,
        actual: UserOutPtr<usize>,
        avail: UserOutPtr<usize>,
    ) -> Result<(), Status> {
        // State captured by `copy_to_buffer` below.
        struct CopyToBufferInfo {
            // Index into `buffer`, used to populate its entries.
            index: usize,
            // Total number of dirty ranges discovered.
            total: usize,
            // Whether the total number of ranges need to be computed, depending on whether
            // `avail` is supplied.
            compute_total: bool,
            // The range that enumeration runs over. Might get updated when enumeration ends early
            // due to a page fault.
            offset: u64,
            length: u64,
            // The buffer to copy out ranges to.
            buffer: UserOutPtr<zx_vmo_dirty_range_t>,
            buffer_size: usize,
            // State that will get populated if the user copy in the enumeration function
            // encounters a page fault.
            fault_info: Option<FaultInfo>,
        }
        let mut info = CopyToBufferInfo {
            index: 0,
            total: 0,
            compute_total: !avail.is_null(),
            offset,
            length,
            buffer: buffer.reinterpret::<zx_vmo_dirty_range_t>(),
            buffer_size,
            fault_info: None,
        };

        // Enumerate dirty ranges with `copy_to_buffer`. If page faults are captured, resolve them
        // and retry enumeration.
        loop {
            let (offset, length) = (info.offset, info.length);
            // Enumeration function that will be invoked on each dirty range found.
            let copy_to_buffer = |range_offset: u64, range_len: u64, range_is_zero: bool| {
                let buffer_full = |info: &CopyToBufferInfo| {
                    (info.index + 1) * size_of::<zx_vmo_dirty_range_t>() > info.buffer_size
                };
                // No more space in the buffer.
                if buffer_full(&info) {
                    // If we were not asked to compute the total, we can end termination early as
                    // there is nothing more to copy out. Although we would have terminated at the
                    // bottom of this loop if out of space, this could be our first iteration and so
                    // this check is still needed.
                    if !info.compute_total {
                        debug_assert_eq!(info.index, 0);
                        return Status::STOP;
                    }
                    // As there is no more space in the `buffer`, only update the total without
                    // trying to copy out any more ranges.
                    info.total += 1;
                    return Status::NEXT;
                }

                let dirty_range = zx_vmo_dirty_range_t {
                    offset: range_offset,
                    length: range_len,
                    options: if range_is_zero { ZX_VMO_DIRTY_RANGE_IS_ZERO } else { 0 },
                };

                let copy_result = info
                    .buffer
                    .element_offset(info.index)
                    .copy_to_user_capture_faults(&dirty_range);
                // Stash fault information if a fault is encountered. Return early from enumeration
                // with ZX_ERR_SHOULD_WAIT so that the page fault can be resolved.
                if let Err(error) = copy_result {
                    let Some(fault_info) = error.fault_info else {
                        return error.status;
                    };
                    info.fault_info = Some(fault_info);

                    // Update the offset and length to skip over the range that we've already
                    // processed dirty ranges for, to allow forward progress of the syscall.
                    let processed = range_offset - info.offset;
                    info.offset += processed;
                    info.length -= processed;

                    return Status::SHOULD_WAIT;
                }
                // We were able to successfully copy out this dirty range. Advance the index and
                // continue with the enumeration if we need to consider more ranges.
                info.index += 1;
                info.total += 1;
                if !info.compute_total && buffer_full(&info) {
                    // No need to compute the total and the buffer is full, so can cease
                    // considering additional ranges. This is equivalent to the start the loop, but
                    // by doing it here we save the need to calculate the next range before
                    // noticing the buffer is full and terminating.
                    return Status::STOP;
                }
                Status::NEXT
            };
            match vmo.enumerate_dirty_ranges(offset, length, copy_to_buffer) {
                Ok(()) => break,
                // Per `copy_to_buffer`, enumeration will terminate early with ZX_ERR_SHOULD_WAIT if
                // a fault is captured. Resolve the fault and then attempt the enumeration again.
                Err(Status::SHOULD_WAIT) => {
                    // Take `fault_info` so that a future page fault can set it again.
                    let fault = info.fault_info.take().expect("captured fault");
                    ensure!(
                        Current::soft_fault(fault.pf_va, fault.pf_flags).is_ok(),
                        Status::INVALID_ARGS
                    );
                }
                // Another error was encountered. Return.
                Err(status) => return Err(status),
            }
        }

        // Now try to copy out the total and actual number of ranges we populated in `buffer`. We
        // don't need to use copy_to_user_capture_faults() here; we don't hold any locks that need
        // to be dropped before handling a fault.
        if !actual.is_null() {
            actual.copy_to_user(&info.index)?;
        }
        if !avail.is_null() {
            debug_assert!(info.total >= info.index);
            avail.copy_to_user(&info.total)?;
        }
        Ok(())
    }

    /// Writes the pager stats of `vmo` to `buffer`, resetting them if asked to by `options`.
    pub fn query_pager_vmo_stats(
        &self,
        vmo: &VmObjectPaged,
        mut options: u32,
        buffer: UserOutPtr<u8>,
        buffer_size: usize,
    ) -> Result<(), Status> {
        ensure!(buffer_size >= size_of::<zx_pager_vmo_stats_t>(), Status::BUFFER_TOO_SMALL);
        let reset = options & ZX_PAGER_RESET_VMO_STATS != 0;
        options &= !ZX_PAGER_RESET_VMO_STATS;
        ensure!(options == 0, Status::INVALID_ARGS);
        let stats = vmo.query_pager_vmo_stats(reset)?;
        let stats = zx_pager_vmo_stats_t {
            modified: if stats.modified { ZX_PAGER_VMO_STATS_MODIFIED } else { 0 },
        };

        while let Err(error) =
            buffer.reinterpret::<zx_pager_vmo_stats_t>().copy_to_user_capture_faults(&stats)
        {
            let Some(fault_info) = error.fault_info else {
                return Err(error.status);
            };
            ensure!(
                Current::soft_fault(fault_info.pf_va, fault_info.pf_flags).is_ok(),
                Status::INVALID_ARGS
            );
        }

        Ok(())
    }
}
