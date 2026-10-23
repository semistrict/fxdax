// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The main VM object type, based on a copy-on-write set of pages.
//!
//! Ported from `zircon/kernel/vm/vm_object_paged.cc`, `zircon/kernel/vm/include/vm/vm_object_paged.h`
//! and the parts of `zircon/kernel/vm/include/vm/vm_object.h` that paged VMOs use.
//!
//! Deviations:
//! - `VmObjectPaged` is the only VM object type, so `VmObject`'s state (name, user id) lives here.
//! - No children: clones, slices and references are not supported, so the object always covers
//!   all of its `VmCowPages` and has no reference list or parent.
//! - The VMO's mappings are kept with the `VmCowPages` lock, which is the VMO's lock in Zircon too;
//!   see `VmCowPagesLocked::add_mapping`.
//! - No contiguous, discardable or always pinned VMOs, or cache policies. Cache operations only
//!   check their range.
//! - Copies into and out of VMOs are plain memory copies: the kernel shares the address space of
//!   its caller, so there are no user copy faults to resolve.
//! - Reads and writes do not yield the lock to contending threads every 16 pages.
//! - Reads and writes of host memory (`read_vectored_async`, `write_vectored_async`) and zeroing
//!   (`zero_range_async`) may be futures that wait on page requests and faults, rather than
//!   blocking their thread (see `kernel::wakers`). Each runs the same steps as its synchronous
//!   counterpart between waits (`read_write_step`, `zero_range_step`).
//! - Beyond Zircon, `supply_shared_pages` supplies a pager-backed VMO with pages it shares with
//!   another, and `borrowed_bytes` counts them; see `VmCowPages::lend_pages`.
//! - fxdax: dirtying pages (`dirty_pages`) whose wait for a page fails resolves their DIRTY
//!   requests, or fails them, as dirtying them otherwise failing does, rather than leave them
//!   outstanding (`VmCowPages::dirty_pages_failed`, TASK-79.75).

use crate::arch::host::user_copy::{FaultInfo, UserCopyCaptureFaultsError};
use crate::ensure;
use crate::kernel::range_check::{in_range, trim_range};
use crate::kernel::thread::Current;
use crate::kernel::types::PAddr;
use crate::user_copy::{HostBuffer, UserInIovec, UserInPtr, UserOutIovec, UserOutPtr};
use crate::vm::anonymous_page_request::AnonymousPageRequest;
use crate::vm::fault::flag;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_source::{MultiPageRequest, PageSource, VmoDebugInfo};
use crate::vm::physmap::paddr_to_physmap;
use crate::vm::pmm::ALLOC_FLAG_CAN_WAIT;
use crate::vm::vm_cow_pages::{
    DeferredOps, EvictionAction, LookupCursor, RequireResult, SupplyOptions, VmCowPages,
    VmCowPagesLocked, VmCowRange, options as cow_options,
};
use crate::vm::vm_page_list::{VmPageList, VmPageSpliceList};
use parking_lot::Mutex;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use zx_status::Status;
use zx_types::{ZX_MAX_NAME_LEN, zx_koid_t};

const PAGE_SIZE: u64 = page::SIZE as u64;

/// Options for reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmObjectReadWriteOptions {
    None,
    /// Trims the operation to the size of the VMO rather than failing.
    TrimLength,
}

/// Hints about how the pages of a range will be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionHint {
    DontNeed,
    AlwaysNeed,
}

/// Statistics about a pager-backed VMO.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PagerVmoStats {
    pub modified: bool,
}

pub struct VmObjectPaged {
    options: u32,
    cow_pages: Arc<VmCowPages>,
    /// The koid of the dispatcher that owns this VMO, if any.
    user_id: AtomicU64,
    name: Mutex<[u8; ZX_MAX_NAME_LEN]>,
}

impl VmObjectPaged {
    // `VmObject::options_` bitmask is extended with:
    pub const RESIZABLE: u32 = 1 << 1;
    pub const CONTIGUOUS: u32 = 1 << 2;
    pub const SLICE: u32 = 1 << 3;
    pub const DISCARDABLE: u32 = 1 << 4;
    pub const ALWAYS_PINNED: u32 = 1 << 5;
    pub const REFERENCE: u32 = 1 << 6;
    pub const CAN_BLOCK_ON_PAGE_REQUESTS: u32 = 1 << 31;

    /// The largest size a VMO can have. Ensures that MAX_SIZE + page::SIZE doesn't overflow so no
    /// VmObjects need to worry about overflow for loop bounds.
    pub const MAX_SIZE: u64 = VmPageList::MAX_SIZE;
    // RoundDownPageSize(u64::MAX) - page::SIZE
    const _MAX_SIZE_CHECK: () = assert!(Self::MAX_SIZE <= !(PAGE_SIZE - 1) - PAGE_SIZE);

    fn new(options: u32, cow_pages: Arc<VmCowPages>) -> Arc<Self> {
        let vmo = Arc::new(Self {
            options,
            cow_pages,
            user_id: AtomicU64::new(0),
            name: Mutex::new([0; ZX_MAX_NAME_LEN]),
        });
        // This creation has succeeded. Must wire up the cow pages.
        let mut locked = vmo.cow_pages.lock();
        vmo.cow_pages.transition_to_alive_locked(&mut locked);
        drop(locked);
        vmo
    }

    fn create_common(
        pmm_alloc_flags: u32,
        mut options: u32,
        size: u64,
    ) -> Result<Arc<Self>, Status> {
        debug_assert_eq!(options & (Self::CONTIGUOUS | Self::CAN_BLOCK_ON_PAGE_REQUESTS), 0);
        ensure!(options & (Self::DISCARDABLE | Self::ALWAYS_PINNED) == 0, Status::NOT_SUPPORTED);
        if pmm_alloc_flags & ALLOC_FLAG_CAN_WAIT != 0 {
            options |= Self::CAN_BLOCK_ON_PAGE_REQUESTS;
        }
        // make sure size is page aligned
        ensure!(page::is_aligned(size as usize), Status::INVALID_ARGS);
        ensure!(size <= Self::MAX_SIZE, Status::OUT_OF_RANGE);
        // This function isn't used to create slices or pager-backed VMOs, so VmCowPageOptions can
        // be kNone.
        let cow_pages = VmCowPages::create(cow_options::NONE, pmm_alloc_flags, size);
        Ok(Self::new(options, cow_pages))
    }

    /// Creates an anonymous VMO.
    pub fn create(pmm_alloc_flags: u32, options: u32, size: u64) -> Result<Arc<Self>, Status> {
        if options & (Self::CONTIGUOUS | Self::CAN_BLOCK_ON_PAGE_REQUESTS) != 0 {
            // Force callers to use CreateContiguous() instead.
            return Err(Status::INVALID_ARGS);
        }
        Self::create_common(pmm_alloc_flags, options, size)
    }

