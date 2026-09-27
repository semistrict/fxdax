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

use crate::kernel::range_check::{in_range, trim_range};
use crate::kernel::types::PAddr;
use crate::vm::anonymous_page_request::AnonymousPageRequest;
use crate::vm::fault::flag;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_source::{MultiPageRequest, PageSource, VmoDebugInfo};
use crate::vm::physmap::paddr_to_physmap;
use crate::vm::pmm::ALLOC_FLAG_CAN_WAIT;
use crate::vm::vm_cow_pages::{
    DeferredOps, LookupCursor, RequireResult, SupplyOptions, VmCowPages, VmCowPagesLocked,
    VmCowRange, options as cow_options,
};
use crate::vm::vm_page_list::{VmPageList, VmPageSpliceList};
use parking_lot::Mutex;
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
        if options & (Self::DISCARDABLE | Self::ALWAYS_PINNED) != 0 {
            return Err(Status::NOT_SUPPORTED);
        }
        if pmm_alloc_flags & ALLOC_FLAG_CAN_WAIT != 0 {
            options |= Self::CAN_BLOCK_ON_PAGE_REQUESTS;
        }
        // make sure size is page aligned
        if !page::is_aligned(size as usize) {
            return Err(Status::INVALID_ARGS);
        }
        if size > Self::MAX_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
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
        if !page::is_aligned(size as usize) {
            return Err(Status::INVALID_ARGS);
        }
        if size > Self::MAX_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
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

    pub fn size_locked(&self, locked: &VmCowPagesLocked) -> u64 {
        locked.size()
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
        if !in_range(offset, len, self.size()) {
            return Err(Status::OUT_OF_RANGE);
        }
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
                        if pin {
                            return Err(Status::OUT_OF_RANGE);
                        }
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

    pub fn debug_is_range_pinned(&self, offset: u64, len: u64) -> bool {
        let locked = self.cow_pages.lock();
        self.get_cow_range(offset, len)
            .is_some_and(|range| self.cow_pages.debug_is_range_pinned_locked(&locked, range))
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
        debug_assert!(zero_start_offset <= zero_end_offset);
        debug_assert!(zero_end_offset <= PAGE_SIZE);
        debug_assert!(page::is_aligned(page_base_offset as usize));
        {
            let locked = self.cow_pages.lock();
            if page_base_offset >= locked.size() {
                return Err(Status::OUT_OF_RANGE);
            }
            if self.cow_pages.page_would_read_zero_locked(&locked, page_base_offset) {
                // This is already considered zero so no need to redundantly zero again.
                return Ok(());
            }
        }
        // Need to actually zero out bytes in the page.
        self.read_write_internal(
            page_base_offset + zero_start_offset,
            (zero_end_offset - zero_start_offset) as usize,
            true,
            VmObjectReadWriteOptions::None,
            // SAFETY: The copy routine is given a pointer to a page's bytes.
            &mut |dst, _, len| unsafe { std::ptr::write_bytes(dst, 0, len) },
        )
        .0
    }

    pub fn zero_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        self.zero_range_internal(offset, len, true)
    }

    pub fn zero_range_untracked(&self, offset: u64, len: u64) -> Result<(), Status> {
        // We don't expect any committed pages to remain at the end of this call, so we should be
        // operating on whole pages.
        if !page::is_aligned(offset as usize) || !page::is_aligned(len as usize) {
            return Err(Status::INVALID_ARGS);
        }
        self.zero_range_internal(offset, len, false)
    }

    fn zero_range_internal(
        &self,
        mut offset: u64,
        mut len: u64,
        dirty_track: bool,
    ) -> Result<(), Status> {
        // May need to zero in chunks across multiple different lock acquisitions so loop until
        // nothing left to do.
        while len > 0 {
            // Check for any non-page aligned start and handle separately.
            if !page::is_aligned(offset as usize) {
                // We're doing partial page writes, so we should be dirty tracking.
                debug_assert!(dirty_track);
                let page_base = offset & !(PAGE_SIZE - 1);
                let zero_start_offset = offset - page_base;
                let zero_len = (PAGE_SIZE - zero_start_offset).min(len);
                self.zero_partial_page(page_base, zero_start_offset, zero_start_offset + zero_len)?;
                // Advance over the length we zeroed and then, since the lock might have been
                // dropped, go around the loop to redo the checks.
                offset += zero_len;
                len -= zero_len;
                continue;
            }
            // The start is page aligned, so if the remaining length is not a page size then
            // perform the final sub-page zero.
            if len < PAGE_SIZE {
                debug_assert!(dirty_track);
                return self.zero_partial_page(offset, 0, len);
            }
            let aligned_len = len & !(PAGE_SIZE - 1);

            // First try and do the more efficient decommit. We prefer decommit as it performs work
            // in the order of the number of committed pages, instead of work in the order of size
            // of the range. An error from DecommitRange indicates that the VMO is not of a form
            // that decommit can safely be performed without exposing data that we shouldn't, but
            // no actual state will have been changed.
            let cow_range = self.get_cow_range(offset, aligned_len).ok_or(Status::OUT_OF_RANGE)?;
            if self.cow_pages.decommit_range(cow_range).is_ok() {
                offset += cow_range.len;
                len -= cow_range.len;
                continue;
            }

            // We might need a page request if the VMO is backed by a page source.
            let mut page_request = MultiPageRequest::default();
            let (status, zeroed_len) = {
                let mut deferred = DeferredOps::new(&self.cow_pages);
                let mut locked = self.cow_pages.lock();
                // Offset is page aligned, and we have at least one full page to process, so find
                // the page aligned length to hand over to the cow pages zero method.
                let cow_range = self
                    .get_cow_range_size_check_locked(&locked, offset, aligned_len)
                    .ok_or(Status::OUT_OF_RANGE)?;
                let (status, zeroed_len) = self.cow_pages.zero_pages_locked(
                    &mut locked,
                    cow_range,
                    dirty_track,
                    &mut deferred,
                    &mut page_request,
                );
                if zeroed_len != 0 {
                    // Mark modified since we wrote zeros.
                    self.cow_pages.mark_modified_locked(&mut locked);
                }
                (status, zeroed_len)
            };
            // Wait on any page request, which is the only non-fatal error case.
            match status {
                Ok(()) => {}
                Err(Status::SHOULD_WAIT) => page_request.wait(true)?,
                Err(status) => return Err(status),
            }
            // Advance over pages that had already been zeroed.
            offset += zeroed_len;
            len -= zeroed_len;
        }
        Ok(())
    }

    pub fn resize(&self, s: u64) -> Result<(), Status> {
        if !self.is_resizable() {
            return Err(Status::UNAVAILABLE);
        }
        // ensure the size is valid and that we will not wrap.
        if !page::is_aligned(s as usize) {
            return Err(Status::INVALID_ARGS);
        }
        if s > Self::MAX_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
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
        copyfunc: &mut dyn FnMut(*mut u8, usize, usize),
    ) -> (Result<(), Status>, usize) {
        let Some(mut end_offset) = offset.checked_add(len as u64) else {
            return (Err(Status::OUT_OF_RANGE), 0);
        };
        // Track our two offsets.
        let mut src_offset = offset;
        let mut dest_offset = 0usize;
        let copied = |src_offset: u64| (src_offset - offset) as usize;

        // Ideally we can wake up early from the page request to begin processing any partially
        // supplied ranges. However, if performing a write to a dirty tracked VMO this is not
        // presently possible as we need to first read in the range and then dirty it, and we
        // cannot have both a read and dirty request outstanding at one time.
        let mut page_request = MultiPageRequest::new(!write);
        loop {
            let status = {
                let mut deferred = DeferredOps::new(&self.cow_pages);
                let mut locked = self.cow_pages.lock();
                let size = locked.size();
                if end_offset > size {
                    if options == VmObjectReadWriteOptions::TrimLength {
                        if src_offset >= size {
                            return (Ok(()), copied(src_offset));
                        }
                        end_offset = size;
                    } else {
                        return (Err(Status::OUT_OF_RANGE), copied(src_offset));
                    }
                } else if src_offset >= end_offset {
                    return (Ok(()), copied(src_offset));
                }

                let first_page_offset = src_offset & !(PAGE_SIZE - 1);
                let last_page_offset = (end_offset - 1) & !(PAGE_SIZE - 1);
                let mut remaining_pages = (last_page_offset - first_page_offset) / PAGE_SIZE + 1;
                let mut modified = false;

                let mut cursor = match self.get_lookup_cursor_locked(
                    &locked,
                    first_page_offset,
                    remaining_pages * PAGE_SIZE,
                ) {
                    Ok(cursor) => cursor,
                    Err(status) => return (Err(status), copied(src_offset)),
                };
                // Performing explicit accesses by request of the user, so disable zero forking.
                cursor.disable_zero_fork();

                let mut status = Ok(());
                while remaining_pages > 0 {
                    let page_offset = (src_offset % PAGE_SIZE) as usize;
                    let tocopy = (page::SIZE - page_offset).min((end_offset - src_offset) as usize);

                    // If we need to wait on pages then we would like to wait on as many as
                    // possible, up to the actual limit of the read/write operation. For a read we
                    // can wake up once some pages are received, minimizing the latency before we
                    // start making progress, but as this is not true for writes we cap the maximum
                    // number requested.
                    const MAX_WRITE_WAIT_PAGES: u64 = 256;
                    let max_wait_pages = if write { MAX_WRITE_WAIT_PAGES } else { u64::MAX };
                    let max_waitable_pages = remaining_pages.min(max_wait_pages);

                    // Attempt to lookup a page
                    let result = cursor.require_page(
                        &self.cow_pages,
                        &mut locked,
                        write,
                        max_waitable_pages,
                        &mut deferred,
                        &mut page_request,
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
                    // SAFETY: `page_offset` is within the page.
                    copyfunc(unsafe { page_ptr.add(page_offset) }, dest_offset, tocopy);
                    // Advance the copy location.
                    src_offset += tocopy as u64;
                    dest_offset += tocopy;
                    remaining_pages -= 1;
                    modified = write;
                }
                // Before dropping the lock, check if any pages were modified and update the VMO
                // state accordingly.
                if modified {
                    self.cow_pages.mark_modified_locked(&mut locked);
                }
                status
            };
            match status {
                Ok(()) => {}
                Err(Status::SHOULD_WAIT) => {
                    // RequirePage 'failed', but told us that it had filled out the page request,
                    // so we should wait on it.
                    debug_assert!(self.can_block_on_page_requests());
                    if let Err(status) = page_request.wait(true) {
                        return (Err(status), copied(src_offset));
                    }
                }
                Err(status) => return (Err(status), copied(src_offset)),
            }
            if src_offset >= end_offset {
                return (Ok(()), copied(src_offset));
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
                unsafe { std::ptr::copy_nonoverlapping(src, ptr.add(off), len) }
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
                unsafe { std::ptr::copy_nonoverlapping(ptr.add(off), dst, len) }
            },
        )
        .0
    }

    /// Reads `len` bytes at `offset` into the caller's memory at `ptr`, returning the status and
    /// how many bytes were read.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for writes of `len` bytes.
    pub unsafe fn read_user(
        &self,
        ptr: *mut u8,
        offset: u64,
        len: usize,
        options: VmObjectReadWriteOptions,
    ) -> (Result<(), Status>, usize) {
        self.read_write_internal(offset, len, false, options, &mut |src, off, len| {
            // SAFETY: Guaranteed by the caller.
            unsafe { std::ptr::copy(src, ptr.add(off), len) }
        })
    }

    /// Writes `len` bytes from the caller's memory at `ptr` at `offset`, returning the status and
    /// how many bytes were written.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid for reads of `len` bytes.
    pub unsafe fn write_user(
        &self,
        ptr: *const u8,
        offset: u64,
        len: usize,
        options: VmObjectReadWriteOptions,
    ) -> (Result<(), Status>, usize) {
        self.read_write_internal(offset, len, true, options, &mut |dst, off, len| {
            // SAFETY: Guaranteed by the caller.
            unsafe { std::ptr::copy(ptr.add(off), dst, len) }
        })
    }

    /// Performs a cache maintenance operation on the range. Host memory is coherent and VMOs are
    /// not executable, so only the range is checked.
    pub fn cache_op(&self, offset: u64, len: u64) -> Result<(), Status> {
        if len == 0 {
            return Err(Status::INVALID_ARGS);
        }
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
        if self.is_contiguous() {
            return Err(Status::NOT_SUPPORTED);
        }
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
        if self.is_contiguous() {
            return Err(Status::NOT_SUPPORTED);
        }
        let range = self.get_cow_range(offset, len).ok_or(Status::OUT_OF_RANGE)?;
        if range.is_empty() {
            return Ok(());
        }
        // There are no references in the splice list to convert into pages, as nothing is
        // compressed.
        let mut deferred = DeferredOps::new(&self.cow_pages);
        let mut locked = self.cow_pages.lock();
        self.cow_pages.supply_pages_locked(&mut locked, range, pages, options, &mut deferred)
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

    pub fn debug_get_page(&self, offset: u64) -> Option<VmPagePtr> {
        self.cow_pages.debug_get_page(offset)
    }
}

impl Drop for VmObjectPaged {
    fn drop(&mut self) {
        self.cow_pages.dead_transition();
    }
}