    /// Creates a VMO backed by `src`.
    pub fn create_external(
        src: Arc<PageSource>,
        options: u32,
        size: u64,
    ) -> Result<Arc<Self>, Status> {
        if options & (Self::DISCARDABLE | Self::CAN_BLOCK_ON_PAGE_REQUESTS | Self::ALWAYS_PINNED)
            != 0
        {
            return Err(Status::INVALID_ARGS);
        }
        // make sure size is page aligned
        ensure!(page::is_aligned(size as usize), Status::INVALID_ARGS);
        ensure!(size <= Self::MAX_SIZE, Status::OUT_OF_RANGE);
        // External VMOs always support delayed PMM allocations, since they already have to
        // tolerate arbitrary waits for pages due to the PageSource. The cow pages will have a page
        // source, so blocking is always possible.
        let options = options | Self::CAN_BLOCK_ON_PAGE_REQUESTS;
        let mut cow = cow_options::PAGE_SOURCE_ROOT;
        if src.properties().is_user_pager {
            cow |= cow_options::USER_PAGER_BACKED_ROOT;
        }
        let cow_pages = VmCowPages::create_external(src, cow, size);
        Ok(Self::new(options, cow_pages))
    }

    pub fn cow_pages(&self) -> &Arc<VmCowPages> {
        &self.cow_pages
    }

    pub fn is_resizable(&self) -> bool {
        self.options & Self::RESIZABLE != 0
    }

    pub fn is_contiguous(&self) -> bool {
        self.options & Self::CONTIGUOUS != 0
    }

    pub fn is_discardable(&self) -> bool {
        self.options & Self::DISCARDABLE != 0
    }

    pub fn is_user_pager_backed(&self) -> bool {
        self.cow_pages.is_root_source_user_pager_backed()
    }

    pub fn is_dirty_tracked(&self) -> bool {
        self.cow_pages.is_dirty_tracked()
    }

    /// Whether the VMO's size and a stream size can be managed together.
    pub fn is_stream_compatible(&self) -> bool {
        !self.is_contiguous()
    }

    fn can_block_on_page_requests(&self) -> bool {
        self.options & Self::CAN_BLOCK_ON_PAGE_REQUESTS != 0
    }

    pub fn user_id(&self) -> zx_koid_t {
        self.user_id.load(Ordering::Relaxed)
    }

    pub fn set_user_id(&self, user_id: zx_koid_t) {
        self.user_id.store(user_id, Ordering::Relaxed);
        self.update_debug_info();
    }

    pub fn get_name(&self) -> [u8; ZX_MAX_NAME_LEN] {
        *self.name.lock()
    }

    /// Sets the name, truncating it to fit with a terminating zero.
    pub fn set_name(&self, name: &[u8]) {
        let mut stored = [0; ZX_MAX_NAME_LEN];
        let name = &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())];
        let len = name.len().min(ZX_MAX_NAME_LEN - 1);
        stored[..len].copy_from_slice(&name[..len]);
        *self.name.lock() = stored;
        self.update_debug_info();
    }

    /// Tells the pages which VMO they are the pages of, for page requests.
    fn update_debug_info(&self) {
        let mut vmo_name = [0; 8];
        vmo_name.copy_from_slice(&self.name.lock()[..8]);
        let info = VmoDebugInfo { vmo_id: self.user_id(), vmo_name };
        self.cow_pages.lock().set_vmo_debug_info(info);
    }

    pub fn size(&self) -> u64 {
        self.cow_pages.lock().size()
    }

    pub fn num_mappings(&self) -> u32 {
        self.cow_pages.lock().num_mappings() as u32
    }

    /// Translates a range in this object to a range of its cow pages. There are no slices, so the
    /// ranges are the same, but must not overflow.
    fn get_cow_range(&self, offset: u64, len: u64) -> Option<VmCowRange> {
        in_range(offset, len, u64::MAX).then_some(VmCowRange::new(offset, len))
    }

    /// Like `get_cow_range`, but also checks for being within the size of the cow pages.
    fn get_cow_range_size_check_locked(
        &self,
        locked: &VmCowPagesLocked,
        offset: u64,
        len: u64,
    ) -> Option<VmCowRange> {
        in_range(offset, len, locked.size()).then_some(VmCowRange::new(offset, len))
    }

    /// Rounds [offset, offset + len) out to pages, failing on overflow.
    fn round_to_pages(offset: u64, len: u64) -> Result<(u64, u64), Status> {
        let end = offset.checked_add(len).ok_or(Status::OUT_OF_RANGE)?;
        let end_page = end.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::OUT_OF_RANGE)?;
        let offset = offset & !(PAGE_SIZE - 1);
        Ok((offset, end_page - offset))
    }

    /// fxdax: evicts the clean pages of the `len` bytes at `offset` of a pager-backed VMO now, as
    /// the evictor would, whatever their age, and returns how many: memory a tenant past its cap
    /// gives back (TASK-50.3). Their pager supplies them again as they are touched.
    pub fn evict_range(&self, offset: u64, len: u64) -> Result<u64, Status> {
        ensure!(self.cow_pages.can_evict(), Status::NOT_SUPPORTED);
        let reclaimed =
            self.cow_pages.reclaim_range_for_eviction(offset, len, EvictionAction::Demand);
        Ok(reclaimed.map_or(0, |reclaimed| reclaimed.num_pages))
    }

    pub fn hint_range(&self, offset: u64, len: u64, hint: EvictionHint) -> Result<(), Status> {
        // Ignore hints for non user-pager-backed VMOs. We choose to silently ignore hints for
        // incompatible combinations instead of failing. This is because the kernel does not make
        // any explicit guarantees on hints; since they are just hints, the kernel is always free
        // to ignore them.
        if !self.cow_pages.can_evict() {
            return Ok(());
        }
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        match hint {
            EvictionHint::DontNeed => self.cow_pages.promote_range_for_reclamation(cow_range),
            // Hints are best effort, so ignore any errors in the paging in process.
            EvictionHint::AlwaysNeed => {
                self.cow_pages.protect_range_from_reclamation(cow_range, true, true)
            }
        }
    }

    pub fn prefetch_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // Round offset and len to be page aligned.
        let (offset, len) = Self::round_to_pages(offset, len)?;
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        if self.cow_pages.is_root_source_user_pager_backed() {
            return self.cow_pages.protect_range_from_reclamation(cow_range, false, false);
        }
        // Nothing is compressed, so there is nothing to decompress.
        Ok(())
    }

    pub fn commit_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        self.commit_range_internal(offset, len, false, false)
    }

    pub fn commit_range_pinned(&self, offset: u64, len: u64, write: bool) -> Result<(), Status> {
        self.commit_range_internal(offset, len, true, write)
    }

    fn commit_range_internal(
        &self,
        offset: u64,
        len: u64,
        pin: bool,
        write: bool,
    ) -> Result<(), Status> {
        // We only expect write to be set if this a pin. All non-pin commits are reads.
        debug_assert!(!write || pin);
        // Round offset and len to be page aligned.
        let (mut offset, mut len) = Self::round_to_pages(offset, len)?;

        // Although the length, for the non-pin ranges, is allowed to end up outside the VMO range
        // during the operation, at least initially it must be within range.
        ensure!(in_range(offset, len, self.size()), Status::OUT_OF_RANGE);
        if len == 0 {
            // If pinning we explicitly forbid zero length pins as we cannot guarantee consistent
            // semantics.
            return if pin { Err(Status::INVALID_ARGS) } else { Ok(()) };
        }

        // Tracks the end of the pinned range to unpin in case of failure. The `offset` might lag
        // behind the pinned range, as it tracks the range that has been completely processed,
        // which would also include dirtying the page after pinning in case of a write.
        let pinned_start_offset = offset;
        let mut pinned_end_offset = offset;
        let result = (|| {
            let mut page_request = MultiPageRequest::default();
            // As we may need to wait on arbitrary page requests we just keep running this as long
            // as there is a non-zero range to process.
            let mut to_dirty_len = 0u64;
            while len > 0 {
                let mut committed_len = 0u64;
                let status = if to_dirty_len > 0 {
                    let mut locked = self.cow_pages.lock();
                    // The to_dirty_len *must* be within range, even though we just grabbed the lock
                    // and a resize could have happened, since the dirtied range is pinned.
                    debug_assert!(in_range(offset, to_dirty_len, locked.size()));
                    let mut dirty_len = 0;
                    let status = self.cow_pages.prepare_for_write_locked(
                        &mut locked,
                        VmCowRange::new(offset, to_dirty_len),
                        page_request.get_lazy_dirty_request(),
                        &mut dirty_len,
                    );
                    debug_assert!(dirty_len <= to_dirty_len);
                    if status == Err(Status::SHOULD_WAIT) {
                        page_request.made_dirty_request();
                    }
                    // Account for the pages that were dirtied during this attempt.
                    to_dirty_len -= dirty_len;
                    committed_len = dirty_len;
                    status
                } else {
                    let mut deferred = DeferredOps::new(&self.cow_pages);
                    let mut locked = self.cow_pages.lock();
                    let Some(new_len) = trim_range(offset, len, locked.size()) else {
                        return if pin { Err(Status::OUT_OF_RANGE) } else { Ok(()) };
                    };
                    if new_len != len {
                        ensure!(!pin, Status::OUT_OF_RANGE);
                        len = new_len;
                        if len == 0 {
                            break;
                        }
                    }
                    let status = self.cow_pages.commit_range_locked(
                        &mut locked,
                        VmCowRange::new(offset, len),
                        &mut deferred,
                        &mut committed_len,
                        &mut page_request,
                    );
                    debug_assert!(committed_len <= len);

                    // If we're required to pin, try to pin the committed range before waiting on
                    // the page_request, which has been populated to request pages beyond the
                    // committed range. This allows us to keep making forward progress as we will
                    // potentially pin a few pages before trying to fault in further pages, thereby
                    // preventing the already committed (and pinned) pages from being evicted while
                    // we wait with the lock dropped. There are no loaned pages to replace first.
                    if pin && committed_len > 0 {
                        // Verify that we are starting the pin after the previously pinned range, as
                        // we do not want to repeatedly pin the same pages.
                        assert_eq!(pinned_end_offset, offset);
                        self.cow_pages.pin_range_locked(
                            &mut locked,
                            VmCowRange::new(offset, committed_len),
                        )?;
                        // At this point we have successfully committed and pinned committed_len.
                        pinned_end_offset = offset + committed_len;

                        // If this is a write and the VMO supports dirty tracking, we also need to
                        // mark the pinned pages Dirty. We pin the pages first before marking them
                        // dirty in order to guarantee forward progress.
                        if write && self.is_dirty_tracked() {
                            // Prepare the committed range for writing. We need a page request for
                            // this too, so cancel any existing one and reuse it.
                            page_request.cancel_requests();
                            // We want to dirty the entire pinned range.
                            to_dirty_len = committed_len;
                            continue;
                        }
                    }
                    status
                };
                match status {
                    Ok(()) => {}
                    Err(Status::SHOULD_WAIT) => page_request.wait(true)?,
                    Err(status) => return Err(status),
                }
                offset += committed_len;
                len -= committed_len;
            }
            Ok(())
        })();

        // Should any errors occur we need to unpin everything. If we were asked to write, we need
        // to mark the VMO modified if any pages were committed.
        if pinned_end_offset > pinned_start_offset {
            let mut locked = self.cow_pages.lock();
            if result.is_err() {
                self.cow_pages.unpin_locked(
                    &mut locked,
                    VmCowRange::new(pinned_start_offset, pinned_end_offset - pinned_start_offset),
                );
            } else if write {
                self.cow_pages.mark_modified_locked(&mut locked);
            }
        }
        result
    }

    pub fn unpin(&self, offset: u64, len: u64) {
        let cow_range = self.get_cow_range(offset, len).expect("unpin range overflows");
        let _deferred = DeferredOps::new(&self.cow_pages);
        let mut locked = self.cow_pages.lock();
        self.cow_pages.unpin_locked(&mut locked, cow_range);
    }

    pub fn decommit_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.decommit_range(cow_range)
    }

    fn zero_partial_page(
        &self,
        page_base_offset: u64,
        zero_start_offset: u64,
        zero_end_offset: u64,
    ) -> Result<(), Status> {
        if !self.partial_page_needs_zeroing(page_base_offset, zero_start_offset, zero_end_offset)? {
            return Ok(());
        }
        // Need to actually zero out bytes in the page.
        self.read_write_internal(
            page_base_offset + zero_start_offset,
            (zero_end_offset - zero_start_offset) as usize,
            true,
            VmObjectReadWriteOptions::None,
            &mut zero_bytes,
        )
        .0
    }

    async fn zero_partial_page_async(
        &self,
        page_base_offset: u64,
        zero_start_offset: u64,
        zero_end_offset: u64,
    ) -> Result<(), Status> {
        if !self.partial_page_needs_zeroing(page_base_offset, zero_start_offset, zero_end_offset)? {
            return Ok(());
        }
        self.read_write_internal_async(
            page_base_offset + zero_start_offset,
            (zero_end_offset - zero_start_offset) as usize,
            true,
            VmObjectReadWriteOptions::None,
            &mut zero_bytes,
        )
        .await
        .0
    }

    /// Whether the bytes of the page at `page_base_offset` from `zero_start_offset` to
    /// `zero_end_offset` must be zeroed, as the page would not read as zeros.
    fn partial_page_needs_zeroing(
        &self,
        page_base_offset: u64,
        zero_start_offset: u64,
        zero_end_offset: u64,
    ) -> Result<bool, Status> {
        debug_assert!(zero_start_offset <= zero_end_offset);
        debug_assert!(zero_end_offset <= PAGE_SIZE);
        debug_assert!(page::is_aligned(page_base_offset as usize));
        let locked = self.cow_pages.lock();
        ensure!(page_base_offset < locked.size(), Status::OUT_OF_RANGE);
        // If the page is already considered zero there is no need to redundantly zero it again.
        Ok(!self.cow_pages.page_would_read_zero_locked(&locked, page_base_offset))
    }

    pub fn zero_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        self.zero_range_internal(offset, len, true)
    }

    /// Zeroes the range, as `zero_range` does, as a future that waits on page requests rather
    /// than blocking the calling thread.
    pub async fn zero_range_async(&self, offset: u64, len: u64) -> Result<(), Status> {
        let mut zero = ZeroRange { offset, len, dirty_track: true };
        loop {
            match self.zero_range_step(&mut zero) {
                ZeroRangeStep::Done(result) => return result,
                ZeroRangeStep::Partial { page_base, start, end } => {
                    self.zero_partial_page_async(page_base, start, end).await?
                }
                ZeroRangeStep::Wait(mut page_request) => page_request.wait_async(true).await?,
            }
        }
    }

    pub fn zero_range_untracked(&self, offset: u64, len: u64) -> Result<(), Status> {
        // We don't expect any committed pages to remain at the end of this call, so we should be
        // operating on whole pages.
        ensure!(
            page::is_aligned(offset as usize) && page::is_aligned(len as usize),
            Status::INVALID_ARGS
        );
        self.zero_range_internal(offset, len, false)
    }

    fn zero_range_internal(&self, offset: u64, len: u64, dirty_track: bool) -> Result<(), Status> {
        let mut zero = ZeroRange { offset, len, dirty_track };
        loop {
            match self.zero_range_step(&mut zero) {
                ZeroRangeStep::Done(result) => return result,
                ZeroRangeStep::Partial { page_base, start, end } => {
                    self.zero_partial_page(page_base, start, end)?
                }
                ZeroRangeStep::Wait(mut page_request) => page_request.wait(true)?,
            }
        }
    }

    /// Advances the zeroing `zero` until it finishes, or a partial page must be zeroed or a page
    /// request waited on, which must be done without the lock. What it asks for is already
    /// accounted for in `zero`.
    fn zero_range_step(&self, zero: &mut ZeroRange) -> ZeroRangeStep {
        // May need to zero in chunks across multiple different lock acquisitions so loop until
        // nothing left to do.
        while zero.len > 0 {
            // Check for any non-page aligned start and handle separately.
            if !page::is_aligned(zero.offset as usize) {
                // We're doing partial page writes, so we should be dirty tracking.
                debug_assert!(zero.dirty_track);
                let page_base = zero.offset & !(PAGE_SIZE - 1);
                let zero_start_offset = zero.offset - page_base;
                let zero_len = (PAGE_SIZE - zero_start_offset).min(zero.len);
                // Advance over the length to zero and then, since the lock might be dropped, go
                // around the loop to redo the checks.
                zero.offset += zero_len;
                zero.len -= zero_len;
                return ZeroRangeStep::Partial {
                    page_base,
                    start: zero_start_offset,
                    end: zero_start_offset + zero_len,
                };
            }
            // The start is page aligned, so if the remaining length is not a page size then
            // perform the final sub-page zero.
            if zero.len < PAGE_SIZE {
                debug_assert!(zero.dirty_track);
                let end = zero.len;
                zero.len = 0;
                return ZeroRangeStep::Partial { page_base: zero.offset, start: 0, end };
            }
            let aligned_len = zero.len & !(PAGE_SIZE - 1);

            // First try and do the more efficient decommit. We prefer decommit as it performs work
            // in the order of the number of committed pages, instead of work in the order of size
            // of the range. An error from DecommitRange indicates that the VMO is not of a form
            // that decommit can safely be performed without exposing data that we shouldn't, but
            // no actual state will have been changed.
            let Some(cow_range) = self.get_cow_range(zero.offset, aligned_len) else {
                return ZeroRangeStep::Done(Err(Status::OUT_OF_RANGE));
            };
            if self.cow_pages.decommit_range(cow_range).is_ok() {
                zero.offset += cow_range.len;
                zero.len -= cow_range.len;
                continue;
            }

            // We might need a page request if the VMO is backed by a page source.
            let mut page_request = MultiPageRequest::default();
            let (status, zeroed_len) = {
                let mut deferred = DeferredOps::new(&self.cow_pages);
                let mut locked = self.cow_pages.lock();
                // Offset is page aligned, and we have at least one full page to process, so find
                // the page aligned length to hand over to the cow pages zero method.
                let Some(cow_range) =
                    self.get_cow_range_size_check_locked(&locked, zero.offset, aligned_len)
                else {
                    return ZeroRangeStep::Done(Err(Status::OUT_OF_RANGE));
                };
                let (status, zeroed_len) = self.cow_pages.zero_pages_locked(
                    &mut locked,
                    cow_range,
                    zero.dirty_track,
                    &mut deferred,
                    &mut page_request,
                );
                if zeroed_len != 0 {
                    // Mark modified since we wrote zeros.
                    self.cow_pages.mark_modified_locked(&mut locked);
                }
                (status, zeroed_len)
            };
            // Advance over pages that had already been zeroed.
            zero.offset += zeroed_len;
            zero.len -= zeroed_len;
            // Wait on any page request, which is the only non-fatal error case.
            match status {
                Ok(()) => {}
                Err(Status::SHOULD_WAIT) => return ZeroRangeStep::Wait(page_request),
                Err(status) => return ZeroRangeStep::Done(Err(status)),
            }
        }
        ZeroRangeStep::Done(Ok(()))
    }

    pub fn resize(&self, s: u64) -> Result<(), Status> {
        ensure!(self.is_resizable(), Status::UNAVAILABLE);
        // ensure the size is valid and that we will not wrap.
        ensure!(page::is_aligned(s as usize), Status::INVALID_ARGS);
        ensure!(s <= Self::MAX_SIZE, Status::OUT_OF_RANGE);
        self.cow_pages.resize(s)
    }

    /// Performs some sort of copy in/out on a range of the object using the passed in copy
    /// routine, which is called with a pointer into a page, the offset into the caller's buffer
    /// and the length. Returns the status and how much was copied.
    fn read_write_internal(
        &self,
        offset: u64,
        len: usize,
        write: bool,
        options: VmObjectReadWriteOptions,
        copyfunc: &mut dyn FnMut(*mut u8, usize, usize) -> Result<(), UserCopyCaptureFaultsError>,
    ) -> (Result<(), Status>, usize) {
        let mut rw = match ReadWrite::new(offset, len, write, options) {
            Ok(rw) => rw,
            Err(status) => return (Err(status), 0),
        };
        loop {
            let resolved = match self.read_write_step(&mut rw, copyfunc) {
                ReadWriteStep::Done(status) => return (status, rw.copied()),
                ReadWriteStep::Fault { info, len } => {
                    Current::soft_fault_in_range(info.pf_va, info.pf_flags, len)
                }
                ReadWriteStep::Wait => rw.page_request.wait(true),
            };
            if let Some(result) = rw.after_wait(resolved) {
                return result;
            }
        }
    }

    /// Performs a copy in/out on a range of the object, as `read_write_internal` does, as a future
    /// that waits on page requests and faults rather than blocking the calling thread.
    async fn read_write_internal_async(
        &self,
        offset: u64,
        len: usize,
        write: bool,
        options: VmObjectReadWriteOptions,
        copyfunc: &mut (
                 dyn FnMut(*mut u8, usize, usize) -> Result<(), UserCopyCaptureFaultsError> + Send
             ),
    ) -> (Result<(), Status>, usize) {
        let mut rw = match ReadWrite::new(offset, len, write, options) {
            Ok(rw) => rw,
            Err(status) => return (Err(status), 0),
        };
        loop {
            let resolved = match self.read_write_step(&mut rw, copyfunc) {
                ReadWriteStep::Done(status) => return (status, rw.copied()),
                ReadWriteStep::Fault { info, len } => {
                    Current::soft_fault_in_range_async(info.pf_va, info.pf_flags, len).await
                }
                ReadWriteStep::Wait => rw.page_request.wait_async(true).await,
            };
            if let Some(result) = rw.after_wait(resolved) {
                return result;
            }
        }
    }

    /// Advances the copy `rw` until it finishes, or it must wait on its page request or resolve
    /// a fault on the caller's memory, which must be done without the lock.
    fn read_write_step(
        &self,
        rw: &mut ReadWrite,
        copyfunc: &mut dyn FnMut(*mut u8, usize, usize) -> Result<(), UserCopyCaptureFaultsError>,
    ) -> ReadWriteStep {
        loop {
            let mut fault_info = None;
            let status = {
                let mut deferred = DeferredOps::new(&self.cow_pages);
                let mut locked = self.cow_pages.lock();
                let size = locked.size();
                if rw.end_offset > size {
                    if rw.options == VmObjectReadWriteOptions::TrimLength {
                        if rw.src_offset >= size {
                            return ReadWriteStep::Done(Ok(()));
                        }
                        rw.end_offset = size;
                    } else {
                        return ReadWriteStep::Done(Err(Status::OUT_OF_RANGE));
                    }
                } else if rw.src_offset >= rw.end_offset {
                    return ReadWriteStep::Done(Ok(()));
                }

                let first_page_offset = rw.src_offset & !(PAGE_SIZE - 1);
                let last_page_offset = (rw.end_offset - 1) & !(PAGE_SIZE - 1);
                let mut remaining_pages = (last_page_offset - first_page_offset) / PAGE_SIZE + 1;
                let mut modified = false;

                let mut cursor = match self.get_lookup_cursor_locked(
                    &locked,
                    first_page_offset,
                    remaining_pages * PAGE_SIZE,
                ) {
                    Ok(cursor) => cursor,
                    Err(status) => return ReadWriteStep::Done(Err(status)),
                };
                // Performing explicit accesses by request of the user, so disable zero forking.
                cursor.disable_zero_fork();

                let mut status = Ok(());
                while remaining_pages > 0 {
                    let page_offset = (rw.src_offset % PAGE_SIZE) as usize;
                    let tocopy =
                        (page::SIZE - page_offset).min((rw.end_offset - rw.src_offset) as usize);

                    // If we need to wait on pages then we would like to wait on as many as
                    // possible, up to the actual limit of the read/write operation. For a read we
                    // can wake up once some pages are received, minimizing the latency before we
                    // start making progress, but as this is not true for writes we cap the maximum
                    // number requested.
                    const MAX_WRITE_WAIT_PAGES: u64 = 256;
                    let max_wait_pages = if rw.write { MAX_WRITE_WAIT_PAGES } else { u64::MAX };
                    let max_waitable_pages = remaining_pages.min(max_wait_pages);

                    // Attempt to lookup a page
                    let result = cursor.require_page(
                        &self.cow_pages,
                        &mut locked,
                        rw.write,
                        max_waitable_pages,
                        &mut deferred,
                        &mut rw.page_request,
                    );
                    let page = match result {
                        Ok(RequireResult { page, .. }) => page,
                        Err(error) => {
                            status = Err(error);
                            break;
                        }
                    };
                    // Compute the kernel mapping of this page.
                    let page_ptr = paddr_to_physmap(page.paddr());
                    // Call the copy routine. If the copy was successful then Ok is returned,
                    // otherwise the fault it captured, if any, which can be resolved and the copy
                    // retried.
                    // SAFETY: `page_offset` is within the page.
                    if let Err(error) =
                        copyfunc(unsafe { page_ptr.add(page_offset) }, rw.dest_offset, tocopy)
                    {
                        // If a fault has actually occurred, then we will have captured fault info
                        // that we can use to handle the fault.
                        if error.fault_info.is_some() {
                            fault_info = error.fault_info;
                            break;
                        }
                        // If we encounter _any_ unrecoverable error from the copy operation which
                        // produced no fault address, squash the error down to just "NOT_FOUND".
                        // This is what the SoftFault error would have told us if we did try to
                        // handle the fault and could not.
                        status = Err(Status::NOT_FOUND);
                        break;
                    }
                    // Advance the copy location.
                    rw.src_offset += tocopy as u64;
                    rw.dest_offset += tocopy;
                    remaining_pages -= 1;
                    modified = rw.write;
                }
                // Before dropping the lock, check if any pages were modified and update the VMO
                // state accordingly.
                if modified {
                    self.cow_pages.mark_modified_locked(&mut locked);
                }
                status
            };
            // If there was a fault while copying, then handle it now that the lock is dropped.
            if let Some(info) = fault_info {
                return ReadWriteStep::Fault { info, len: rw.len - rw.dest_offset };
            }
            match status {
                Ok(()) => {}
                Err(Status::SHOULD_WAIT) => {
                    // RequirePage 'failed', but told us that it had filled out the page request,
                    // so we should wait on it.
                    debug_assert!(self.can_block_on_page_requests());
                    return ReadWriteStep::Wait;
                }
                Err(status) => return ReadWriteStep::Done(Err(status)),
            }
            if rw.src_offset >= rw.end_offset {
                return ReadWriteStep::Done(Ok(()));
            }
        }
    }

    /// Reads into `buf` from `offset`.
    pub fn read(&self, buf: &mut [u8], offset: u64) -> Result<(), Status> {
        let ptr = buf.as_mut_ptr();
        self.read_write_internal(
            offset,
            buf.len(),
            false,
            VmObjectReadWriteOptions::None,
            &mut |src, off, len| {
                // SAFETY: `src` points at `len` bytes of a page, and `off + len` is within `buf`.
                unsafe { std::ptr::copy_nonoverlapping(src, ptr.add(off), len) };
                Ok(())
            },
        )
        .0
    }

    /// Writes `buf` at `offset`.
    pub fn write(&self, buf: &[u8], offset: u64) -> Result<(), Status> {
        let ptr = buf.as_ptr();
        self.read_write_internal(
            offset,
            buf.len(),
            true,
            VmObjectReadWriteOptions::None,
            &mut |dst, off, len| {
                // SAFETY: `dst` points at `len` bytes of a page, and `off + len` is within `buf`.
                unsafe { std::ptr::copy_nonoverlapping(ptr.add(off), dst, len) };
                Ok(())
            },
        )
        .0
    }

    /// Reads `len` bytes at `offset` into user memory at `ptr`, returning the status and how many
    /// bytes were read.
    pub fn read_user(
        &self,
        ptr: UserOutPtr<u8>,
        offset: u64,
        len: usize,
        options: VmObjectReadWriteOptions,
    ) -> (Result<(), Status>, usize) {
        // read routine that uses copy_to_user
        let mut read_routine = |src: *mut u8, offset: usize, len: usize| {
            // SAFETY: `src` points at `len` bytes of a page.
            let src = unsafe { std::slice::from_raw_parts(src, len) };
            ptr.byte_offset(offset as isize).copy_slice_to_user_capture_faults(src)
        };

        self.read_write_internal(offset, len, false, options, &mut read_routine)
    }

    /// Writes `len` bytes from user memory at `ptr` at `offset`, returning the status and how many
    /// bytes were written.
    ///
    /// `on_bytes_transferred` is called with the VMO offset and length of each chunk copied.
    pub fn write_user(
        &self,
        ptr: UserInPtr<u8>,
        offset: u64,
        len: usize,
        options: VmObjectReadWriteOptions,
        on_bytes_transferred: Option<&dyn Fn(u64, usize)>,
    ) -> (Result<(), Status>, usize) {
        let base_vmo_offset = offset;
        // write routine that uses copy_from_user
        let mut write_routine = |dst: *mut u8, offset: usize, len: usize| {
            // SAFETY: `dst` points at `len` bytes of a page, which is plain memory.
            let dst = unsafe { std::slice::from_raw_parts_mut(dst.cast::<MaybeUninit<u8>>(), len) };
            ptr.byte_offset(offset as isize).copy_slice_from_user_capture_faults(dst)?;
            if let Some(on_bytes_transferred) = on_bytes_transferred {
                on_bytes_transferred(base_vmo_offset + offset as u64, len);
            }
            Ok(())
        };

        self.read_write_internal(offset, len, true, options, &mut write_routine)
    }

    /// Reads `len` bytes at `offset` into the user memory `vec` describes, returning the status and
    /// how many bytes were read.
    pub fn read_user_vector(
        &self,
        vec: UserOutIovec,
        mut offset: u64,
        mut len: usize,
    ) -> (Result<(), Status>, usize) {
        if len == 0 {
            return (Ok(()), 0);
        }
        if len as u64 > u64::MAX - offset {
            return (Err(Status::OUT_OF_RANGE), 0);
        }

        let mut total = 0;
        let status = vec.for_each(|ptr, mut capacity| {
            if capacity > len {
                capacity = len;
            }

            let (read_status, chunk_actual) =
                self.read_user(ptr, offset, capacity, VmObjectReadWriteOptions::None);

            // Always add |chunk_actual| since some bytes may have been transferred, even on error
            total += chunk_actual;
            read_status?;

            debug_assert!(chunk_actual == capacity);

            offset += chunk_actual as u64;
            len -= chunk_actual;
            // `ZX_ERR_STOP` ends the iteration, as for the C++ iovecs' `ForEach`.
            if len > 0 { Ok(()) } else { Err(Status::STOP) }
        });
        let status = if status == Err(Status::STOP) { Ok(()) } else { status };

        // Return |ZX_ERR_BUFFER_TOO_SMALL| if all of |len| was not transferred.
        let status = if status.is_ok() && len > 0 { Err(Status::BUFFER_TOO_SMALL) } else { status };
        (status, total)
    }

    /// Writes `len` bytes from the user memory `vec` describes at `offset`, returning the status
    /// and how many bytes were written.
    ///
    /// `on_bytes_transferred` is called with the VMO offset and length of each chunk copied.
    pub fn write_user_vector(
        &self,
        vec: UserInIovec,
        mut offset: u64,
        mut len: usize,
        on_bytes_transferred: Option<&dyn Fn(u64, usize)>,
    ) -> (Result<(), Status>, usize) {
        if len == 0 {
            return (Ok(()), 0);
        }
        if len as u64 > u64::MAX - offset {
            return (Err(Status::OUT_OF_RANGE), 0);
        }

        let mut total = 0;
        let status = vec.for_each(|ptr, mut capacity| {
            if capacity > len {
                capacity = len;
            }

            let (write_status, chunk_actual) = self.write_user(
                ptr,
                offset,
                capacity,
                VmObjectReadWriteOptions::None,
                on_bytes_transferred,
            );

            // Always add |chunk_actual| since some bytes may have been transferred, even on error
            total += chunk_actual;
            write_status?;

            debug_assert!(chunk_actual == capacity);

            offset += chunk_actual as u64;
            len -= chunk_actual;
            // `ZX_ERR_STOP` ends the iteration, as for the C++ iovecs' `ForEach`.
            if len > 0 { Ok(()) } else { Err(Status::STOP) }
        });
        let status = if status == Err(Status::STOP) { Ok(()) } else { status };

        // Return |ZX_ERR_BUFFER_TOO_SMALL| if all of |len| was not transferred.
        let status = if status.is_ok() && len > 0 { Err(Status::BUFFER_TOO_SMALL) } else { status };
        (status, total)
    }

    /// Writes as `write_user_vector` does, calling `on_progress` for each chunk copied that
    /// extends beyond `prev_stream_size`, as `cpp_vm_object_paged_write_user_vector_progress`
    /// does.
    pub fn write_user_vector_progress(
        &self,
        vec: UserInIovec,
        offset: u64,
        len: usize,
        prev_stream_size: u64,
        on_progress: &dyn Fn(u64, usize),
    ) -> (Result<(), Status>, usize) {
        let on_bytes_transferred = |write_offset: u64, len: usize| {
            if write_offset + len as u64 > prev_stream_size {
                on_progress(write_offset, len);
            }
        };
        self.write_user_vector(vec, offset, len, Some(&on_bytes_transferred))
    }

    /// Reads `len` bytes at `offset` into the host buffers `bufs`, as `read_user_vector`
    /// reads into user memory, as a future that waits on page requests and faults rather than
    /// blocking the calling thread. Returns the status and how many bytes were read.
    pub async fn read_vectored_async(
        &self,
        bufs: &[HostBuffer],
        mut offset: u64,
        mut len: usize,
    ) -> (Result<(), Status>, usize) {
        if len == 0 {
            return (Ok(()), 0);
        }
        if len as u64 > u64::MAX - offset {
            return (Err(Status::OUT_OF_RANGE), 0);
        }
        let mut total = 0;
        for buf in bufs {
            if len == 0 {
                break;
            }
            let capacity = buf.len.min(len);
            let dst = buf.addr;
            let (status, actual) = self
                .read_write_internal_async(
                    offset,
                    capacity,
                    false,
                    VmObjectReadWriteOptions::None,
                    &mut |src, buf_offset, len| {
                        // SAFETY: `src` points at `len` bytes of a page.
                        let src = unsafe { std::slice::from_raw_parts(src, len) };
                        UserOutPtr::new((dst + buf_offset) as *mut u8)
                            .copy_slice_to_user_capture_faults(src)
                    },
                )
                .await;
            // Always add `actual` since some bytes may have been transferred, even on error.
            total += actual;
            if status.is_err() {
                return (status, total);
            }
            debug_assert!(actual == capacity);
            offset += actual as u64;
            len -= actual;
        }
        // Return `ZX_ERR_BUFFER_TOO_SMALL` if all of `len` was not transferred.
        let status = if len > 0 { Err(Status::BUFFER_TOO_SMALL) } else { Ok(()) };
        (status, total)
    }

    /// Writes `len` bytes from the host buffers `bufs` at `offset`, as
    /// `write_user_vector` writes from user memory, as a future that waits on page requests and
    /// faults rather than blocking the calling thread. Returns the status and how many bytes were
    /// written.
    ///
    /// `on_bytes_transferred` is called with the VMO offset and length of each chunk copied.
    pub async fn write_vectored_async(
        &self,
        bufs: &[HostBuffer],
        mut offset: u64,
        mut len: usize,
        on_bytes_transferred: Option<&(dyn Fn(u64, usize) + Sync)>,
    ) -> (Result<(), Status>, usize) {
        if len == 0 {
            return (Ok(()), 0);
        }
        if len as u64 > u64::MAX - offset {
            return (Err(Status::OUT_OF_RANGE), 0);
        }
        let mut total = 0;
        for buf in bufs {
            if len == 0 {
                break;
            }
            let capacity = buf.len.min(len);
            let src = buf.addr;
            let base_vmo_offset = offset;
            let (status, actual) = self
                .read_write_internal_async(
                    offset,
                    capacity,
                    true,
                    VmObjectReadWriteOptions::None,
                    &mut |dst, buf_offset, len| {
                        // SAFETY: `dst` points at `len` bytes of a page, which is plain memory.
                        let dst = unsafe {
                            std::slice::from_raw_parts_mut(dst.cast::<MaybeUninit<u8>>(), len)
                        };
                        UserInPtr::new((src + buf_offset) as *const u8)
                            .copy_slice_from_user_capture_faults(dst)?;
                        if let Some(on_bytes_transferred) = on_bytes_transferred {
                            on_bytes_transferred(base_vmo_offset + buf_offset as u64, len);
                        }
                        Ok(())
                    },
                )
                .await;
            // Always add `actual` since some bytes may have been transferred, even on error.
            total += actual;
            if status.is_err() {
                return (status, total);
            }
            debug_assert!(actual == capacity);
            offset += actual as u64;
            len -= actual;
        }
        // Return `ZX_ERR_BUFFER_TOO_SMALL` if all of `len` was not transferred.
        let status = if len > 0 { Err(Status::BUFFER_TOO_SMALL) } else { Ok(()) };
        (status, total)
    }

    /// Writes as `write_vectored_async` does, calling `on_progress` for each chunk copied that
    /// extends beyond `prev_stream_size`, as `write_user_vector_progress` does.
    pub async fn write_vectored_progress_async(
        &self,
        bufs: &[HostBuffer],
        offset: u64,
        len: usize,
        prev_stream_size: u64,
        on_progress: &(dyn Fn(u64, usize) + Sync),
    ) -> (Result<(), Status>, usize) {
        let on_bytes_transferred = |write_offset: u64, len: usize| {
            if write_offset + len as u64 > prev_stream_size {
                on_progress(write_offset, len);
            }
        };
        self.write_vectored_async(bufs, offset, len, Some(&on_bytes_transferred)).await
    }

    /// Performs a cache maintenance operation on the range. Host memory is coherent and VMOs are
    /// not executable, so only the range is checked.
    pub fn cache_op(&self, offset: u64, len: u64) -> Result<(), Status> {
        ensure!(len != 0, Status::INVALID_ARGS);
        let locked = self.cow_pages.lock();
        // verify that the range is within the object
        self.get_cow_range_size_check_locked(&locked, offset, len).ok_or(Status::OUT_OF_RANGE)?;
        Ok(())
    }

    /// Calls `lookup_fn` with the offset and physical address of each committed page in the range.
    pub fn lookup(
        &self,
        offset: u64,
        len: u64,
        lookup_fn: impl FnMut(u64, PAddr) -> Status,
    ) -> Result<(), Status> {
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        let locked = self.cow_pages.lock();
        self.cow_pages.lookup_locked(&locked, cow_range, lookup_fn)
    }

    pub fn take_pages(
        &self,
        offset: u64,
        len: u64,
        pages: &mut VmPageSpliceList,
    ) -> Result<(), Status> {
        ensure!(!self.is_contiguous(), Status::NOT_SUPPORTED);
        let mut range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        // Initialize the splice list to the right size.
        pages.initialize(range.len);
        let mut splice_offset = 0;
        while !range.is_empty() {
            let mut taken_len = 0;
            self.cow_pages.take_pages(range, splice_offset, pages, &mut taken_len)?;
            // We should have taken the entire range requested, as there are no page requests to
            // wait on without compressed pages.
            debug_assert_eq!(taken_len, range.len);
            splice_offset += taken_len;
            // Record the completed portion.
            range = range.trimmed_from_start(taken_len);
        }
        Ok(())
    }

    pub fn supply_pages(
        &self,
        offset: u64,
        len: u64,
        pages: &mut VmPageSpliceList,
        options: SupplyOptions,
    ) -> Result<(), Status> {
        // We need this check here instead of in SupplyPagesLocked, as we do use that function to
        // provide pages to contiguous VMOs as well.
        ensure!(!self.is_contiguous(), Status::NOT_SUPPORTED);
        let range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        if range.is_empty() {
            return Ok(());
        }
        // Pre-process pages: with no references to convert into pages, as nothing is compressed,
        // that is fxdax's copying of pages of other arenas into the object's, which waits for
        // pages, and so comes before any lock is taken. Pages from the PhysicalPageProvider, which
        // are not yet initialized, are supplied as they are, as Zircon's provider supplies them.
        if options != SupplyOptions::PhysicalPageProvider {
            self.cow_pages.process_pages_for_supply(pages)?;
        }
        let mut deferred = DeferredOps::new(&self.cow_pages);
        let mut locked = self.cow_pages.lock();
        self.cow_pages.supply_pages_locked(&mut locked, range, pages, options, &mut deferred)
    }

    /// Supplies [offset, offset + len) with the pages of `src` at `src_offset`, which this object
    /// then shares with `src` rather than taking. See `VmCowPages::lend_pages`.
    pub fn supply_shared_pages(
        &self,
        offset: u64,
        len: u64,
        src: &VmObjectPaged,
        src_offset: u64,
    ) -> Result<(), Status> {
        ensure!(!self.is_contiguous() && !src.is_contiguous(), Status::NOT_SUPPORTED);
        let range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        let src_range = src.get_cow_range(src_offset, len).ok_or(Status::OUT_OF_RANGE)?;
        if range.is_empty() {
            return Ok(());
        }
        src.cow_pages.lend_pages(src_range, &self.cow_pages, range.offset)
    }

    pub fn fail_page_requests(
        &self,
        offset: u64,
        len: u64,
        error_status: Status,
    ) -> Result<(), Status> {
        let locked = self.cow_pages.lock();
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.fail_page_requests_locked(&locked, cow_range, error_status)
    }

    pub fn dirty_pages(&self, offset: u64, len: u64) -> Result<(), Status> {
        // It is possible to encounter delayed PMM allocations, which requires waiting on the
        // page_request.
        let mut page_request = AnonymousPageRequest::new();
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;

        // Initialize a list of allocated pages that DirtyPages will allocate any new pages into
        // before inserting them in the VMO. Allocated pages can therefore be shared across multiple
        // calls to DirtyPages. Instead of having to allocate and free pages in case DirtyPages
        // cannot successfully dirty the entire range atomically, we can just hold on to the
        // allocated pages and use them for the next call. This ensures that we are making forward
        // progress with each successive call to DirtyPages.
        let mut alloc_list = VmPageDoublyLinkedList::new();
        let result = loop {
            match self.cow_pages.dirty_pages(cow_range, &mut alloc_list, &mut page_request) {
                Ok(()) => break Ok(()),
                Err(Status::SHOULD_WAIT) => {
                    if let Err(status) = page_request.allocate() {
                        // fxdax: the requests are resolved, or failed, as for a failure to dirty
                        // the pages (TASK-79.75).
                        self.cow_pages.dirty_pages_failed(cow_range, status);
                        break Err(status);
                    }
                    // If the wait was successful, loop around and try the call again, which will
                    // re-validate any state that might have changed when the lock was dropped.
                }
                Err(status) => break Err(status),
            }
        };
        // A page allocated by the wait but not used is freed when the request is dropped.
        if let Some(page) = page_request.take_page() {
            alloc_list.push_back(page);
        }
        if !alloc_list.is_empty() {
            self.cow_pages.free_pages(&mut alloc_list);
        }
        result
    }

    /// Calls `dirty_range_fn` with each dirty range in the given range, with its offset, length
    /// and whether it is zero.
    pub fn enumerate_dirty_ranges(
        &self,
        offset: u64,
        len: u64,
        dirty_range_fn: impl FnMut(u64, u64, bool) -> Status,
    ) -> Result<(), Status> {
        let locked = self.cow_pages.lock();
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.enumerate_dirty_ranges_locked(&locked, cow_range, dirty_range_fn)
    }

    pub fn query_pager_vmo_stats(&self, reset: bool) -> Result<PagerVmoStats, Status> {
        let mut locked = self.cow_pages.lock();
        let modified = self.cow_pages.query_pager_vmo_stats_locked(&mut locked, reset)?;
        Ok(PagerVmoStats { modified })
    }

    pub fn reset_pager_vmo_stats(&self) {
        let mut locked = self.cow_pages.lock();
        // Only pager-backed VMOs have stats to reset.
        let _ = self.cow_pages.query_pager_vmo_stats_locked(&mut locked, true);
    }

    pub fn writeback_begin(
        &self,
        offset: u64,
        len: u64,
        is_zero_range: bool,
    ) -> Result<(), Status> {
        let mut locked = self.cow_pages.lock();
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.writeback_begin_locked(&mut locked, cow_range, is_zero_range)
    }

    pub fn writeback_end(&self, offset: u64, len: u64) -> Result<(), Status> {
        let mut locked = self.cow_pages.lock();
        let cow_range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.writeback_end_locked(&mut locked, cow_range)
    }

    pub fn detach_source(&self) {
        self.cow_pages.detach_source();
    }

    pub fn get_page_source_koid(&self) -> Option<zx_koid_t> {
        self.cow_pages.get_page_source_koid()
    }

    /// Gets a cursor for the specified range in the VMO. The returned cursor and all the items
    /// returned by the cursor are only valid as long as the lock is continuously held.
    pub fn get_lookup_cursor_locked(
        &self,
        locked: &VmCowPagesLocked,
        offset: u64,
        max_len: u64,
    ) -> Result<LookupCursor, Status> {
        let cow_range = self.get_cow_range(offset, max_len).ok_or(Status::OUT_OF_RANGE)?;
        self.cow_pages.get_lookup_cursor_locked(locked, cow_range)
    }

    /// Looks up the page at `offset` for a fault or lookup described by `pf_flags`.
    pub fn get_page(
        &self,
        offset: u64,
        pf_flags: u32,
        page_request: &mut MultiPageRequest,
    ) -> Result<VmPagePtr, Status> {
        let mut deferred = DeferredOps::new(&self.cow_pages);
        let mut locked = self.cow_pages.lock();
        let write = pf_flags & flag::WRITE != 0;
        let mut cursor = self.get_lookup_cursor_locked(&locked, offset, PAGE_SIZE)?;
        // Hardware faults are considered to update access times separately, all other lookup
        // reasons should do the default update of access time.
        if pf_flags & flag::HW_FAULT != 0 {
            cursor.disable_mark_accessed();
        }
        if pf_flags & flag::FAULT_MASK == 0 {
            return cursor.maybe_page(&mut locked, write).ok_or(Status::NOT_FOUND);
        }
        let result = cursor.require_page(
            &self.cow_pages,
            &mut locked,
            write,
            1,
            &mut deferred,
            page_request,
        )?;
        Ok(result.page)
    }

    pub fn reclamation_event_count(&self) -> u64 {
        let locked = self.cow_pages.lock();
        self.cow_pages.reclamation_event_count_locked(&locked)
    }

    /// Returns the number of bytes of committed pages.
    pub fn committed_bytes(&self) -> u64 {
        let locked = self.cow_pages.lock();
        self.cow_pages.committed_page_count_locked(&locked) * PAGE_SIZE
    }

    /// Returns the number of bytes of pages shared with, and counted by, another object.
    pub fn borrowed_bytes(&self) -> u64 {
        let locked = self.cow_pages.lock();
        self.cow_pages.borrowed_page_count_locked(&locked) * PAGE_SIZE
    }

    pub fn debug_get_page(&self, offset: u64) -> Option<VmPagePtr> {
        self.cow_pages.debug_get_page(offset)
    }
}

impl Drop for VmObjectPaged {
    fn drop(&mut self) {
        self.cow_pages.dead_transition();
    }
}

/// A zeroing of a range of an object in progress, which `zero_range_step` advances.
struct ZeroRange {
    offset: u64,
    len: u64,
    dirty_track: bool,
}

/// What a zeroing needs once `zero_range_step` returns.
enum ZeroRangeStep {
    /// Nothing: it is finished, with this result.
    Done(Result<(), Status>),
    /// The bytes from `start` to `end` of the page at `page_base` zeroed.
    Partial { page_base: u64, start: u64, end: u64 },
    /// A wait on this page request.
    Wait(MultiPageRequest),
}

/// A copy routine that zeroes the bytes it is given.
fn zero_bytes(dst: *mut u8, _offset: usize, len: usize) -> Result<(), UserCopyCaptureFaultsError> {
    // SAFETY: The copy routine is given a pointer to `len` bytes of a page.
    unsafe { std::ptr::write_bytes(dst, 0, len) };
    Ok(())
}

/// A copy in or out of a range of an object in progress, which `read_write_step` advances.
struct ReadWrite {
    offset: u64,
    len: usize,
    end_offset: u64,
    // Track our two offsets.
    src_offset: u64,
    dest_offset: usize,
    write: bool,
    options: VmObjectReadWriteOptions,
    page_request: MultiPageRequest,
}

impl ReadWrite {
    fn new(
        offset: u64,
        len: usize,
        write: bool,
        options: VmObjectReadWriteOptions,
    ) -> Result<Self, Status> {
        let end_offset = offset.checked_add(len as u64).ok_or(Status::OUT_OF_RANGE)?;
        Ok(Self {
            offset,
            len,
            end_offset,
            src_offset: offset,
            dest_offset: 0,
            write,
            options,
            // Ideally we can wake up early from the page request to begin processing any
            // partially supplied ranges. However, if performing a write to a dirty tracked VMO
            // this is not presently possible as we need to first read in the range and then dirty
            // it, and we cannot have both a read and dirty request outstanding at one time.
            page_request: MultiPageRequest::new(!write),
        })
    }

    fn copied(&self) -> usize {
        (self.src_offset - self.offset) as usize
    }

    /// The result of the copy after `resolved`, the result of the wait or fault resolution that
    /// `read_write_step` asked for, if the copy is finished.
    fn after_wait(&self, resolved: Result<(), Status>) -> Option<(Result<(), Status>, usize)> {
        if let Err(status) = resolved {
            return Some((Err(status), self.copied()));
        }
        (self.src_offset >= self.end_offset).then(|| (Ok(()), self.copied()))
    }
}

/// What a copy needs once `read_write_step` returns.
enum ReadWriteStep {
    /// Nothing: it is finished, with this status.
    Done(Result<(), Status>),
    /// The fault on the caller's memory captured while copying resolved, for the rest of the copy.
    Fault { info: FaultInfo, len: usize },
    /// A wait on its page request.
    Wait,
}
