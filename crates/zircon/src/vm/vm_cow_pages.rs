// Copyright 2020 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The pages of a VMO.
//!
//! Ported from `zircon/kernel/vm/vm_cow_pages.cc` and
//! `zircon/kernel/vm/include/vm/vm_cow_pages.h`.
//!
//! A `VmCowPages` holds the pages of a VMO in a `VmPageList`, fetches missing pages from its page
//! source, tracks the dirty state of pages backed by a user pager, and places its pages in the page
//! queues so they can be aged and reclaimed.
//!
//! Deviations:
//! - Only root nodes are supported: copy-on-write clones, hidden nodes and parent content are not
//!   ported. As a consequence a lookup's owner is always the target, and there are never children
//!   to apply range changes to.
//! - No compressed references, loaned pages, discardable VMOs, contiguous page sources or page
//!   caches. The high priority count is kept but nothing raises it yet.
//! - Methods that Zircon names `...Locked` take the locked state explicitly, as
//!   `&mut VmCowPagesLocked`.
//! - A page's backlink to its `VmCowPages` is a `Weak` reference, so the page queues can upgrade
//!   it safely; see `backlink`.
//! - No kcounters, ktrace or attribution tracking beyond counting committed pages.
//! - fxdax: a VMO's mappings are kept by their addresses, rather than in an intrusive list, so
//!   that removing one of many is quick (`VmCowPagesLocked::mappings`).
//! - fxdax: an object's pages are of the arena current when it was created (`pmm::with_arena`,
//!   `vm::physmap`, TASK-43): pages supplied to it of another arena's are copied into its own
//!   before they are supplied, without the lock (`process_pages_for_supply`). A page it borrows
//!   is of its lender's.
//! - Beyond Zircon, a pager-backed object can borrow the clean pages of another (`lend_pages`), so
//!   that objects with the same content share its memory. Unrelated to Zircon's loaned pages, which
//!   are physical pages a contiguous VMO lends the PMM. A borrowed page stays in its lender's page
//!   list, page queue and accounting, and is also in the borrower's page list, where its backlink,
//!   which is the lender, tells it apart from the borrower's own pages. Lent pages are always
//!   Clean and unpinned: the lender takes back its loans (`revoke_loans`) before it dirties, pins,
//!   evicts or otherwise removes one, and a borrower replaces a borrowed page with a copy of its
//!   own (`replace_borrowed_page_locked`) before writing, dirtying or pinning it. Everywhere else a
//!   borrower removes a borrowed page, it gives it back (`return_borrowed_page`) instead of freeing
//!   it. An object only ever lends or borrows, never both, so locks only nest lender then borrower.
//! - fxdax: the DIRTY requests of pages that could not be dirtied for want of memory fail with
//!   `NO_MEMORY`, rather than being resolved for their writers to try again for ever
//!   (`dirty_pages_failed_locked`, TASK-79.75).
//! - fxdax: an object can be held from eviction (`hold_from_eviction`), as a high priority count
//!   holds it, but without moving its pages between page queues, and without its lock
//!   (TASK-92.50).

use crate::debug::ltracef;
use crate::ensure;
use crate::sync::{LockId, rules};
use crate::vm::anonymous_page_request::AnonymousPageRequest;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use crate::vm::page_source::{
    LazyPageRequest, MultiPageRequest, PageRequest, PageSource, VmoDebugInfo,
};
use crate::vm::page_state::VmPageState;
use crate::vm::physmap::{ArenaId, paddr_to_physmap};
use crate::vm::pmm::{self, ALLOC_FLAG_CAN_WAIT, PmmOptDelayReuse};
use crate::vm::vm_get_zero_page;
use crate::vm::vm_page_list::{
    IntervalHandling, VmPageList, VmPageOrMarker, VmPageOrMarkerRef, VmPageSpliceList, VmplCursor,
    ZeroRangeDirtyState,
};
use parking_lot::{ArcMutexGuard, Mutex, MutexGuard, RawMutex};
use std::collections::{BTreeMap, HashMap};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;
const PAGE_SIZE: u64 = page::SIZE as u64;

fn is_page_rounded(value: u64) -> bool {
    value.is_multiple_of(PAGE_SIZE)
}

fn round_down_page_size(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

fn round_up_page_size(value: u64) -> u64 {
    (value + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Options a `VmCowPages` is created with.
pub mod options {
    pub const NONE: u32 = 0;
    pub const USER_PAGER_BACKED_ROOT: u32 = 1 << 0;
    pub const PAGE_SOURCE_ROOT: u32 = 1 << 1;
    /// With this clear, zeroing a page tries to decommit the page. With this set, zeroing never
    /// decommits the page.
    pub const CANNOT_DECOMMIT_ZERO_PAGES: u32 = 1 << 2;
}

/// A range of offsets in a `VmCowPages`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VmCowRange {
    pub offset: u64,
    pub len: u64,
}

impl VmCowRange {
    pub const fn new(offset: u64, len: u64) -> Self {
        Self { offset, len }
    }

    pub fn end(&self) -> u64 {
        self.offset + self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_page_aligned(&self) -> bool {
        is_page_rounded(self.offset) && is_page_rounded(self.len)
    }

    pub fn expand_till_page_aligned(&self) -> Self {
        let start = round_down_page_size(self.offset);
        Self::new(start, round_up_page_size(self.end()) - start)
    }

    pub fn trimmed_from_start(&self, amount: u64) -> Self {
        Self::new(self.offset + amount, self.len - amount)
    }

    /// Returns the minimal range that covers both `self` and `other`.
    pub fn cover(&self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let start = self.offset.min(other.offset);
        let end = self.end().max(other.end());
        Self::new(start, end - start)
    }

    /// Returns whether the range lies within [0, max) without overflowing.
    pub fn is_bounded_by(&self, max: u64) -> bool {
        self.offset.checked_add(self.len).is_some_and(|end| end <= max) && self.offset <= max
    }
}

/// How a page's contents relate to its page source. See `update_dirty_state_locked`.
///
/// The transitions between the three states can roughly be summarized as follows:
/// 1. A page starts off as Clean when supplied.
/// 2. A write transitions the page from Clean to Dirty.
/// 3. A writeback_begin moves the Dirty page to AwaitingClean.
/// 4. A writeback_end moves the AwaitingClean page to Clean.
/// 5. A write that comes in while the writeback is in progress (i.e. the page is AwaitingClean)
///    moves the AwaitingClean page back to Dirty.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyState {
    /// The page does not track dirty state. Used for non pager backed pages.
    Untracked = 0,
    /// The page is clean, i.e. its contents have not been altered from when the page was supplied.
    Clean,
    /// The page's contents have been modified from the time of supply, and should be written back
    /// to the page source at some point.
    Dirty,
    /// The page still has modified contents, but the page source is in the process of writing back
    /// the changes.
    AwaitingClean,
}

fn dirty_state(page: VmPagePtr) -> u8 {
    // SAFETY: The owner's lock is held by callers, which serializes the page's object state.
    unsafe { page.object().dirty_state() }
}

pub fn is_page_dirty_tracked(page: VmPagePtr) -> bool {
    dirty_state(page) != DirtyState::Untracked as u8
}

pub fn is_page_dirty(page: VmPagePtr) -> bool {
    dirty_state(page) == DirtyState::Dirty as u8
}

pub fn is_page_clean(page: VmPagePtr) -> bool {
    dirty_state(page) == DirtyState::Clean as u8
}

pub fn is_page_awaiting_clean(page: VmPagePtr) -> bool {
    dirty_state(page) == DirtyState::AwaitingClean as u8
}

fn pin_count(page: VmPagePtr) -> u8 {
    // SAFETY: As in `dirty_state`.
    unsafe { page.object().pin_count() }
}

/// Controls the type of slot in the page list that the add page functions can overwrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanOverwriteSlot {
    /// Only overwrite empty slots.
    Empty,
    /// Can overwrite parent content markers (in addition to empty slots).
    EmptyOrParent,
    /// Can overwrite zero page markers and zero intervals (in addition to the above).
    ZeroMarkerOrInterval,
    /// Can overwrite pages and compressed references, i.e. all types.
    PageOrRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSourceType {
    /// Not directly backed by a page source.
    Anonymous,
    /// Directly backed by a contiguous page source.
    Contiguous,
    /// Directly backed by a user pager, which implies dirty tracking.
    UserPager,
}

/// An operation to apply to the mappings of a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeChangeOp {
    Unmap,
    UnmapZeroPage,
    RemoveWrite,
    UnmapAndHarvest,
    DebugUnpin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionAction {
    FollowHint,
    IgnoreHint,
    /// fxdax: evicts whatever the page's age and hints, if it is clean and unpinned: memory an
    /// arena past its cap gives back, as chosen by what holds it (TASK-50.3).
    Demand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclaimType {
    Evict,
    Discard,
    Compress,
}

/// The result of successfully reclaiming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmCowReclaimSuccess {
    pub r#type: ReclaimType,
    pub num_pages: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmCowReclaimFailure {
    EvictAccessed,
    CompressAccessed,
    CompressFailed,
    IncorrectPage,
    Other,
}

pub type VmCowReclaimResult = Result<VmCowReclaimSuccess, VmCowReclaimFailure>;

/// How a `VmCowPages` distinguishes the VMOs supplied by `SupplyPages`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupplyOptions {
    PagerSupply,
    TransferData,
    PhysicalPageProvider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifeCycle {
    Init,
    Alive,
    Dying,
    Dead,
}

/// Something with mappings of a `VmCowPages` range that must be updated when content changes,
/// namely the mappings of the VMO. Implemented by the VM mapping layer.
pub trait RangeChangeListener: Send + Sync {
    /// Applies `op` to the mappings of `range`, with the lock held, whose state `locked` is: what
    /// the mapping keeps that the lock guards is reached by it.
    fn range_change_update(&self, locked: &VmCowPagesLocked, range: VmCowRange, op: RangeChangeOp);
}

/// State of a `VmCowPages` protected by its lock.
pub struct VmCowPagesLocked {
    size: u64,
    /// Counts the total number of pages pinned by commit. If one page is pinned n times, it
    /// contributes n to this count.
    pinned_page_count: u64,
    /// Count reclamation events so that we can report them to the user.
    reclamation_event_count: u64,
    /// a tree of pages
    page_list: VmPageList,
    /// Count of how many references to this VMO are requesting this be high priority. If this is
    /// >0 then any kind of reclamation will be disabled.
    high_priority_count: i64,
    /// Whether this VMO was modified (written / resized) if backed by a pager.
    pager_stats_modified: bool,
    life_cycle: LifeCycle,
    /// Information about the VMO these are the pages of, for page requests.
    vmo_debug_info: VmoDebugInfo,
    /// Mappings of these pages. Zircon reaches them through the VmObjectPaged backlink, in an
    /// intrusive list. fxdax: by their addresses, so that one of many is removed at once (a DAX
    /// window maps a file in as many pieces as its extents, TASK-53).
    mappings: BTreeMap<usize, Arc<dyn RangeChangeListener>>,
}

impl VmCowPagesLocked {
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn pinned_page_count(&self) -> u64 {
        self.pinned_page_count
    }

    pub fn set_vmo_debug_info(&mut self, info: VmoDebugInfo) {
        self.vmo_debug_info = info;
    }

    pub fn add_mapping(&mut self, mapping: Arc<dyn RangeChangeListener>) {
        let added = self.mappings.insert(mapping_key(&mapping), mapping);
        assert!(added.is_none(), "adding a mapping twice");
    }

    pub fn remove_mapping(&mut self, mapping: &Arc<dyn RangeChangeListener>) {
        let removed = self.mappings.remove(&mapping_key(mapping));
        assert!(removed.is_some(), "removing a mapping that was not added");
    }

    pub fn num_mappings(&self) -> usize {
        self.mappings.len()
    }
}

/// A mapping's key among its VMO's: its address, which is its own while it lives.
fn mapping_key(mapping: &Arc<dyn RangeChangeListener>) -> usize {
    Arc::as_ptr(mapping).cast::<()>() as usize
}

/// The pages of a VMO.
pub struct VmCowPages {
    pmm_alloc_flags: u32,
    /// fxdax: the arena its pages are of: the one current when it was created (`pmm::with_arena`).
    arena: ArenaId,
    options: u32,
    /// The page source, if any.
    page_source: Option<Arc<PageSource>>,
    /// Lets the pages point back at this object.
    this: Weak<VmCowPages>,
    /// A one-way bool indicating whether this object has ever held pinned pages.
    ever_pinned: AtomicBool,
    /// Whether this object lends or borrows pages, a `Sharing`.
    sharing: AtomicU8,
    /// The pages this object lends.
    loans: Mutex<Loans>,
    /// The number of loans in `loans`, so that changing a page need not lock them when there are
    /// none. Loans are only made with our lock held, so with it held this can only decrease.
    loan_count: AtomicUsize,
    /// fxdax: how many holds keep its pages from eviction ([`VmCowPages::hold_from_eviction`]):
    /// while any is held, eviction takes none of them, as with a high priority count, but its pages
    /// stay in the page queues they are in. Counted without the lock, which a thread letting go of
    /// a hold may not take: one waiting with it held, as an eviction unmapping the pages from a
    /// second stage in another process does, may wait for the thread itself (TASK-92.50).
    eviction_holds: AtomicU64,
    lock: Mutex<VmCowPagesLocked>,
}

/// How an object takes part in sharing pages. Fixed by its first loan, so that an object never both
/// lends and borrows.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sharing {
    None,
    Lender,
    Borrower,
}

/// A page lent to `borrower`, which holds it at `offset`.
struct Loan {
    borrower: Weak<VmCowPages>,
    offset: u64,
}

/// The loans of a lender's pages, by the pages' offsets. The lock is a leaf: it is taken with the
/// lender's or a borrower's lock held, and nothing is locked under it.
#[derive(Default)]
struct Loans {
    by_offset: HashMap<u64, Vec<Loan>>,
}

/// Operations that must be performed after the lock is dropped, and serialization of operations on
/// a pager-backed hierarchy. Construct before taking the lock, so it is dropped after it:
///
/// ```ignore
/// let mut deferred = DeferredOps::new(&cow);
/// let mut locked = cow.lock();
/// ```
pub struct DeferredOps {
    cow: Arc<VmCowPages>,
    /// Pages to free after the operation.
    freed_list: VmPageDoublyLinkedList,
    /// Serializes operations on a pager-backed hierarchy, including these deferred operations.
    _page_source_lock: Option<ArcMutexGuard<RawMutex, ()>>,
}

impl DeferredOps {
    pub fn new(cow: &Arc<VmCowPages>) -> Self {
        // If we are referencing a pager backed object then we must acquire the pager hierarchy
        // lock. Only roots are supported, so the page source is our own.
        let page_source_lock = cow.page_source.as_ref().map(|source| source.paged_vmo_lock());
        Self {
            cow: cow.clone(),
            freed_list: VmPageDoublyLinkedList::new(),
            _page_source_lock: page_source_lock,
        }
    }

    fn freed_list(&mut self) -> &mut VmPageDoublyLinkedList {
        &mut self.freed_list
    }
}

impl Drop for DeferredOps {
    fn drop(&mut self) {
        // There are no children, so there are no range changes to perform on them. The pages must
        // be freed before dropping the page source lock.
        if !self.freed_list.is_empty() {
            self.cow.free_pages(&mut self.freed_list);
        }
    }
}

/// Collects pages removed from `cow` to remove from the page queues in batches, placing them in a
/// freed list. Pages `cow` borrowed are given back instead, and loans of its own pages taken back.
struct BatchPqRemove<'a> {
    cow: &'a VmCowPages,
    pages: Vec<VmPagePtr>,
    freed_list: &'a mut VmPageDoublyLinkedList,
    freed_count: usize,
}

impl<'a> BatchPqRemove<'a> {
    const MAX_PAGES: usize = 64;

    fn new(cow: &'a VmCowPages, freed_list: &'a mut VmPageDoublyLinkedList) -> Self {
        Self { cow, pages: Vec::with_capacity(Self::MAX_PAGES), freed_list, freed_count: 0 }
    }

    /// Pushes `page`, which was removed from `offset`.
    fn push(&mut self, offset: u64, page: VmPagePtr) {
        if self.cow.is_borrowed(page) {
            self.cow.return_borrowed_page(offset, page);
            return;
        }
        self.cow.revoke_loans(offset, page);
        assert_eq!(pin_count(page), 0);
        self.pages.push(page);
        if self.pages.len() == Self::MAX_PAGES {
            self.flush();
        }
    }

    /// Removes any content from the slot at `offset` and pushes it, leaving the slot empty.
    fn push_content(&mut self, slot: &mut VmPageOrMarker, offset: u64) {
        if slot.is_page() {
            self.push(offset, slot.release_page());
        } else {
            *slot = VmPageOrMarker::empty();
        }
    }

    fn flush(&mut self) {
        if !self.pages.is_empty() {
            page_queues().remove_array_into_list(&self.pages, self.freed_list);
            self.freed_count += self.pages.len();
            self.pages.clear();
        }
    }
}

impl Drop for BatchPqRemove<'_> {
    fn drop(&mut self) {
        debug_assert!(self.pages.is_empty(), "BatchPqRemove dropped without flushing");
    }
}

fn page_queues() -> &'static PageQueues {
    pmm::node().page_queues()
}

/// fxdax: the arena of physical memory `page` is of.
fn arena_of(page: VmPagePtr) -> ArenaId {
    pmm::node().phys_memory().location(page.paddr()).arena
}

/// fxdax: allocates `count` pages of `arena`, waiting for each the PMM will not give at once, as a
/// page fault would: all of them, or, if an allocation fails, none. Must be called without locks
/// held.
fn alloc_pages_waiting(arena: ArenaId, count: usize) -> Result<VmPageDoublyLinkedList, Status> {
    let alloc = |list: &mut VmPageDoublyLinkedList| {
        let mut left = count;
        while left > 0 {
            match pmm::node().alloc_pages_in(arena, left, ALLOC_FLAG_CAN_WAIT, list) {
                Ok(()) => return Ok(()),
                Err(Status::SHOULD_WAIT) => {
                    let mut request = AnonymousPageRequest::new();
                    request.make_active(arena);
                    request.allocate()?;
                    list.push_back(request.take_page().expect("the request allocated a page"));
                    left -= 1;
                }
                Err(status) => return Err(status),
            }
        }
        Ok(())
    };
    let mut pages = VmPageDoublyLinkedList::new();
    if let Err(status) = alloc(&mut pages) {
        // SAFETY: The pages were allocated above, and are this function's.
        unsafe { pmm::free_list(&mut pages) };
        return Err(status);
    }
    Ok(pages)
}

/// Prepares a page allocated from the PMM for use in an object.
fn initialize_vm_page(page: VmPagePtr) {
    // SAFETY: The page was just allocated and is owned by the caller.
    unsafe {
        debug_assert!(!page.in_container());
        // Page should be in the ALLOC state so we can transition it to the OBJECT state.
        debug_assert_eq!(page.state(), VmPageState::Alloc);
        page.set_state(VmPageState::Object);
        let object = page.object();
        object.share_count = 0;
        object.set_pin_count(0);
        object.set_always_need(false);
        object.set_dirty_state(DirtyState::Untracked as u8);
        object.set_object(std::ptr::null_mut());
        object.set_page_offset(0);
    }
    page.page_queue().store(0, Ordering::Relaxed);
}

/// Fills a page with zeroes.
pub fn zero_page(page: VmPagePtr) {
    // SAFETY: The physmap has a page at the page's address.
    unsafe { std::ptr::write_bytes(paddr_to_physmap(page.paddr()), 0, page::SIZE) };
}

/// Copies the contents of `src` to `dst`.
fn copy_page(dst: VmPagePtr, src: VmPagePtr) {
    debug_assert_ne!(dst, src);
    // SAFETY: The physmap has a page at each page's address, and they are different pages.
    unsafe {
        std::ptr::copy_nonoverlapping(
            paddr_to_physmap(src.paddr()),
            paddr_to_physmap(dst.paddr()),
            page::SIZE,
        )
    };
}

fn is_zero_page(page: VmPagePtr) -> bool {
    let base = paddr_to_physmap(page.paddr());
    // SAFETY: The physmap has a page at `base`.
    let bytes = unsafe { std::slice::from_raw_parts(base, page::SIZE) };
    bytes.iter().all(|b| *b == 0)
}

/// The result of a successful lookup: a page and whether it may be written to.
#[derive(Clone, Copy, Debug)]
pub struct RequireResult {
    pub page: VmPagePtr,
    pub writable: bool,
}

/// Retrieves successive pages over a range of a VMO. The VMO lock must be held continuously from
/// `get_lookup_cursor_locked` over the entire use of this object.
pub struct LookupCursor {
    /// The current offset. Only allowed to increase.
    offset: u64,
    /// The offset at which the cursor ceases being valid.
    end_offset: u64,
    /// Cursor in the page list at `offset`, valid only when `is_valid`.
    cursor: VmplCursor,
    /// Whether the target is directly backed by a user pager.
    target_directly_backed_by_user_pager: bool,
    /// Whether zero forks should be placed in the zero fork queue.
    zero_fork: bool,
    /// Whether existing pages should have their access time updated when they are returned.
    mark_accessed: bool,
    is_valid: bool,
    /// Optional list of pages to use for any allocations.
    alloc_list: Option<*mut VmPageDoublyLinkedList>,
}

impl LookupCursor {
    fn new(target: &VmCowPages, range: VmCowRange) -> Self {
        let user_pager = target.page_source_type() == PageSourceType::UserPager;
        Self {
            offset: range.offset,
            end_offset: range.end(),
            cursor: VmplCursor::default(),
            target_directly_backed_by_user_pager: user_pager,
            zero_fork: !user_pager && target.can_decommit_zero_pages(),
            mark_accessed: true,
            is_valid: false,
            alloc_list: None,
        }
    }

    /// Provides a list of pages that can be used to service any allocations. `clear_alloc_list`
    /// must be called before the list is dropped.
    pub fn give_alloc_list(&mut self, alloc_list: &mut VmPageDoublyLinkedList) {
        self.alloc_list = Some(alloc_list);
    }

    pub fn clear_alloc_list(&mut self) {
        debug_assert!(self.alloc_list.is_some());
        self.alloc_list = None;
    }

    fn alloc_list(&mut self) -> Option<&mut VmPageDoublyLinkedList> {
        // SAFETY: The alloc list outlives the cursor's use of it; see `give_alloc_list`.
        self.alloc_list.map(|list| unsafe { &mut *list })
    }

    /// Disables placing newly allocated zero pages in the zero fork list.
    pub fn disable_zero_fork(&mut self) {
        self.zero_fork = false;
    }

    /// Indicates that any existing pages that are returned should not be considered accessed.
    pub fn disable_mark_accessed(&mut self) {
        self.mark_accessed = false;
    }

    fn invalidate_cursor(&mut self) {
        self.is_valid = false;
    }

    fn increment_offset_and_invalidate_cursor(&mut self, delta: u64) {
        self.offset += delta;
        self.invalidate_cursor();
    }

    fn increment_cursor(&mut self) {
        self.offset += PAGE_SIZE;
        if self.offset == self.end_offset {
            self.invalidate_cursor();
        } else {
            self.cursor.step();
            // An empty cursor means the next lookup must find the node again.
            if self.cursor.current().is_none() {
                self.invalidate_cursor();
            }
        }
    }

    fn establish_cursor(&mut self, locked: &mut VmCowPagesLocked) {
        if self.is_valid {
            return;
        }
        debug_assert!(self.offset < self.end_offset);
        // Only roots are supported, so the target always owns the content.
        self.cursor = locked.page_list.lookup_mutable_cursor(self.offset);
        self.is_valid = true;
    }

    fn slot(&self) -> Option<&mut VmPageOrMarker> {
        self.cursor.current()
    }

    fn cursor_is_page(&self) -> bool {
        self.slot().is_some_and(|s| s.is_page())
    }

    fn cursor_is_marker(&self) -> bool {
        self.slot().is_some_and(|s| s.is_marker())
    }

    fn cursor_is_empty(&self) -> bool {
        self.slot().is_none_or(|s| s.is_empty())
    }

    fn cursor_is_interval_zero(&self) -> bool {
        self.slot().is_some_and(|s| s.is_interval_zero())
    }

    /// Checks if the offset, not just the slot, is in a zero interval.
    fn cursor_is_in_interval_zero(&self, locked: &VmCowPagesLocked) -> bool {
        self.cursor_is_interval_zero() || locked.page_list.is_offset_in_zero_interval(self.offset)
    }

    /// The cursor has zero content if it points at a zero marker, or it is empty and content is
    /// initially zero, which is the case without a page source or in a zero interval.
    fn cursor_is_content_zero(&self, target: &VmCowPages, locked: &VmCowPagesLocked) -> bool {
        if self.cursor_is_marker() {
            return true;
        }
        if target.page_source.is_some() {
            return self.cursor_is_in_interval_zero(locked);
        }
        self.cursor_is_empty()
    }

    fn cursor_is_usable_page(&self, writing: bool) -> bool {
        self.cursor_is_page() && (!writing || !self.target_directly_backed_by_user_pager)
    }

    /// Whether zero content at the cursor should be supplied as dirty.
    fn target_zero_content_supply_dirty(&self, locked: &VmCowPagesLocked, writing: bool) -> bool {
        if !self.target_directly_backed_by_user_pager {
            return false;
        }
        if writing {
            return true;
        }
        // Markers start clean
        if self.cursor_is_marker() {
            return false;
        }
        // Zero intervals are considered implicitly dirty and allocating them, even for reading,
        // causes them to be supplied as new dirty pages.
        debug_assert!(self.cursor_is_in_interval_zero(locked));
        true
    }

    fn page_as_result(&self, page: VmPagePtr) -> RequireResult {
        // The page is writable if it does not need a dirty transition. Only user pager backed
        // objects borrow pages, which are always Clean, so a borrowed page is never writable.
        RequireResult {
            page,
            writable: !self.target_directly_backed_by_user_pager || is_page_dirty(page),
        }
    }

    /// Turns the current cursor, which must be a page, into a result and increments the cursor.
    fn cursor_as_result(&mut self) -> RequireResult {
        let page = self.slot().unwrap().page();
        if self.mark_accessed {
            page_queues().mark_accessed(page);
        }
        let result = self.page_as_result(page);
        self.increment_cursor();
        result
    }

    /// Allocates a zero page for the target at the current offset, and increments the cursor.
    fn target_allocate_zero_page_as_result(
        &mut self,
        target: &VmCowPages,
        locked: &mut VmCowPagesLocked,
        dirty_state: DirtyState,
        deferred: &mut DeferredOps,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<RequireResult, Status> {
        let out_page = target.allocate_zero_page(self.alloc_list(), page_request)?;

        // We could be allocating a page to replace a zero page marker in a pager-backed VMO. If so
        // then set its dirty state to what was requested.
        if self.target_directly_backed_by_user_pager {
            target.update_dirty_state_locked(
                out_page,
                self.offset,
                dirty_state,
                true,
                locked.high_priority_count,
            );
        }

        // Reuse the cursor's slot if it exists, unless the target may have intervals that would
        // need splitting.
        let can_reuse_slot =
            self.slot().is_some() && target.page_source_type() != PageSourceType::UserPager;
        let transaction = if can_reuse_slot {
            let slot = self.cursor.current().unwrap();
            target.begin_add_page_with_slot_locked(
                locked,
                self.offset,
                slot,
                CanOverwriteSlot::ZeroMarkerOrInterval,
            )
        } else {
            target.begin_add_page_locked(
                locked,
                self.offset,
                CanOverwriteSlot::ZeroMarkerOrInterval,
            )
        };
        let mut transaction = match transaction {
            Ok(transaction) => transaction,
            Err(status) => {
                target.free_page(out_page);
                return Err(status);
            }
        };
        // Skip range change updates since we're forking the zero page, and use a more optimal
        // unmap request ourselves.
        let old = target.complete_add_page_locked(
            locked,
            &mut transaction,
            VmPageOrMarker::from_page(out_page),
            None,
        );
        debug_assert!(!old.is_page_or_ref());
        target.range_change_update_locked(
            locked,
            VmCowRange::new(self.offset, PAGE_SIZE),
            RangeChangeOp::UnmapZeroPage,
            Some(deferred),
        );

        // If asked to explicitly mark zero forks, move to the correct queue.
        if self.zero_fork {
            page_queues().move_anonymous_to_anonymous_zero_fork(out_page);
        }

        // Increment the cursor, re-establishing it if the page list may have been modified.
        if can_reuse_slot {
            debug_assert!(self.cursor_is_page());
            self.increment_cursor();
        } else {
            self.increment_offset_and_invalidate_cursor(PAGE_SIZE);
        }
        Ok(self.page_as_result(out_page))
    }

    /// Generates a read request for up to `max_request_pages` of absent content.
    fn read_request(
        &self,
        target: &VmCowPages,
        locked: &VmCowPagesLocked,
        max_request_pages: u64,
        page_request: &mut PageRequest,
    ) -> Status {
        let source = target.page_source.as_ref().expect("read request needs a page source");
        // The cursor should be explicitly empty as read requests are only for complete content
        // absence.
        debug_assert!(self.cursor_is_empty());
        debug_assert!(!self.cursor_is_in_interval_zero(locked));
        debug_assert!(self.offset + PAGE_SIZE * max_request_pages <= self.end_offset);
        debug_assert!(max_request_pages > 0);

        // Try and batch more pages up to max_request_pages, limited to the first page present to
        // avoid requesting pages that are already present.
        let mut request_size = max_request_pages * PAGE_SIZE;
        if request_size > PAGE_SIZE {
            let _ = locked.page_list.for_every_page_in_range(
                self.offset,
                self.offset + request_size,
                |p, offset| {
                    // Content should have been empty initially.
                    debug_assert!(offset > self.offset);
                    // If this is an interval sentinel, it can only be a start or slot.
                    debug_assert!(
                        !p.is_interval() || p.is_interval_slot() || p.is_interval_start()
                    );
                    request_size = offset - self.offset;
                    Status::STOP
                },
            );
        }
        debug_assert!(request_size >= PAGE_SIZE);
        // Pager page sources will never synchronously return a page, so this is always an error.
        source.get_pages(self.offset, request_size, page_request, locked.vmo_debug_info)
    }

    /// Generates a dirty request, or dirties pages without one if dirty transitions are not
    /// trapped.
    fn dirty_request(
        &self,
        target: &VmCowPages,
        locked: &mut VmCowPagesLocked,
        max_request_pages: u64,
        page_request: &mut LazyPageRequest,
    ) -> Result<(), Status> {
        debug_assert!(target.page_source.is_some());
        debug_assert!(max_request_pages > 0);
        debug_assert!(self.offset + PAGE_SIZE * max_request_pages <= self.end_offset);
        let mut dirty_len = 0;
        let result = target.prepare_for_write_locked(
            locked,
            VmCowRange::new(self.offset, PAGE_SIZE * max_request_pages),
            page_request,
            &mut dirty_len,
        );
        if result.is_ok() {
            // If success is claimed then at least one page was dirtied, allowing progress.
            debug_assert!(dirty_len != 0 && dirty_len <= max_request_pages * PAGE_SIZE);
        } else {
            debug_assert_eq!(dirty_len, 0);
        }
        result
    }

    /// Checks the current slot for a page that is usable without any transitions, and returns it.
    /// The cursor is always incremented.
    pub fn maybe_page(
        &mut self,
        locked: &mut VmCowPagesLocked,
        will_write: bool,
    ) -> Option<VmPagePtr> {
        self.establish_cursor(locked);
        let page = if self.cursor_is_usable_page(will_write) {
            Some(self.slot().unwrap().page())
        } else {
            None
        };
        if let Some(page) = page
            && self.mark_accessed
        {
            page_queues().mark_accessed(page);
        }
        self.increment_cursor();
        page
    }

    /// Returns how many times in a row `maybe_page` would have returned nothing, incrementing the
    /// cursor by that many pages.
    pub fn skip_missing_pages(&mut self, locked: &mut VmCowPagesLocked) -> u64 {
        self.establish_cursor(locked);
        if !self.cursor_is_empty() || self.cursor_is_in_interval_zero(locked) {
            return 0;
        }
        let mut possibly_empty = self.end_offset - self.offset;
        if possibly_empty > PAGE_SIZE {
            let _ = locked.page_list.for_every_page_in_range(
                self.offset,
                self.offset + possibly_empty,
                |p, offset| {
                    debug_assert!(offset > self.offset);
                    debug_assert!(
                        !p.is_interval() || p.is_interval_slot() || p.is_interval_start()
                    );
                    possibly_empty = offset - self.offset;
                    Status::STOP
                },
            );
        }
        debug_assert!(possibly_empty >= PAGE_SIZE && is_page_rounded(possibly_empty));
        self.increment_offset_and_invalidate_cursor(possibly_empty);
        possibly_empty / PAGE_SIZE
    }

    /// Returns up to `max_pages` physical addresses of contiguous pages that are usable right now
    /// without transitions, incrementing the cursor by that many.
    pub fn if_exist_pages(
        &mut self,
        locked: &mut VmCowPagesLocked,
        will_write: bool,
        max_pages: u64,
        paddrs: &mut [crate::kernel::types::PAddr],
    ) -> u64 {
        debug_assert!(self.offset + PAGE_SIZE * max_pages <= self.end_offset);
        self.establish_cursor(locked);
        // We only return actual pages that are ready to use right now without needing to mark
        // them accessed.
        if !self.cursor_is_usable_page(will_write) || self.mark_accessed {
            return 0;
        }
        let mut pages = 0u64;
        let _ = self.cursor.for_every_contiguous(|slot| {
            if slot.is_page() {
                paddrs[pages as usize] = slot.page_as_paddr();
                pages += 1;
                return if pages == max_pages { Status::STOP } else { Status::NEXT };
            }
            Status::STOP
        });
        self.increment_offset_and_invalidate_cursor(pages * PAGE_SIZE);
        pages
    }

    /// Returns a page owned by the target, performing a dirty transition if `will_write`.
    pub fn require_owned_page(
        &mut self,
        target: &VmCowPages,
        locked: &mut VmCowPagesLocked,
        will_write: bool,
        max_request_pages: u64,
        deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> Result<RequireResult, Status> {
        self.establish_cursor(locked);

        // If the page exists, we might need to dirty it.
        if self.cursor_is_page() {
            // A borrowed page is not owned by the target, which needs a copy of its own.
            if target.is_borrowed(self.slot().unwrap().page()) {
                target.replace_borrowed_page_locked(
                    locked,
                    self.offset,
                    self.alloc_list(),
                    deferred,
                    page_request.get_anonymous(),
                )?;
                self.invalidate_cursor();
                self.establish_cursor(locked);
            }
            // If we're writing to a root VMO backed by a user pager, we might need to mark pages
            // Dirty so that they can be written back later.
            if will_write
                && self.target_directly_backed_by_user_pager
                && !is_page_dirty(self.slot().unwrap().page())
            {
                let status = self.dirty_request(
                    target,
                    locked,
                    max_request_pages,
                    page_request.get_lazy_dirty_request(),
                );
                if let Err(status) = status {
                    if status == Status::SHOULD_WAIT {
                        page_request.made_dirty_request();
                    }
                    return Err(status);
                }
            }
            return Ok(self.cursor_as_result());
        }

        // Zero content is the most complicated case where, even if reading, dirty requests might
        // need to be performed and the resulting committed pages may / may not be dirty.
        if self.cursor_is_content_zero(target, locked) {
            // If the page source traps dirty transitions, we first need to generate a DIRTY request
            // *before* the zero page can be forked and marked dirty.
            let target_page_dirty = self.target_zero_content_supply_dirty(locked, will_write);
            if target_page_dirty
                && target.page_source.as_ref().unwrap().should_trap_dirty_transitions()
            {
                let status = self
                    .dirty_request(
                        target,
                        locked,
                        max_request_pages,
                        page_request.get_lazy_dirty_request(),
                    )
                    .expect_err("page sources never succeed synchronously");
                if status == Status::SHOULD_WAIT {
                    page_request.made_dirty_request();
                }
                return Err(status);
            }
            // Allocate the page and mark it dirty or clean as previously determined.
            let state = if target_page_dirty { DirtyState::Dirty } else { DirtyState::Clean };
            return self.target_allocate_zero_page_as_result(
                target,
                locked,
                state,
                deferred,
                page_request.get_anonymous(),
            );
        }
        debug_assert!(self.cursor_is_empty());

        // Generate a read request to populate the content. Even if this is a write, we still
        // populate content first, then perform any dirty transitions / requests.
        Err(self.read_request(target, locked, max_request_pages, page_request.get_read_request()))
    }

    /// Returns a page that will only be read from, which may be the zero page.
    pub fn require_read_page(
        &mut self,
        target: &VmCowPages,
        locked: &mut VmCowPagesLocked,
        max_request_pages: u64,
        _deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> Result<RequireResult, Status> {
        self.establish_cursor(locked);
        if self.cursor_is_page() {
            return Ok(self.cursor_as_result());
        }
        if self.cursor_is_content_zero(target, locked) {
            self.increment_cursor();
            return Ok(RequireResult { page: vm_get_zero_page(), writable: false });
        }
        // No available content, need to fetch it from the page source.
        Err(self.read_request(target, locked, max_request_pages, page_request.get_read_request()))
    }

    /// Returns a page readable or writable based on `will_write`.
    pub fn require_page(
        &mut self,
        target: &VmCowPages,
        locked: &mut VmCowPagesLocked,
        will_write: bool,
        max_request_pages: u64,
        deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> Result<RequireResult, Status> {
        if will_write {
            self.require_owned_page(target, locked, true, max_request_pages, deferred, page_request)
        } else {
            self.require_read_page(target, locked, max_request_pages, deferred, page_request)
        }
    }
}

/// A pending insertion of a page, split into a fallible check and slot allocation and an
/// infallible completion.
pub struct AddPageTransaction {
    slot: *mut VmPageOrMarker,
    offset: u64,
}

impl VmCowPages {
    fn new(
        options: u32,
        pmm_alloc_flags: u32,
        size: u64,
        page_source: Option<Arc<PageSource>>,
    ) -> Arc<Self> {
        debug_assert!(is_page_rounded(size));
        Arc::new_cyclic(|this| Self {
            pmm_alloc_flags,
            arena: pmm::current_arena(),
            options,
            page_source,
            this: this.clone(),
            ever_pinned: AtomicBool::new(false),
            sharing: AtomicU8::new(Sharing::None as u8),
            loans: Mutex::new(Loans::default()),
            loan_count: AtomicUsize::new(0),
            eviction_holds: AtomicU64::new(0),
            lock: Mutex::new(VmCowPagesLocked {
                size,
                pinned_page_count: 0,
                reclamation_event_count: 0,
                page_list: VmPageList::new(),
                high_priority_count: 0,
                pager_stats_modified: false,
                life_cycle: LifeCycle::Init,
                vmo_debug_info: VmoDebugInfo::default(),
                mappings: BTreeMap::new(),
            }),
        })
    }

    /// Creates anonymous pages.
    pub fn create(options: u32, pmm_alloc_flags: u32, size: u64) -> Arc<Self> {
        Self::new(options, pmm_alloc_flags, size, None)
    }

    /// Creates pages backed by a page source.
    pub fn create_external(src: Arc<PageSource>, options: u32, size: u64) -> Arc<Self> {
        Self::new(options, ALLOC_FLAG_CAN_WAIT, size, Some(src))
    }

    /// Acquires the lock. Construct any `DeferredOps` first.
    pub fn lock(&self) -> MutexGuard<'_, VmCowPagesLocked> {
        self.lock.lock()
    }

    /// The lock, as the state of its mappings that it guards knows it (`LockedBy`).
    pub(crate) fn lock_id(&self) -> LockId<VmCowPagesLocked> {
        LockId::of(&self.lock)
    }

    /// Returns a weak reference to store in a page's backlink. The page queues release it when the
    /// page is removed.
    fn backlink(&self) -> *mut () {
        Weak::into_raw(self.this.clone()).cast_mut().cast()
    }

    fn is_backlink(&self, object: *mut ()) -> bool {
        std::ptr::eq(object.cast::<Self>(), self)
    }

    pub fn transition_to_alive_locked(&self, locked: &mut VmCowPagesLocked) {
        assert_eq!(locked.life_cycle, LifeCycle::Init);
        locked.life_cycle = LifeCycle::Alive;
    }

    pub fn is_root_source_user_pager_backed(&self) -> bool {
        (self.options & options::USER_PAGER_BACKED_ROOT) != 0
    }

    pub fn root_has_page_source(&self) -> bool {
        (self.options & options::PAGE_SOURCE_ROOT) != 0
    }

    fn can_decommit_zero_pages(&self) -> bool {
        (self.options & options::CANNOT_DECOMMIT_ZERO_PAGES) == 0
    }

    pub fn can_evict(&self) -> bool {
        self.page_source.as_ref().is_some_and(|s| s.properties().is_user_pager)
    }

    /// fxdax: the arena its pages are of, the one current when it was created (`pmm::with_arena`).
    pub fn arena(&self) -> ArenaId {
        self.arena
    }

    pub fn is_discardable(&self) -> bool {
        false
    }

    pub fn is_dirty_tracked(&self) -> bool {
        self.can_evict()
    }

    /// Parent content markers are used if there is no page source. Only roots are supported, so
    /// they never appear, but empty slots are known to be zero.
    fn node_has_parent_content_markers(&self) -> bool {
        !self.root_has_page_source()
    }

    pub fn page_source(&self) -> Option<&Arc<PageSource>> {
        self.page_source.as_ref()
    }

    pub fn page_source_type(&self) -> PageSourceType {
        match &self.page_source {
            None => PageSourceType::Anonymous,
            Some(source) if source.properties().is_user_pager => PageSourceType::UserPager,
            Some(_) => PageSourceType::Contiguous,
        }
    }

    pub fn get_page_source_koid(&self) -> Option<u64> {
        self.page_source.as_ref().and_then(|s| s.get_provider_koid())
    }

    /// Marks the VMO modified, for pager VMO stats.
    pub fn mark_modified_locked(&self, locked: &mut VmCowPagesLocked) {
        if self.is_dirty_tracked() {
            locked.pager_stats_modified = true;
        }
    }

    /// Returns whether the VMO was modified, resetting that if `reset`.
    pub fn query_pager_vmo_stats_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        reset: bool,
    ) -> Result<bool, Status> {
        ensure!(self.page_source_type() == PageSourceType::UserPager, Status::NOT_SUPPORTED);
        let modified = locked.pager_stats_modified;
        if reset {
            locked.pager_stats_modified = false;
        }
        Ok(modified)
    }

    fn should_delay_reuse_on_free(&self) -> PmmOptDelayReuse {
        // If this object has ever held a pinned page, delay reuse of its pages to reduce the
        // impact of a "bad DMA" bug should one occur.
        if self.ever_pinned.load(Ordering::Acquire) {
            PmmOptDelayReuse::Yes
        } else {
            PmmOptDelayReuse::Default
        }
    }

    /// Frees `pages` to the PMM.
    pub fn free_pages(&self, pages: &mut VmPageDoublyLinkedList) {
        // SAFETY: The pages are owned by this object and were removed from it.
        unsafe { pmm::node().free_list(pages, self.should_delay_reuse_on_free()) };
    }

    /// Frees `page` to the PMM.
    pub fn free_page(&self, page: VmPagePtr) {
        // SAFETY: The page is owned by this object and was removed from it.
        unsafe {
            debug_assert!(!page.in_container());
            pmm::node().free_page(page, self.should_delay_reuse_on_free());
        }
    }

    /// Allocates a page that has not been initialized for use in an object.
    fn alloc_uninitialized_page(
        &self,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<VmPagePtr, Status> {
        // Another layer has already allocated a page for us.
        if let Some(page) = page_request.take_page() {
            return Ok(page);
        }
        let result = pmm::node().alloc_page_in(self.arena, self.pmm_alloc_flags);
        if result == Err(Status::SHOULD_WAIT) {
            page_request.make_active(self.arena);
        }
        result
    }

    /// Allocates a new page for the VMO, filling out the page request if necessary.
    pub fn alloc_page(&self, page_request: &mut AnonymousPageRequest) -> Result<VmPagePtr, Status> {
        let page = self.alloc_uninitialized_page(page_request)?;
        initialize_vm_page(page);
        Ok(page)
    }

    /// Allocates a page that has not been initialized, from `alloc_list` if it has pages.
    fn allocate_uninitialized_page(
        &self,
        alloc_list: Option<&mut VmPageDoublyLinkedList>,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<VmPagePtr, Status> {
        match page_request.take_page().or_else(|| alloc_list.and_then(|l| l.pop_front())) {
            Some(page) => Ok(page),
            None => self.alloc_uninitialized_page(page_request),
        }
    }

    /// Allocates a new zeroed page, from `alloc_list` if it has pages. This is `AllocateCopyPage`
    /// of the zero page.
    fn allocate_zero_page(
        &self,
        alloc_list: Option<&mut VmPageDoublyLinkedList>,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<VmPagePtr, Status> {
        let page = self.allocate_uninitialized_page(alloc_list, page_request)?;
        initialize_vm_page(page);
        zero_page(page);
        Ok(page)
    }

    /// Removes the page at `offset` from the page queues and adds it to the deferred freed list,
    /// or gives it back if it is borrowed.
    fn remove_page_locked(&self, offset: u64, page: VmPagePtr, deferred: &mut DeferredOps) {
        if self.is_borrowed(page) {
            self.return_borrowed_page(offset, page);
            return;
        }
        self.revoke_loans(offset, page);
        page_queues().remove(page);
        deferred.freed_list().push_back(page);
    }

    /// Performs initial checks and slot allocations for inserting a new page at `offset`.
    fn begin_add_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        overwrite: CanOverwriteSlot,
    ) -> Result<AddPageTransaction, Status> {
        // If we're backed by a user pager, we cannot directly update empty slots in the page list.
        // An empty slot might lie in a sparse zero interval, which would require splitting the
        // interval around the required offset before it can be manipulated.
        let interval_handling = if self.page_source_type() == PageSourceType::UserPager {
            // We can overwrite zero intervals if we're allowed to overwrite zeros (or non-zeros).
            if overwrite != CanOverwriteSlot::Empty {
                IntervalHandling::SplitInterval
            } else {
                IntervalHandling::CheckForInterval
            }
        } else {
            IntervalHandling::NoIntervals
        };
        let (slot, is_in_interval) = locked.page_list.lookup_or_allocate(offset, interval_handling);
        let slot = slot.map(|s| s as *mut VmPageOrMarker);
        if is_in_interval {
            debug_assert_ne!(interval_handling, IntervalHandling::NoIntervals);
            // Return error if the offset lies in an interval but we cannot overwrite intervals.
            if interval_handling != IntervalHandling::SplitInterval {
                debug_assert!(slot.is_none());
                return Err(Status::ALREADY_EXISTS);
            }
            // SAFETY: A slot was returned for the split interval.
            debug_assert!(slot.is_some_and(|s| unsafe { (*s).is_interval_slot() }));
        }
        let Some(slot) = slot else {
            return Err(Status::NO_MEMORY);
        };
        // SAFETY: The slot is in the page list, which is not modified until the transaction ends.
        let slot_ref = unsafe { &*slot };
        if let Err(status) = Self::check_overwrite_conditions_locked(
            locked.size,
            locked.life_cycle,
            offset,
            slot_ref,
            overwrite,
        ) {
            if slot_ref.is_empty() {
                locked.page_list.return_empty_slot(offset);
            }
            return Err(status);
        }
        Ok(AddPageTransaction { slot, offset })
    }

    /// Like `begin_add_page_locked` with a slot the caller already has, which must be for
    /// `offset` and not in the middle of an interval.
    fn begin_add_page_with_slot_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        slot: &mut VmPageOrMarker,
        overwrite: CanOverwriteSlot,
    ) -> Result<AddPageTransaction, Status> {
        Self::check_overwrite_conditions_locked(
            locked.size,
            locked.life_cycle,
            offset,
            slot,
            overwrite,
        )?;
        debug_assert!(
            self.page_source_type() != PageSourceType::UserPager
                || !slot.is_empty()
                || !locked.page_list.is_offset_in_zero_interval(offset)
        );
        Ok(AddPageTransaction { slot, offset })
    }

    fn check_overwrite_conditions_locked(
        size: u64,
        life_cycle: LifeCycle,
        offset: u64,
        slot: &VmPageOrMarker,
        overwrite: CanOverwriteSlot,
    ) -> Result<(), Status> {
        // Pages can be added as part of Init, but not once we transition to dead.
        debug_assert_ne!(life_cycle, LifeCycle::Dead);
        ensure!(offset < size, Status::OUT_OF_RANGE);
        match overwrite {
            CanOverwriteSlot::Empty if !slot.is_empty() => Err(Status::ALREADY_EXISTS),
            CanOverwriteSlot::EmptyOrParent if !slot.is_empty() && !slot.is_parent_content() => {
                Err(Status::ALREADY_EXISTS)
            }
            CanOverwriteSlot::ZeroMarkerOrInterval if slot.is_page_or_ref() => {
                Err(Status::ALREADY_EXISTS)
            }
            _ => Ok(()),
        }
    }

    /// Completes an add page transaction by inserting `p`, returning the previous content, which
    /// the caller must deal with. Unmaps the offset from mappings unless `deferred` is `None`.
    fn complete_add_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        transaction: &mut AddPageTransaction,
        p: VmPageOrMarker,
        deferred: Option<&mut DeferredOps>,
    ) -> VmPageOrMarker {
        let offset = transaction.offset;
        ltracef!("vmo {:p}, offset {:#x}\n", self, offset);
        // Markers should never be placed in a node that uses parent content markers.
        debug_assert!(!p.is_marker() || !self.node_has_parent_content_markers());
        // If this is actually a real page, we need to place it into the appropriate queue.
        if p.is_page() {
            let page = p.page();
            debug_assert_eq!(page.state(), VmPageState::Object);
            debug_assert_eq!(pin_count(page), 0);
            self.set_not_pinned_locked(page, offset, locked.high_priority_count);
        }
        // SAFETY: The slot is valid until the transaction completes.
        let old = VmPageOrMarkerRef::new(unsafe { &mut *transaction.slot }).swap_content(p);
        transaction.slot = std::ptr::null_mut();
        if let Some(deferred) = deferred {
            if old.is_empty() && self.page_source_type() == PageSourceType::UserPager {
                // An empty slot where the page source is a user pager cannot have any mappings,
                // since the content is unknown, so no range change update is needed.
            } else {
                // Other mappings may have covered this offset into the vmo, so unmap those ranges.
                self.range_change_update_locked(
                    locked,
                    VmCowRange::new(offset, PAGE_SIZE),
                    RangeChangeOp::Unmap,
                    Some(deferred),
                );
            }
        }
        old
    }

    /// Completes an add page transaction with a page not yet in the object state, optionally
    /// zeroing it.
    fn complete_add_new_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        transaction: &mut AddPageTransaction,
        page: VmPagePtr,
        zero: bool,
        deferred: Option<&mut DeferredOps>,
    ) -> VmPageOrMarker {
        debug_assert!(is_page_rounded(transaction.offset));
        initialize_vm_page(page);
        if zero {
            zero_page(page);
        }
        // Pages being added to pager backed VMOs should have a valid dirty_state before being
        // added to the page list, so that they can be inserted in the correct page queue. New pages
        // start off clean.
        if self.page_source_type() == PageSourceType::UserPager {
            // Only zero pages can be added as new pages to pager backed VMOs.
            debug_assert!(zero || is_zero_page(page));
            self.update_dirty_state_locked(
                page,
                transaction.offset,
                DirtyState::Clean,
                true,
                locked.high_priority_count,
            );
        }
        self.complete_add_page_locked(
            locked,
            transaction,
            VmPageOrMarker::from_page(page),
            deferred,
        )
    }

    /// Adds `p` at `offset`, freeing it if insertion fails.
    fn add_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        mut p: VmPageOrMarker,
        overwrite: CanOverwriteSlot,
        deferred: Option<&mut DeferredOps>,
    ) -> Result<VmPageOrMarker, Status> {
        let mut transaction = match self.begin_add_page_locked(locked, offset, overwrite) {
            Ok(transaction) => transaction,
            Err(status) => {
                if p.is_page() {
                    self.free_page(p.release_page());
                }
                return Err(status);
            }
        };
        Ok(self.complete_add_page_locked(locked, &mut transaction, p, deferred))
    }

    /// Adds an allocated page at `offset`, optionally zeroing it. If an error is returned the
    /// caller retains ownership of `page`.
    fn add_new_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        page: VmPagePtr,
        overwrite: CanOverwriteSlot,
        zero: bool,
        deferred: Option<&mut DeferredOps>,
    ) -> Result<VmPageOrMarker, Status> {
        let mut transaction = self.begin_add_page_locked(locked, offset, overwrite)?;
        Ok(self.complete_add_new_page_locked(locked, &mut transaction, page, zero, deferred))
    }

    /// Releases all the pages in [offset, offset + len) into `freed_list`.
    fn release_owned_pages_range_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        len: u64,
        freed_list: &mut VmPageDoublyLinkedList,
    ) {
        debug_assert!(offset <= locked.size && offset + len <= locked.size);
        let mut page_remover = BatchPqRemove::new(self, freed_list);
        // Removing all content is quicker, but does not say where the pages were, which loans
        // need.
        if offset == 0 && len == locked.size && self.sharing() == Sharing::None {
            locked.page_list.remove_all_content(|mut p| {
                if p.is_page() {
                    let page = p.release_page();
                    // SAFETY: The lock is held, and the page is ours as we do not share.
                    page_remover.push(unsafe { page.object().get_page_offset() }, page);
                }
            });
        } else {
            let _ = locked.page_list.remove_pages(offset, offset + len, |p, off| {
                page_remover.push_content(p, off);
                Status::NEXT
            });
        }
        page_remover.flush();
    }

    /// Transitions to Dead, closing the page source and freeing all pages. Called when the VMO is
    /// destroyed.
    pub fn dead_transition(&self) {
        let mut freed_list = VmPageDoublyLinkedList::new();
        {
            let mut locked = self.lock();
            if locked.life_cycle != LifeCycle::Alive {
                return;
            }
            locked.life_cycle = LifeCycle::Dying;
            // Close any PageSource.
            if let Some(source) = &self.page_source {
                source.close();
            }
            // At the point of destruction we should no longer have any mappings still referencing
            // us, and by extension our priority count must therefore be back to zero.
            debug_assert_eq!(locked.high_priority_count, 0);
            let size = locked.size;
            self.release_owned_pages_range_locked(&mut locked, 0, size, &mut freed_list);
            debug_assert!(locked.page_list.is_empty());
            // Every loan was taken back with its page.
            debug_assert!(self.loans.lock().by_offset.is_empty());
            locked.life_cycle = LifeCycle::Dead;
        }
        self.free_pages(&mut freed_list);
    }

    /// Updates the page's dirty state, moving the page between page queues if required.
    /// `is_pending_add` indicates whether the page is yet to be added to the page list, in which
    /// case the page queue is updated when it is added.
    fn update_dirty_state_locked(
        &self,
        page: VmPagePtr,
        offset: u64,
        dirty_state: DirtyState,
        is_pending_add: bool,
        high_priority_count: i64,
    ) {
        assert_eq!(self.page_source_type(), PageSourceType::UserPager);
        // SAFETY: The lock is held.
        debug_assert!(is_pending_add || unsafe { self.is_backlink(page.object().get_object()) });
        // SAFETY: The lock is held.
        debug_assert!(is_pending_add || unsafe { page.object().get_page_offset() } == offset);

        let mut update_page_queues = false;
        match dirty_state {
            DirtyState::Clean => {
                // If the page is not in the process of being added, we can only see a transition
                // to Clean from AwaitingClean.
                assert!(is_pending_add || is_page_awaiting_clean(page));
                update_page_queues = !is_pending_add;
            }
            DirtyState::Dirty => {
                // If the page is not in the process of being added, we can only see a transition
                // to Dirty from Clean or AwaitingClean.
                assert!(is_pending_add || is_page_clean(page) || is_page_awaiting_clean(page));
                update_page_queues = !is_pending_add;
                // Lent pages must stay Clean, so take them back before they are written.
                if !is_pending_add {
                    self.revoke_loans(offset, page);
                }
            }
            DirtyState::AwaitingClean => {
                // A newly added page cannot start off as AwaitingClean.
                assert!(!is_pending_add);
                // A pinned page will be kept Dirty as long as it is pinned.
                assert_eq!(pin_count(page), 0);
                // We can only transition to AwaitingClean from Dirty.
                assert!(is_page_dirty(page));
                // Leave the page in the Dirty queue for now as it is not clean yet; it will be
                // moved out on WritebackEnd.
                debug_assert!(page_queues().debug_page_is_pager_backed_dirty(page));
            }
            DirtyState::Untracked => panic!("cannot transition to Untracked"),
        }
        // SAFETY: The lock is held.
        unsafe { page.object().set_dirty_state(dirty_state as u8) };
        if update_page_queues && pin_count(page) == 0 {
            // Move the page to the appropriate page queue: the reclaimable queue if Clean, where it
            // will age, or the Dirty queue, where it is protected from eviction.
            self.move_to_not_pinned_locked(page, offset, high_priority_count);
        }
    }

    /// Tries to prepare [offset, offset + len) for writing by marking pages dirty or verifying that
    /// they are already dirty. `dirty_len_out` returns the length from `offset` that is dirty
    /// after the call. If pages need a DIRTY request, one is made and ZX_ERR_SHOULD_WAIT returned.
    pub fn prepare_for_write_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        page_request: &mut LazyPageRequest,
        dirty_len_out: &mut u64,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        debug_assert!(range.is_bounded_by(locked.size));
        let source = self.page_source.as_ref().expect("page source");
        debug_assert_eq!(self.page_source_type(), PageSourceType::UserPager);

        let mut dirty_len = 0u64;
        let start_offset = range.offset;
        let end_offset = range.end();
        let high_priority_count = locked.high_priority_count;

        // If the VMO does not require us to trap dirty transitions, simply mark the pages dirty for
        // the first consecutive run of committed pages within the range.
        if !source.should_trap_dirty_transitions() {
            let status = locked.page_list.for_every_page_and_gap_in_range(
                start_offset,
                end_offset,
                |p, off| {
                    if p.is_marker() || p.is_interval_zero() {
                        // Found a marker or zero interval. End the traversal.
                        return Status::STOP;
                    }
                    debug_assert!(p.is_page());
                    let page = p.page();
                    debug_assert!(is_page_dirty_tracked(page));
                    // A borrowed page must be replaced by a copy before it can be dirtied, which
                    // the lookup that follows does. End the traversal.
                    if self.is_borrowed(page) {
                        return Status::STOP;
                    }
                    // Mark the page dirty.
                    if !is_page_dirty(page) {
                        self.update_dirty_state_locked(
                            page,
                            off,
                            DirtyState::Dirty,
                            false,
                            high_priority_count,
                        );
                    }
                    debug_assert_eq!(start_offset + dirty_len, off);
                    dirty_len += PAGE_SIZE;
                    Status::NEXT
                },
                // We found a gap. End the traversal.
                |_, _| Status::STOP,
            );
            debug_assert!(status.is_ok());
            *dirty_len_out = dirty_len;
            return Ok(());
        }

        // Otherwise, generate a DIRTY page request for pages in the range which need to transition
        // to Dirty: any contiguous run of non-Dirty pages (committed pages as well as zero page
        // markers), and zero intervals.
        let mut pages_to_dirty_len = 0u64;
        // Try to add pages to the run of dirty pages being tracked.
        let accumulate_dirty_pages =
            |pages_to_dirty_len: &mut u64, dirty_len: &mut u64, start: u64, end: u64| {
                // Bail if we were tracking a non-zero run of pages to be dirtied.
                if *pages_to_dirty_len > 0 {
                    return Status::STOP;
                }
                // Append the page to the dirty range being tracked if it immediately follows it.
                if start_offset + *dirty_len == start {
                    *dirty_len += end - start;
                    return Status::NEXT;
                }
                Status::STOP
            };
        // Try to add pages to the run of to-be-dirtied pages being tracked.
        let accumulate_pages_to_dirty =
            |pages_to_dirty_len: &mut u64, dirty_len: &mut u64, start: u64, end: u64| {
                // Bail if we were already accumulating a non-zero run of Dirty pages.
                if *dirty_len > 0 {
                    return Status::STOP;
                }
                // Append the pages to the range being tracked if they immediately follow it.
                if start_offset + *pages_to_dirty_len == start {
                    *pages_to_dirty_len += end - start;
                    return Status::NEXT;
                }
                Status::STOP
            };

        // This tracks the beginning of an interval that falls in the specified range. Since we
        // might start partway inside an interval, this is initialized to start_offset.
        let mut interval_start_off = start_offset;
        let mut unmatched_interval_start = false;
        let mut found_page_or_gap = false;
        let found_gap = core::cell::Cell::new(false);
        let status = locked.page_list.for_every_page_and_gap_in_range(
            start_offset,
            end_offset,
            |p, off| {
                found_page_or_gap = true;
                if p.is_page() {
                    let page = p.page();
                    debug_assert!(is_page_dirty_tracked(page));
                    // Page is already dirty. Try to add it to the dirty run.
                    if is_page_dirty(page) {
                        return accumulate_dirty_pages(
                            &mut pages_to_dirty_len,
                            &mut dirty_len,
                            off,
                            off + PAGE_SIZE,
                        );
                    }
                    // If the page is clean, mark it accessed to grant it some protection from
                    // eviction until the pager has a chance to respond to the DIRTY request.
                    if is_page_clean(page) {
                        page_queues().mark_accessed(page);
                    }
                } else if p.is_interval_zero() {
                    if p.is_interval_start() || p.is_interval_slot() {
                        unmatched_interval_start = true;
                        interval_start_off = off;
                    }
                    if p.is_interval_end() || p.is_interval_slot() {
                        unmatched_interval_start = false;
                        // We need to commit pages if this is an interval, irrespective of the dirty
                        // state.
                        return accumulate_pages_to_dirty(
                            &mut pages_to_dirty_len,
                            &mut dirty_len,
                            interval_start_off,
                            off + PAGE_SIZE,
                        );
                    }
                    return Status::NEXT;
                }
                // This is a either a zero page marker (which represents a clean zero page) or a
                // committed page which is not already Dirty.
                debug_assert!(p.is_marker() || !is_page_dirty(p.page()));
                accumulate_pages_to_dirty(
                    &mut pages_to_dirty_len,
                    &mut dirty_len,
                    off,
                    off + PAGE_SIZE,
                )
            },
            |_, _| {
                found_gap.set(true);
                // We found a gap. End the traversal.
                Status::STOP
            },
        );
        debug_assert!(status.is_ok());
        let found_page_or_gap = found_page_or_gap || found_gap.get();

        // Process the last remaining interval if there is one.
        if unmatched_interval_start {
            accumulate_pages_to_dirty(
                &mut pages_to_dirty_len,
                &mut dirty_len,
                interval_start_off,
                end_offset,
            );
        }

        // Account for the case where we started and ended in unpopulated slots inside an interval.
        if !found_page_or_gap {
            debug_assert!(locked.page_list.is_offset_in_zero_interval(start_offset));
            debug_assert!(dirty_len == 0 && pages_to_dirty_len == 0);
            // The entire range falls in an interval so it needs a DIRTY request.
            pages_to_dirty_len = end_offset - start_offset;
        }

        // We should either have found dirty pages or pages that need to be dirtied, but not both.
        debug_assert!(dirty_len == 0 || pages_to_dirty_len == 0);
        *dirty_len_out = dirty_len;

        // No pages need to transition to Dirty.
        if pages_to_dirty_len == 0 {
            return Ok(());
        }

        // Found a contiguous run of pages that need to transition to Dirty. The page source will
        // never succeed synchronously.
        Err(source.request_dirty_transition(
            page_request.get(),
            start_offset,
            pages_to_dirty_len,
            locked.vmo_debug_info,
        ))
    }

    /// Returns a cursor for looking up pages in `range`.
    pub fn get_lookup_cursor_locked(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
    ) -> Result<LookupCursor, Status> {
        debug_assert!(!range.is_empty());
        debug_assert!(range.is_page_aligned());
        debug_assert_eq!(locked.life_cycle, LifeCycle::Alive);
        ensure!(
            range.offset < locked.size && range.is_bounded_by(locked.size),
            Status::OUT_OF_RANGE
        );
        Ok(LookupCursor::new(self, range))
    }

    /// Attempts to commit a range of pages, reporting how much was committed in
    /// `committed_len`. ZX_ERR_SHOULD_WAIT means the page request should be waited on before
    /// retrying.
    pub fn commit_range_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        deferred: &mut DeferredOps,
        committed_len: &mut u64,
        page_request: &mut MultiPageRequest,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        debug_assert!(range.is_bounded_by(locked.size));

        // If this vmo has a direct page source, then the source will provide the backing memory.
        // Otherwise we optimize things by preallocating all the pages.
        let mut page_list = VmPageDoublyLinkedList::new();
        if !self.root_has_page_source() {
            // make a pass through the list to find out how many pages we need to allocate
            let mut count = range.len / PAGE_SIZE;
            let _ = locked.page_list.for_every_page_in_range(range.offset, range.end(), |p, _| {
                if p.is_page() {
                    count -= 1;
                }
                Status::NEXT
            });
            if count == 0 {
                *committed_len = range.len;
                return Ok(());
            }
            // Ignore ZX_ERR_SHOULD_WAIT since the loop below will fall back to a page by page
            // allocation, allowing us to wait for single pages should we need to.
            let flags = self.pmm_alloc_flags;
            match pmm::node().alloc_pages_in(self.arena, count as usize, flags, &mut page_list) {
                Ok(()) | Err(Status::SHOULD_WAIT) => {}
                Err(status) => return Err(status),
            }
        }

        let start_offset = range.offset;
        let end = range.end();
        let result = (|| {
            let mut cursor = self.get_lookup_cursor_locked(locked, range)?;
            // Commit represents an explicit desire to have pages and should not be deduped back to
            // the zero page.
            cursor.disable_zero_fork();
            cursor.give_alloc_list(&mut page_list);
            let mut offset = start_offset;
            let mut status = Ok(());
            while offset < end {
                if let Err(error) = cursor.require_owned_page(
                    self,
                    locked,
                    false,
                    (end - offset) / PAGE_SIZE,
                    deferred,
                    page_request,
                ) {
                    status = Err(error);
                    break;
                }
                offset += PAGE_SIZE;
            }
            // Record how much we were able to process.
            *committed_len = offset - start_offset;
            cursor.clear_alloc_list();
            status
        })();
        if !page_list.is_empty() {
            self.free_pages(&mut page_list);
        }
        result
    }

    /// Increases the pin count of the pages in the range, which must all be committed.
    pub fn pin_range_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        debug_assert!(range.is_bounded_by(locked.size));
        self.ever_pinned.store(true, Ordering::Release);

        // Tracks our expected page offset when iterating to ensure all pages are present.
        let mut next_offset = range.offset;
        let status =
            locked.page_list.for_every_page_in_range(range.offset, range.end(), |p, off| {
                // Committing replaces borrowed pages with our own, which are the ones we can pin.
                if off != next_offset || !p.is_page() || self.is_borrowed(p.page()) {
                    return Status::BAD_STATE;
                }
                let page = p.page();
                debug_assert_eq!(page.state(), VmPageState::Object);
                let count = pin_count(page);
                if u32::from(count) == crate::vm::page::object::MAX_PIN_COUNT {
                    return Status::UNAVAILABLE;
                }
                // Lent pages must stay unpinned, so take them back first.
                if count == 0 {
                    self.revoke_loans(off, page);
                }
                // SAFETY: The lock is held.
                unsafe { page.object().set_pin_count(count + 1) };
                if count == 0 {
                    page_queues().move_to_wired(page);
                }
                next_offset += PAGE_SIZE;
                Status::NEXT
            });
        let actual = (next_offset - range.offset) / PAGE_SIZE;
        // Count whatever pages we pinned, in the failure scenario this will get decremented on the
        // unpin.
        locked.pinned_page_count += actual;
        // Perform one final check that we actually pinned the number of pages we expected to.
        let result = status.and(if actual != range.len / PAGE_SIZE {
            Err(Status::BAD_STATE)
        } else {
            Ok(())
        });
        if result.is_err() && next_offset > range.offset {
            self.unpin_locked(locked, VmCowRange::new(range.offset, next_offset - range.offset));
        }
        result
    }

    /// Decreases the pin count of the pages in the range.
    pub fn unpin_locked(&self, locked: &mut VmCowPagesLocked, range: VmCowRange) {
        assert!(range.is_bounded_by(locked.size));
        assert!(!range.is_empty());
        let start_page_offset = round_down_page_size(range.offset);
        let end_page_offset = round_up_page_size(range.end());
        let high_priority_count = locked.high_priority_count;
        let mut unpin_count = 0u64;
        let status = locked.page_list.for_every_page_and_gap_in_range(
            start_page_offset,
            end_page_offset,
            |p, off| {
                // Only real pages can be pinned.
                assert!(p.is_page());
                let page = p.page();
                let count = pin_count(page);
                assert!(count > 0);
                // SAFETY: The lock is held.
                unsafe { page.object().set_pin_count(count - 1) };
                if count == 1 {
                    self.move_to_not_pinned_locked(page, off, high_priority_count);
                }
                unpin_count += 1;
                Status::NEXT
            },
            |_, _| Status::NOT_FOUND,
        );
        assert!(status.is_ok(), "Tried to unpin an uncommitted page");
        assert_eq!(unpin_count, (end_page_offset - start_page_offset) / PAGE_SIZE);
        locked.pinned_page_count =
            locked.pinned_page_count.checked_sub(unpin_count).expect("pinned page count underflow");
    }

    fn any_pages_pinned_locked(&self, locked: &VmCowPagesLocked, offset: u64, len: u64) -> bool {
        debug_assert!(is_page_rounded(offset) && is_page_rounded(len));
        if locked.pinned_page_count == 0 {
            return false;
        }
        let mut found_pinned = false;
        let _ = locked.page_list.for_every_page_in_range(offset, offset + len, |p, _| {
            if p.is_page() && pin_count(p.page()) > 0 {
                found_pinned = true;
                return Status::STOP;
            }
            Status::NEXT
        });
        found_pinned
    }

    /// Releases pages in the range, so it becomes zero again. Only valid when the absence of a
    /// page implies zero.
    pub fn decommit_range(self: &Arc<Self>, range: VmCowRange) -> Result<(), Status> {
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        // was in range, just zero length
        if range.is_empty() {
            return Ok(());
        }
        // Currently, we can't decommit if the absence of a page doesn't imply zeroes.
        ensure!(self.page_source_type() != PageSourceType::UserPager, Status::NOT_SUPPORTED);
        // Demand offset and length be correctly aligned to not give surprising user semantics.
        ensure!(range.is_page_aligned(), Status::INVALID_ARGS);
        self.unmap_and_free_pages_locked(&mut locked, range.offset, range.len, &mut deferred)
            .map(|_| ())
    }

    /// Unmaps and frees all the committed pages in the range, returning how many were freed.
    fn unmap_and_free_pages_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        len: u64,
        deferred: &mut DeferredOps,
    ) -> Result<u64, Status> {
        ensure!(!self.any_pages_pinned_locked(locked, offset, len), Status::BAD_STATE);
        // unmap all of the pages in this range on all the mapping regions
        self.range_change_update_locked(
            locked,
            VmCowRange::new(offset, len),
            RangeChangeOp::Unmap,
            Some(deferred),
        );
        let mut page_remover = BatchPqRemove::new(self, deferred.freed_list());
        let _ = locked.page_list.remove_pages(offset, offset + len, |p, off| {
            page_remover.push_content(p, off);
            Status::NEXT
        });
        page_remover.flush();
        Ok(page_remover.freed_count as u64)
    }

    /// Returns true if a page is not currently committed, and if the offset were to be read from,
    /// it would be read as zero.
    pub fn page_would_read_zero_locked(&self, locked: &VmCowPagesLocked, page_offset: u64) -> bool {
        debug_assert!(is_page_rounded(page_offset));
        debug_assert!(page_offset < locked.size);
        let slot = locked.page_list.lookup(page_offset);
        if self.node_has_parent_content_markers() {
            return !slot.is_some_and(|s| s.is_page_or_ref());
        }
        if slot.is_some_and(|s| s.is_marker()) {
            // This is already considered zero as there's a marker.
            return true;
        }
        if self.page_source_type() == PageSourceType::UserPager
            && (slot.is_some_and(|s| s.is_interval_zero())
                || locked.page_list.is_offset_in_zero_interval(page_offset))
        {
            // Pages in zero intervals are supplied as zero by the kernel.
            return true;
        }
        // An empty slot with a page source reads the page source's content, assumed non-zero.
        false
    }

    /// Zeroes the range, which for a user pager is done by inserting zero intervals that are dirty
    /// if `dirty_track`. Returns the status and the number of bytes zeroed, which may be nonzero
    /// even if the status is an error.
    pub fn zero_pages_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        dirty_track: bool,
        deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> (Result<(), Status>, u64) {
        debug_assert!(range.is_bounded_by(locked.size));
        debug_assert!(range.is_page_aligned());
        match self.page_source_type() {
            PageSourceType::UserPager => self.zero_pages_direct_user_pager_locked(
                locked,
                range,
                dirty_track,
                deferred,
                page_request,
            ),
            PageSourceType::Anonymous => {
                self.zero_pages_no_direct_page_source_locked(locked, range, deferred)
            }
            PageSourceType::Contiguous => unreachable!("contiguous VMOs are not supported"),
        }
    }

    fn zero_pages_direct_user_pager_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        dirty_track: bool,
        deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> (Result<(), Status>, u64) {
        let start = range.offset;
        let end = range.end();
        let mut processed_len = 0u64;
        if start == end {
            return (Ok(()), processed_len);
        }
        // If we're not asked to dirty track, we will need to drop pages, because if a page is
        // present it is going to be in one of the dirty tracked states. So check for any pinned
        // pages first.
        if !dirty_track && self.any_pages_pinned_locked(locked, start, end - start) {
            return (Err(Status::BAD_STATE), processed_len);
        }
        // Unmap any page that is touched by this range in any of our mapping regions.
        self.range_change_update_locked(locked, range, RangeChangeOp::Unmap, Some(deferred));

        // Inserting zero intervals can modify the page list, so we cannot insert them while
        // iterating. Instead we traverse to find a range that can be a zero interval, break out of
        // the traversal and insert it, then resume from after it.
        let source = self.page_source.as_ref().unwrap().clone();
        let required_state =
            if dirty_track { ZeroRangeDirtyState::Dirty } else { ZeroRangeDirtyState::Untracked };
        let mut next_start_offset = start;
        #[derive(Clone, Copy, Default)]
        struct State {
            add_zero_interval: bool,
            start: u64,
            end: u64,
            replace_page: bool,
            overwrite_interval: bool,
        }
        loop {
            let mut in_interval = false;
            let mut interval_start = next_start_offset;
            let prev_start_offset = next_start_offset;
            let state = core::cell::Cell::new(State::default());
            let mut pinned_page = None;

            let traversal_start = next_start_offset;
            let status = locked.page_list.remove_pages_and_iterate_gaps(
                |p, off| {
                    // If this is a page, see if we can remove it and absorb it into a zero interval.
                    if p.is_page() {
                        if pin_count(p.page()) > 0 {
                            debug_assert!(dirty_track);
                            // Cannot remove this page if it is pinned. It must be looked up and
                            // zeroed, which needs the page list, so stop here.
                            pinned_page = Some(off);
                            return Status::STOP;
                        }
                        // Break out of the traversal. We can release the page and add a zero
                        // interval instead.
                        state.set(State {
                            add_zero_interval: true,
                            start: off,
                            end: off,
                            replace_page: true,
                            overwrite_interval: false,
                        });
                        return Status::STOP;
                    }
                    // Otherwise this is a marker or zero interval, in which case we already have
                    // zeroes, but we might need to change the dirty state.
                    debug_assert!(p.is_marker() || p.is_interval_zero());
                    if p.is_interval_start() {
                        // Track the interval start so we know how much to add to processed_len.
                        interval_start = off;
                        in_interval = true;
                        if p.zero_interval_dirty_state() != required_state {
                            // If we find the matching end, we will update state.end.
                            state.set(State {
                                add_zero_interval: true,
                                start: interval_start,
                                end: u64::MAX,
                                replace_page: false,
                                overwrite_interval: true,
                            });
                        }
                    } else if p.is_interval_end() {
                        if p.zero_interval_dirty_state() != required_state {
                            state.set(State {
                                add_zero_interval: true,
                                start: if in_interval { interval_start } else { u64::MAX },
                                end: off,
                                replace_page: false,
                                overwrite_interval: true,
                            });
                            return Status::STOP;
                        }
                        // Add the range from interval start to end.
                        processed_len += off + PAGE_SIZE - interval_start;
                        in_interval = false;
                    } else {
                        // This is either a single interval slot or a marker. Overwrite it with a
                        // zero interval if it is an interval slot with a different dirty state, or
                        // a marker and we're asked to not dirty track, since a marker is a clean
                        // zero page.
                        if p.is_marker() && !dirty_track {
                            // Release the marker so that it can be replaced by a gap.
                            *p = VmPageOrMarker::empty();
                        }
                        if p.is_empty()
                            || (p.is_interval_slot()
                                && p.zero_interval_dirty_state() != required_state)
                        {
                            state.set(State {
                                add_zero_interval: true,
                                start: off,
                                end: off,
                                replace_page: false,
                                overwrite_interval: p.is_interval_slot(),
                            });
                            return Status::STOP;
                        }
                        processed_len += PAGE_SIZE;
                    }
                    next_start_offset = off + PAGE_SIZE;
                    Status::NEXT
                },
                |gap_start, gap_end| {
                    // This gap will be replaced with a zero interval. Invalidate any read requests
                    // in this range.
                    source.on_pages_supplied(gap_start, gap_end - gap_start);
                    state.set(State {
                        add_zero_interval: true,
                        start: gap_start,
                        end: gap_end - PAGE_SIZE,
                        replace_page: false,
                        overwrite_interval: false,
                    });
                    Status::STOP
                },
                traversal_start,
                end,
            );
            if let Err(status) = status {
                return (Err(status), processed_len);
            }
            let mut state = state.get();

            if let Some(off) = pinned_page {
                // Look up the pinned page, which ensures that we request a dirty transition if
                // needed by the pager, and zero it.
                let mut cursor = LookupCursor::new(self, VmCowRange::new(off, PAGE_SIZE));
                match cursor.require_owned_page(self, locked, true, 1, deferred, page_request) {
                    Ok(result) => zero_page(result.page),
                    Err(status) => return (Err(status), processed_len),
                }
                processed_len += PAGE_SIZE;
                next_start_offset = off + PAGE_SIZE;
            } else if state.add_zero_interval {
                let result = if state.replace_page {
                    debug_assert_eq!(state.start, state.end);
                    let page = locked
                        .page_list
                        .replace_page_with_zero_interval(state.start, required_state);
                    debug_assert_eq!(pin_count(page), 0);
                    self.remove_page_locked(state.start, page, deferred);
                    Ok(())
                } else if state.overwrite_interval {
                    let old_start = (state.start != u64::MAX).then_some(state.start);
                    let old_end = (state.end != u64::MAX).then_some(state.end);
                    if state.start == u64::MAX {
                        state.start = next_start_offset;
                    }
                    if state.end == u64::MAX {
                        state.end = end - PAGE_SIZE;
                    }
                    locked.page_list.overwrite_zero_interval(
                        old_start,
                        old_end,
                        state.start,
                        state.end,
                        required_state,
                    )
                } else {
                    locked.page_list.add_zero_interval(
                        state.start,
                        state.end + PAGE_SIZE,
                        required_state,
                    )
                };
                if let Err(status) = result {
                    debug_assert_eq!(status, Status::NO_MEMORY);
                    return (Err(status), processed_len);
                }
                processed_len += state.end - state.start + PAGE_SIZE;
                next_start_offset = state.end + PAGE_SIZE;
            } else if in_interval || next_start_offset == prev_start_offset {
                // Handle the last partial interval. Or the case where we did not advance
                // next_start_offset at all, which can only happen if the range fell entirely
                // inside an interval. We do not split an interval in the middle just to change its
                // dirty state, and claim that we processed the range.
                debug_assert!(
                    next_start_offset != prev_start_offset
                        || locked.page_list.is_offset_in_zero_interval(next_start_offset)
                );
                processed_len += end - interval_start;
                next_start_offset = end;
            }
            // Ensure we're making progress.
            debug_assert!(next_start_offset > prev_start_offset);
            if next_start_offset >= end {
                break;
            }
        }
        (Ok(()), processed_len)
    }

    /// Zeroes a range of anonymous pages. Without a parent, empty slots are zero, so pages are
    /// removed unless pinned, in which case they are zeroed.
    fn zero_pages_no_direct_page_source_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        deferred: &mut DeferredOps,
    ) -> (Result<(), Status>, u64) {
        debug_assert!(self.page_source.is_none());
        debug_assert!(self.can_decommit_zero_pages());
        let zeroed_len = core::cell::Cell::new(0u64);
        let mut already_unmapped = false;
        let mut to_remove = Vec::new();
        let status = locked.page_list.remove_pages_and_iterate_gaps(
            |slot, off| {
                // We don't expect intervals in non pager-backed VMOs.
                debug_assert!(!slot.is_interval());
                if slot.is_marker() {
                    // The slot is already zero.
                    zeroed_len.set(zeroed_len.get() + PAGE_SIZE);
                    return Status::NEXT;
                }
                if slot.is_page() && pin_count(slot.page()) > 0 {
                    zero_page(slot.page());
                    zeroed_len.set(zeroed_len.get() + PAGE_SIZE);
                    return Status::NEXT;
                }
                // All of the below cases require us to invalidate the mappings of the affected
                // pages, done after the traversal.
                already_unmapped = true;
                if slot.is_page() {
                    // We have a page in an anonymous hierarchy, so we can just remove the content.
                    to_remove.push((off, slot.release_page()));
                } else {
                    *slot = VmPageOrMarker::empty();
                }
                zeroed_len.set(zeroed_len.get() + PAGE_SIZE);
                Status::NEXT
            },
            |gap_start, gap_end| {
                // Gaps are already zero.
                zeroed_len.set(zeroed_len.get() + gap_end - gap_start);
                Status::NEXT
            },
            range.offset,
            range.end(),
        );
        if already_unmapped {
            self.range_change_update_locked(locked, range, RangeChangeOp::Unmap, Some(deferred));
        }
        for (offset, page) in to_remove {
            self.remove_page_locked(offset, page, deferred);
        }
        (status, zeroed_len.get())
    }

    /// Moves an existing page to the queue appropriate for its state, now that it is not pinned.
    fn move_to_not_pinned_locked(&self, page: VmPagePtr, _offset: u64, high_priority_count: i64) {
        let pq = page_queues();
        if self.page_source_type() == PageSourceType::UserPager {
            debug_assert!(is_page_dirty_tracked(page));
            // We can only move Clean pages to the pager backed queues as they track age information
            // for eviction; only Clean pages can be evicted. Pages in AwaitingClean and Dirty are
            // protected from eviction in the Dirty queue.
            if is_page_clean(page) {
                if high_priority_count != 0 {
                    pq.move_to_high_priority(page);
                } else {
                    pq.move_to_reclaim(page);
                }
            } else {
                pq.move_to_pager_backed_dirty(page);
            }
        } else if self.can_decommit_zero_pages() {
            if high_priority_count != 0 && !pq.reclaim_is_only_pager_backed() {
                pq.move_to_high_priority(page);
            } else {
                pq.move_to_anonymous(page, false);
            }
        } else {
            pq.move_to_wired(page);
        }
    }

    /// Places a newly added, not yet pinned, page into the appropriate page queue.
    fn set_not_pinned_locked(&self, page: VmPagePtr, offset: u64, high_priority_count: i64) {
        let pq = page_queues();
        let backlink = self.backlink();
        if self.page_source_type() == PageSourceType::UserPager {
            debug_assert!(is_page_dirty_tracked(page));
            if is_page_clean(page) {
                if high_priority_count != 0 {
                    pq.set_high_priority(page, backlink, offset);
                } else {
                    pq.set_reclaim(page, backlink, offset);
                }
            } else {
                pq.set_pager_backed_dirty(page, backlink, offset);
            }
        } else if self.can_decommit_zero_pages() {
            if high_priority_count != 0 && !pq.reclaim_is_only_pager_backed() {
                pq.set_high_priority(page, backlink, offset);
            } else {
                pq.set_anonymous(page, backlink, offset, false);
            }
        } else {
            pq.set_wired(page, backlink, offset);
        }
    }

    /// Resolves any READ requests in the range by spuriously resolving them.
    fn invalidate_read_requests_locked(&self, locked: &VmCowPagesLocked, offset: u64, len: u64) {
        let source = self.page_source.as_ref().unwrap();
        let status = locked.page_list.for_every_page_and_gap_in_range(
            offset,
            offset + len,
            |_, _| Status::NEXT,
            |gap_start, gap_end| {
                source.on_pages_supplied(gap_start, gap_end - gap_start);
                Status::NEXT
            },
        );
        debug_assert!(status.is_ok());
    }

    /// Resolves any DIRTY requests in the range by spuriously resolving them. fxdax: or, with a
    /// `failure`, fails them with it (`PageSource::on_dirty_failed`, TASK-79.75).
    fn invalidate_dirty_requests_locked(
        &self,
        locked: &VmCowPagesLocked,
        offset: u64,
        len: u64,
        failure: Option<Status>,
    ) {
        debug_assert_eq!(self.page_source_type(), PageSourceType::UserPager);
        let source = self.page_source.as_ref().unwrap();
        debug_assert!(source.should_trap_dirty_transitions());
        let resolve = |start, len| match failure {
            None => source.on_pages_dirtied(start, len),
            Some(status) => source.on_dirty_failed(start, len, status),
        };
        let start = offset;
        let end = offset + len;
        let status = locked.page_list.for_every_page_and_contiguous_run_in_range(
            |p, _| {
                // A marker is a clean zero page and might have an outstanding DIRTY request, as
                // might an interval, which is an uncommitted zero page.
                if p.is_marker() || p.is_interval_zero() {
                    return true;
                }
                // A page that is not Dirty already might have an outstanding DIRTY request.
                debug_assert!(is_page_dirty_tracked(p.page()));
                !is_page_dirty(p.page())
            },
            // Nothing to update for the page as we're not actually marking it Dirty.
            |_, _| Status::NEXT,
            |start, end, _| {
                // Resolve any DIRTY requests in this contiguous range.
                resolve(start, end - start);
                Status::NEXT
            },
            start,
            end,
        );
        debug_assert!(status.is_ok());
        // Now resolve DIRTY requests for any gaps, since pages could have been evicted, or zero
        // intervals written back, after request generation.
        let status = locked.page_list.for_every_page_and_gap_in_range(
            start,
            end,
            |_, _| Status::NEXT,
            |gap_start, gap_end| {
                resolve(gap_start, gap_end - gap_start);
                Status::NEXT
            },
        );
        debug_assert!(status.is_ok());
    }

    /// Resizes to `s`, which must be page aligned.
    pub fn resize(self: &Arc<Self>, s: u64) -> Result<(), Status> {
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        debug_assert!(is_page_rounded(locked.size) && is_page_rounded(s));
        let size = locked.size;
        if s < size {
            // shrinking
            let start = s;
            let len = size - start;
            // bail if there are any pinned pages in the range we're trimming
            ensure!(!self.any_pages_pinned_locked(&locked, start, len), Status::BAD_STATE);
            // unmap all of the pages in this range on all the mapping regions
            self.range_change_update_locked(
                &mut locked,
                VmCowRange::new(start, len),
                RangeChangeOp::Unmap,
                Some(&mut deferred),
            );
            // Resolve any outstanding page requests tracked by the page source that are now
            // out-of-bounds.
            if let Some(source) = &self.page_source {
                // Tell the page source that any non-resident pages that are now out-of-bounds were
                // supplied, to ensure that any reads of those pages get woken up.
                self.invalidate_read_requests_locked(&locked, start, len);
                // If DIRTY requests are supported, also tell the page source that any non-Dirty
                // pages that are now out-of-bounds were dirtied (without actually dirtying them).
                if self.page_source_type() == PageSourceType::UserPager
                    && source.should_trap_dirty_transitions()
                {
                    self.invalidate_dirty_requests_locked(&locked, start, len, None);
                }
            }
            // If pager-backed and the new size falls partway in an interval, clip the interval.
            if self.page_source_type() == PageSourceType::UserPager {
                // Check if the first populated slot we find in the now-invalid range is an
                // interval end.
                let mut interval_end = u64::MAX;
                let status = locked.page_list.for_every_page_in_range(start, size, |p, off| {
                    if p.is_interval_end() {
                        interval_end = off;
                    }
                    Status::STOP
                });
                debug_assert!(status.is_ok());
                if interval_end != u64::MAX {
                    locked
                        .page_list
                        .clip_interval_end(interval_end, interval_end - start + PAGE_SIZE)?;
                }
            }
            self.release_owned_pages_range_locked(&mut locked, start, len, deferred.freed_list());
        } else if s > size {
            // expanding
            let start = size;
            let len = s - start;
            // inform all our mappings that there's new bits
            self.range_change_update_locked(
                &mut locked,
                VmCowRange::new(start, len),
                RangeChangeOp::Unmap,
                Some(&mut deferred),
            );
            // If pager-backed, need to insert a dirty zero interval beyond the old size.
            if self.page_source_type() == PageSourceType::UserPager {
                locked.page_list.add_zero_interval(start, s, ZeroRangeDirtyState::Dirty)?;
            }
        }
        locked.size = s;
        // We were able to successfully resize. Mark as modified.
        self.mark_modified_locked(&mut locked);
        Ok(())
    }

    /// Calls `lookup_fn` with the offset and physical address of every committed page in the
    /// range.
    pub fn lookup_locked<F>(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
        mut lookup_fn: F,
    ) -> Result<(), Status>
    where
        F: FnMut(u64, crate::kernel::types::PAddr) -> Status,
    {
        ensure!(!range.is_empty(), Status::INVALID_ARGS);
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        locked.page_list.for_every_page_in_range(
            round_down_page_size(range.offset),
            round_up_page_size(range.end()),
            |p, off| {
                if !p.is_page() {
                    return Status::NEXT;
                }
                lookup_fn(off, p.page().paddr())
            },
        )
    }

    /// Takes the pages in `range` into `pages` starting at `splice_offset`, leaving the range
    /// empty. Only anonymous VMOs support this.
    pub fn take_pages(
        self: &Arc<Self>,
        range: VmCowRange,
        splice_offset: u64,
        pages: &mut VmPageSpliceList,
        taken_len: &mut u64,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        if !range.is_bounded_by(locked.size) {
            pages.finalize();
            return Err(Status::OUT_OF_RANGE);
        }
        if self.page_source.is_some() {
            pages.finalize();
            return Err(Status::NOT_SUPPORTED);
        }
        if self.any_pages_pinned_locked(&locked, range.offset, range.len) {
            pages.finalize();
            return Err(Status::BAD_STATE);
        }
        // On the assumption of success, unmap the entire range we are going to process.
        self.range_change_update_locked(
            &mut locked,
            range,
            RangeChangeOp::Unmap,
            Some(&mut deferred),
        );

        // Without a parent, and with an empty splice list, move the page list nodes directly into
        // the splice list.
        if pages.is_empty() && splice_offset == 0 {
            let mut found_page = false;
            let status = pages.add_pages_from(
                |src, dst, _| {
                    found_page = true;
                    // Splice lists do not support page intervals.
                    assert!(!src.is_interval());
                    if src.is_page() {
                        debug_assert_eq!(pin_count(src.page()), 0);
                        page_queues().remove(src.page());
                    }
                    *dst = src.swap(VmPageOrMarker::empty());
                },
                &mut locked.page_list,
                range.offset,
            );
            // If we did not find any pages, we could either be entirely inside a gap or an
            // interval. Make sure we're not inside an interval.
            assert!(found_page || !locked.page_list.is_offset_in_zero_interval(range.offset));
            status?;
            *taken_len = range.len;
            return Ok(());
        }

        // Otherwise move each page into the splice list. Gaps are zero, so can be left as gaps in
        // the splice list.
        let mut insert_status = Ok(());
        let status = locked.page_list.remove_pages(range.offset, range.end(), |slot, offset| {
            if slot.is_marker() {
                // Already zero. Can leave a gap, which is implied zero, in the splice list.
                *slot = VmPageOrMarker::empty();
                return Status::NEXT;
            }
            if slot.is_page() {
                debug_assert_eq!(pin_count(slot.page()), 0);
                page_queues().remove(slot.page());
            }
            let content = slot.swap(VmPageOrMarker::empty());
            if let Err(status) = pages.insert(offset - range.offset + splice_offset, content) {
                *taken_len = offset - range.offset;
                insert_status = Err(status);
                return status;
            }
            Status::NEXT
        });
        insert_status?;
        status?;
        pages.finalize();
        *taken_len = range.len;
        Ok(())
    }

    /// Pre-processes `pages` for supply (`supply_pages_locked`).
    ///
    /// Zircon converts references to pages here, which there are none of, as nothing is
    /// compressed. fxdax: every page supplied to the object must be of its arena (`vm::physmap`),
    /// so each of another arena's, such as a transfer's from another tenant's VMO, is replaced
    /// with a copy in a page of its own, and cleared and freed: freed pages keep what they held,
    /// and another arena's must hold nothing of this one's. The copies are allocated waiting, as a
    /// page fault's are, so this must be called without locks held, before `DeferredOps::new`
    /// takes the page source's: the allocation may wait for eviction, which takes it to evict this
    /// object's pages.
    pub fn process_pages_for_supply(&self, pages: &mut VmPageSpliceList) -> Result<(), Status> {
        let foreign = pages.count_pages(|page| arena_of(page) != self.arena);
        if foreign == 0 {
            return Ok(());
        }
        let mut copies = alloc_pages_waiting(self.arena, foreign)?;
        let mut originals = VmPageDoublyLinkedList::new();
        let _ = pages.mutate_pages(
            |mut slot, _| {
                if slot.is_page() && arena_of(slot.page()) != self.arena {
                    let copy = copies.pop_front().expect("a copy for each page of another arena");
                    initialize_vm_page(copy);
                    copy_page(copy, slot.page());
                    zero_page(slot.page());
                    let mut original = slot.swap_content(VmPageOrMarker::from_page(copy));
                    originals.push_back(original.release_page());
                }
                Status::NEXT
            },
            0,
        );
        debug_assert!(copies.is_empty());
        // SAFETY: The splice list owned the originals, which it holds no more.
        unsafe { pmm::free_list(&mut originals) };
        Ok(())
    }

    /// Supplies the pages in `pages` into `range`. For a pager-backed VMO only absent content is
    /// populated and existing content is left unchanged; gaps in `pages` become zero markers.
    ///
    /// fxdax: the pages must be of the object's arena, but for a `PhysicalPageProvider`'s, as
    /// `process_pages_for_supply` leaves them.
    pub fn supply_pages_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        pages: &mut VmPageSpliceList,
        options: SupplyOptions,
        deferred: &mut DeferredOps,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        assert!(options != SupplyOptions::PagerSupply || self.page_source.is_some());
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        if options == SupplyOptions::TransferData {
            ensure!(self.page_source.is_none(), Status::NOT_SUPPORTED);
            ensure!(
                !self.any_pages_pinned_locked(locked, range.offset, range.len),
                Status::BAD_STATE
            );
        }
        ensure!(!self.page_source.as_ref().is_some_and(|s| s.is_detached()), Status::BAD_STATE);
        let overwrite_policy = if options == SupplyOptions::TransferData {
            CanOverwriteSlot::PageOrRef
        } else {
            CanOverwriteSlot::Empty
        };
        let start = range.offset;
        let end = range.end();
        let parent_content_markers = self.node_has_parent_content_markers();

        // [new_pages_start, new_pages_start + new_pages_len) tracks the current run of consecutive
        // new pages added to this vmo.
        let mut new_pages_start = start;
        let mut new_pages_len = 0u64;
        let mut supplied_pages_len = 0u64;
        let initial_list_position = pages.position();

        // The callbacks both need the object and its locked state, which the splice list iteration
        // cannot express, so they share them through a RefCell.
        let context = core::cell::RefCell::new((
            &mut *locked,
            &mut *deferred,
            &mut new_pages_start,
            &mut new_pages_len,
            &mut supplied_pages_len,
        ));
        // Handles previous content that was overwritten.
        let handle_add_page_result = |old: &mut VmPageOrMarker, deferred: &mut DeferredOps| {
            if old.is_page() {
                let page = old.release_page();
                page_queues().remove(page);
                deferred.freed_list().push_back(page);
            }
        };
        // Handles an insertion failing because content already exists.
        let handle_add_page_error = |status: Status,
                                     offset: u64,
                                     locked: &mut VmCowPagesLocked,
                                     deferred: &mut DeferredOps,
                                     new_pages_start: &mut u64,
                                     new_pages_len: &mut u64,
                                     supplied_pages_len: &mut u64|
         -> Status {
            if parent_content_markers {
                return status;
            }
            if status == Status::ALREADY_EXISTS {
                // We hit the end of a run of absent pages, so notify the page source of any new
                // pages that were added and reset the tracking variables.
                if *new_pages_len > 0 {
                    self.range_change_update_locked(
                        locked,
                        VmCowRange::new(*new_pages_start, *new_pages_len),
                        RangeChangeOp::Unmap,
                        Some(deferred),
                    );
                    if let Some(source) = &self.page_source {
                        source.on_pages_supplied(*new_pages_start, *new_pages_len);
                    }
                }
                *new_pages_start = offset + PAGE_SIZE;
                *new_pages_len = 0;
                *supplied_pages_len += PAGE_SIZE;
                Status::NEXT
            } else {
                // Only cause for this should be running out of memory for page list nodes.
                assert_eq!(status, Status::NO_MEMORY);
                status
            }
        };

        let status = pages.remove_pages_and_iterate_gaps(
            |mut slot, src_offset| {
                let mut ctx = context.borrow_mut();
                let (locked, deferred, new_pages_start, new_pages_len, supplied_pages_len) =
                    &mut *ctx;
                debug_assert!(!slot.is_interval() && !slot.is_empty());
                let dst_offset = start + src_offset;
                debug_assert!(
                    options == SupplyOptions::PhysicalPageProvider
                        || !slot.is_page()
                        || arena_of(slot.page()) == self.arena,
                    "a page of another arena supplied: see `process_pages_for_supply`"
                );
                // A newly supplied page starts off as Clean.
                if slot.is_page() && self.page_source_type() == PageSourceType::UserPager {
                    self.update_dirty_state_locked(
                        slot.page(),
                        dst_offset,
                        DirtyState::Clean,
                        true,
                        locked.high_priority_count,
                    );
                }
                let result = if options == SupplyOptions::PhysicalPageProvider {
                    // Pages from the PhysicalPageProvider are not yet initialized.
                    let page = slot.release_page();
                    self.add_new_page_locked(
                        locked,
                        dst_offset,
                        page,
                        overwrite_policy,
                        false,
                        None,
                    )
                } else {
                    let content = slot.swap(VmPageOrMarker::empty());
                    self.add_page_locked(locked, dst_offset, content, overwrite_policy, None)
                };
                match result {
                    Ok(mut old) => handle_add_page_result(&mut old, deferred),
                    Err(status) => {
                        return handle_add_page_error(
                            status,
                            dst_offset,
                            locked,
                            deferred,
                            new_pages_start,
                            new_pages_len,
                            supplied_pages_len,
                        );
                    }
                }
                **new_pages_len += PAGE_SIZE;
                debug_assert!(**new_pages_start + **new_pages_len <= end);
                **supplied_pages_len += PAGE_SIZE;
                Status::NEXT
            },
            |gap_start, gap_end| {
                let mut ctx = context.borrow_mut();
                let (locked, deferred, new_pages_start, new_pages_len, supplied_pages_len) =
                    &mut *ctx;
                let gap_dst_start = gap_start + start;
                let gap_dst_end = gap_end + start;
                if parent_content_markers {
                    // Any gaps in the splice list imply zeroes, which here can be represented by
                    // ensuring the corresponding range in this VMO is empty.
                    let len = gap_dst_end - gap_dst_start;
                    self.release_owned_pages_range_locked(
                        locked,
                        gap_dst_start,
                        len,
                        deferred.freed_list(),
                    );
                    **supplied_pages_len += len;
                    **new_pages_len += len;
                } else {
                    // The source VMO of supply pages cannot have a page source, so empty pages are
                    // zeroes and explicit markers are inserted to resolve the pager fault.
                    let mut zero_offset = gap_dst_start;
                    while zero_offset < gap_dst_end {
                        match self.add_page_locked(
                            locked,
                            zero_offset,
                            VmPageOrMarker::marker(),
                            overwrite_policy,
                            None,
                        ) {
                            Ok(mut old) => {
                                handle_add_page_result(&mut old, deferred);
                                **new_pages_len += PAGE_SIZE;
                                **supplied_pages_len += PAGE_SIZE;
                            }
                            Err(status) => {
                                let status = handle_add_page_error(
                                    status,
                                    zero_offset,
                                    locked,
                                    deferred,
                                    new_pages_start,
                                    new_pages_len,
                                    supplied_pages_len,
                                );
                                if status != Status::NEXT {
                                    return status;
                                }
                            }
                        }
                        zero_offset += PAGE_SIZE;
                    }
                }
                Status::NEXT
            },
        );

        // Unless there was an error, there should have been the correct number of pages.
        debug_assert!(start + supplied_pages_len == end || status.is_err());
        if new_pages_len > 0 {
            self.range_change_update_locked(
                locked,
                VmCowRange::new(new_pages_start, new_pages_len),
                RangeChangeOp::Unmap,
                Some(deferred),
            );
            if let Some(source) = &self.page_source {
                source.on_pages_supplied(new_pages_start, new_pages_len);
            }
        }
        debug_assert!(
            pages.position() - initial_list_position == supplied_pages_len || status.is_err()
        );
        status
    }

    /// Fails currently outstanding page requests in the range.
    pub fn fail_page_requests_locked(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
        error_status: Status,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        let source = self.page_source.as_ref().expect("page source");
        ensure!(PageSource::is_valid_internal_failure_code(error_status), Status::INVALID_ARGS);
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        ensure!(!source.is_detached(), Status::BAD_STATE);
        source.on_pages_failed(range.offset, range.len, error_status);
        Ok(())
    }

    /// Marks the pages in the range dirty, in response to DIRTY requests. Zero markers and zero
    /// intervals are replaced with pages from `alloc_list`, allocating into it as needed so that
    /// successive calls make forward progress.
    pub fn dirty_pages(
        self: &Arc<Self>,
        range: VmCowRange,
        alloc_list: &mut VmPageDoublyLinkedList,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        let source = self.page_source.as_ref().expect("page source").clone();
        ensure!(source.should_trap_dirty_transitions(), Status::NOT_SUPPORTED);
        debug_assert_eq!(self.page_source_type(), PageSourceType::UserPager);
        let start_offset = range.offset;
        let end_offset = range.end();

        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        ensure!(start_offset <= locked.size, Status::OUT_OF_RANGE);
        // Overflow check.
        ensure!(end_offset >= start_offset, Status::OUT_OF_RANGE);

        // After the above checks, the page source has tried to respond correctly to a range of
        // dirty requests, so the kernel should resolve those outstanding dirty requests, even in
        // the failure case.
        let result = self.dirty_pages_locked(
            &mut locked,
            &mut deferred,
            &source,
            start_offset,
            end_offset,
            alloc_list,
            page_request,
        );
        match result {
            // If propagating up ZX_ERR_SHOULD_WAIT do not invalidate the dirty request as we are
            // going to retry it.
            Ok(()) | Err(Status::SHOULD_WAIT) => {}
            Err(status) => self.dirty_pages_failed_locked(&locked, range, status),
        }
        result
    }

    /// fxdax: resolves the DIRTY requests in `range` once the page source's response to them,
    /// dirtying their pages (`dirty_pages`), has failed with `status`, under the lock, `locked`:
    /// spuriously, as upstream, for their writers to try again; but, the pages failing for want of
    /// memory, failed with `NO_MEMORY`, which their writers then fail with (TASK-79.75). Upstream's
    /// writers try again until its response to memory running out has made room, or stopped the
    /// system. The kernel's makes none, as there are no jobs to kill; and with memory never out
    /// (`OomBehavior::Evict`), the default, an allocation fails only once nothing would make it
    /// room: the writers asked again for ever, as Fxfs's writes to its page cache did.
    ///
    /// Failed for want of memory, they fail as one: every DIRTY request outstanding over the pages
    /// of `range` the response left undirtied, whatever their pages need. That includes a request
    /// that only overlaps the range, at the page its writer waits for, and the request of a writer
    /// of a page already committed, which needs no page allocated to be dirtied: resolved
    /// spuriously, it would have asked again and succeeded. Telling them apart would take the
    /// requests sorted by what their pages need; with memory out, what it costs is such a write
    /// failing, which asking again would have let through.
    fn dirty_pages_failed_locked(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
        status: Status,
    ) {
        // Those past the end, should the object have shrunk since the response began, its resize
        // resolved.
        let Some(len) = locked.size.checked_sub(range.offset) else { return };
        let failure = (status == Status::NO_MEMORY).then_some(status);
        self.invalidate_dirty_requests_locked(locked, range.offset, len.min(range.len), failure);
    }

    /// fxdax: resolves the DIRTY requests in `range` as `dirty_pages_failed_locked` does, taking
    /// the lock: for a response that failed waiting for a page to dirty them with
    /// (`VmObjectPaged::dirty_pages`), whose requests upstream leaves outstanding, and their writers
    /// waiting for ever.
    pub fn dirty_pages_failed(&self, range: VmCowRange, status: Status) {
        let locked = self.lock();
        self.dirty_pages_failed_locked(&locked, range, status);
    }

    #[allow(clippy::too_many_arguments)]
    fn dirty_pages_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        deferred: &mut DeferredOps,
        source: &Arc<PageSource>,
        start_offset: u64,
        end_offset: u64,
        alloc_list: &mut VmPageDoublyLinkedList,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<(), Status> {
        // The page source may have tried to mark a larger range than necessary as dirty.
        ensure!(end_offset <= locked.size, Status::OUT_OF_RANGE);
        ensure!(!source.is_detached(), Status::BAD_STATE);

        // If any of the pages in the range are zero page markers (Clean zero pages), they need to
        // be forked in order to be dirtied. We also need to allocate zero pages to replace sparse
        // zero intervals, and pages to copy borrowed pages to. Find the number of such pages.
        let mut new_pages_count = 0u64;
        let mut interval_start = start_offset;
        let mut unmatched_interval_start = false;
        let mut found_page_or_gap = false;
        let found_gap = core::cell::Cell::new(false);
        locked.page_list.for_every_page_and_gap_in_range(
            start_offset,
            end_offset,
            |p, off| {
                found_page_or_gap = true;
                if p.is_marker() {
                    new_pages_count += 1;
                    return Status::NEXT;
                }
                if p.is_interval_zero() {
                    if p.is_interval_start() {
                        interval_start = off;
                        unmatched_interval_start = true;
                    } else if p.is_interval_end() {
                        new_pages_count += (off - interval_start + PAGE_SIZE) / PAGE_SIZE;
                        unmatched_interval_start = false;
                    } else {
                        debug_assert!(p.is_interval_slot());
                        new_pages_count += 1;
                    }
                    return Status::NEXT;
                }
                debug_assert!(p.is_page());
                if self.is_borrowed(p.page()) {
                    new_pages_count += 1;
                }
                Status::NEXT
            },
            |_, _| {
                found_gap.set(true);
                // A gap indicates a page that has not been supplied yet, possibly evicted after
                // the DIRTY request was generated. Spuriously resolve the DIRTY page request, and
                // let the waiter(s) retry looking up the page, which will generate a READ request.
                Status::NOT_FOUND
            },
        )?;
        let found_page_or_gap = found_page_or_gap || found_gap.get();

        // Handle the last interval or if we did not enter the traversal callbacks at all.
        if unmatched_interval_start || !found_page_or_gap {
            debug_assert!(found_page_or_gap || interval_start == start_offset);
            new_pages_count += (end_offset - interval_start) / PAGE_SIZE;
        }

        // If we have found any zero pages to populate or borrowed pages to copy, then we need to
        // allocate and transition them to the dirty state.
        if new_pages_count > 0 {
            // Allocate the number of pages required upfront, so that we can fail the call early if
            // the page allocation fails.
            let alloc_list_len = alloc_list.size_slow() as u64;
            new_pages_count = new_pages_count.saturating_sub(alloc_list_len);
            // First try to allocate all the pages at once, falling back to one page at a time,
            // giving reclamation strategies a better chance to catch up.
            let (count, flags) = (new_pages_count as usize, self.pmm_alloc_flags);
            match pmm::node().alloc_pages_in(self.arena, count, flags, alloc_list) {
                Ok(()) => new_pages_count = 0,
                Err(Status::SHOULD_WAIT) => {}
                Err(status) => return Err(status),
            }
            while new_pages_count > 0 {
                // We will initialize this page later when adding it.
                let page = self.alloc_uninitialized_page(page_request)?;
                alloc_list.push_back(page);
                new_pages_count -= 1;
            }

            // We have to mark all the requested pages Dirty *atomically*, so evaluate any failure
            // conditions before starting. Install page slots for all the intervals we'll be adding
            // zero pages in.
            let mut next_start_offset = start_offset;
            loop {
                let mut found_interval = None;
                let status = locked.page_list.for_every_page_and_contiguous_run_in_range(
                    |p, _| p.is_interval_start() || p.is_interval_end(),
                    |p, _| {
                        debug_assert!(p.is_interval_zero());
                        Status::NEXT
                    },
                    |start, end, is_interval| {
                        debug_assert!(is_interval);
                        found_interval = Some((start, end));
                        Status::STOP
                    },
                    next_start_offset,
                    end_offset,
                );
                debug_assert!(status.is_ok());
                // No intervals remain.
                let Some((start, end)) = found_interval else { break };
                debug_assert!(end - start >= PAGE_SIZE);
                if let Err(status) = locked.page_list.populate_slots_in_interval(start, end) {
                    debug_assert_eq!(status, Status::NO_MEMORY);
                    // Undo any slots we might have populated in intervals we previously encountered.
                    let mut off = start_offset;
                    while off < start {
                        if locked.page_list.lookup(off).is_some_and(|s| s.is_interval_slot()) {
                            locked.page_list.return_interval_slot(off);
                        }
                        off += PAGE_SIZE;
                    }
                    return Err(status);
                }
                next_start_offset = end;
                if next_start_offset >= end_offset {
                    break;
                }
            }

            // All operations from this point on must succeed. Install newly allocated pages in
            // place of the zero page markers, interval slots and borrowed pages, as clean pages to
            // be transitioned to dirty uniformly with pager supplied pages below.
            let mut offsets = Vec::new();
            let _ = locked.page_list.for_every_page_in_range(start_offset, end_offset, |p, off| {
                if p.is_page() && self.is_borrowed(p.page()) {
                    offsets.push((off, true));
                } else if p.is_marker() || p.is_interval_slot() {
                    offsets.push((off, false));
                }
                Status::NEXT
            });
            for (off, borrowed) in offsets {
                if borrowed {
                    self.replace_borrowed_page_locked(
                        locked,
                        off,
                        Some(&mut *alloc_list),
                        deferred,
                        page_request,
                    )
                    .expect("replacing a borrowed page with an allocated page cannot fail");
                    continue;
                }
                let page = alloc_list.pop_front().expect("enough pages were allocated");
                // add_new_page_locked will not fail as markers and interval slots can be
                // overwritten and their slots already exist.
                let old = self
                    .add_new_page_locked(
                        locked,
                        off,
                        page,
                        CanOverwriteSlot::ZeroMarkerOrInterval,
                        true,
                        Some(deferred),
                    )
                    .expect("replacing zero content cannot fail");
                debug_assert!(!old.is_page_or_ref());
            }
        }

        let high_priority_count = locked.high_priority_count;
        let status = locked.page_list.for_every_page_and_contiguous_run_in_range(
            |p, _| {
                if p.is_page() {
                    let page = p.page();
                    debug_assert!(is_page_dirty_tracked(page));
                    return !is_page_dirty(page);
                }
                false
            },
            |p, off| {
                debug_assert!(p.is_page());
                self.update_dirty_state_locked(
                    p.page(),
                    off,
                    DirtyState::Dirty,
                    false,
                    high_priority_count,
                );
                Status::NEXT
            },
            |start, end, _| {
                source.on_pages_dirtied(start, end - start);
                Status::NEXT
            },
            start_offset,
            end_offset,
        );
        debug_assert!(status.is_ok());
        status
    }

    /// Calls `dirty_range_fn` with each range of dirty (or awaiting clean) content in `range`.
    pub fn enumerate_dirty_ranges_locked<F>(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
        mut dirty_range_fn: F,
    ) -> Result<(), Status>
    where
        F: FnMut(u64, u64, bool) -> Status,
    {
        // Dirty pages are only tracked if the page source is a user pager.
        ensure!(self.page_source_type() == PageSourceType::UserPager, Status::NOT_SUPPORTED);
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        locked.page_list.for_every_page_and_contiguous_run_in_range(
            |p, _| {
                // Enumerate both AwaitingClean and Dirty pages, i.e. anything that is not Clean.
                if p.is_page() {
                    return !is_page_clean(p.page());
                }
                // Enumerate any dirty zero intervals.
                if p.is_interval_zero() {
                    // For now we do not support clean intervals.
                    debug_assert!(!p.is_zero_interval_clean());
                    return p.is_zero_interval_dirty();
                }
                // Pager-backed VMOs cannot have compressed references, so this is a marker.
                debug_assert!(p.is_marker());
                false
            },
            |_, _| Status::NEXT,
            // Zero intervals are enumerated as zero ranges.
            |start, end, is_interval| dirty_range_fn(start, end - start, is_interval),
            round_down_page_size(range.offset),
            round_up_page_size(range.end()),
        )
    }

    /// Transitions Dirty pages and dirty zero intervals in the range to AwaitingClean, and removes
    /// write permission from their mappings. With `is_zero_range`, only zero intervals transition,
    /// as the pager intends to write back zeroes.
    pub fn writeback_begin_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        is_zero_range: bool,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        assert!(self.page_source.is_some());
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        ensure!(self.page_source_type() == PageSourceType::UserPager, Status::NOT_SUPPORTED);
        let start_offset = range.offset;
        let end_offset = range.end();
        let high_priority_count = locked.high_priority_count;
        let mut interval_start: Option<*mut VmPageOrMarker> = None;
        let mut interval_start_off = 0u64;
        let status =
            locked.page_list.for_every_page_in_range_mut(start_offset, end_offset, |p, off| {
                // If the page is pinned we have to leave it Dirty in case it is still being written to
                // via DMA. If the caller indicates that they're only cleaning zero pages, any committed
                // pages need to be left dirty.
                if p.is_page() && (pin_count(p.page()) > 0 || is_zero_range) {
                    return Status::NEXT;
                }
                // Transition pages from Dirty to AwaitingClean.
                if p.is_page() && is_page_dirty(p.page()) {
                    self.update_dirty_state_locked(
                        p.page(),
                        off,
                        DirtyState::AwaitingClean,
                        false,
                        high_priority_count,
                    );
                    return Status::NEXT;
                }
                // Transition dirty zero intervals to AwaitingClean.
                if p.is_interval_zero() {
                    if !p.is_zero_interval_dirty() {
                        // The only other state we support is Untracked.
                        debug_assert!(p.is_zero_interval_untracked());
                        return Status::NEXT;
                    }
                    if p.is_interval_start() || p.is_interval_slot() {
                        // Start tracking a dirty interval. It will only transition once the end is
                        // encountered.
                        debug_assert!(interval_start.is_none());
                        interval_start = Some(p);
                        interval_start_off = off;
                    }
                    if p.is_interval_end() || p.is_interval_slot() {
                        // Now that we've encountered the end, the entire interval can be transitioned
                        // to AwaitingClean by setting the AwaitingCleanLength of the start sentinel.
                        if let Some(start) = interval_start {
                            // SAFETY: The start sentinel is in the page list, which the traversal does
                            // not modify, and it is not otherwise referenced.
                            let start = unsafe { &mut *start };
                            let old_len = start.zero_interval_awaiting_clean_length();
                            start.set_zero_interval_awaiting_clean_length(
                                (off - interval_start_off + PAGE_SIZE).max(old_len),
                            );
                        }
                        interval_start = None;
                    }
                    return Status::NEXT;
                }
                // This was either a marker (which is already clean), or a non-Dirty page.
                debug_assert!(p.is_marker() || !is_page_dirty(p.page()));
                Status::NEXT
            });
        debug_assert!(status.is_ok());

        // Process the last partial interval.
        if let Some(start) = interval_start {
            // SAFETY: As above.
            let start = unsafe { &mut *start };
            debug_assert!(start.is_interval_start());
            let old_len = start.zero_interval_awaiting_clean_length();
            start.set_zero_interval_awaiting_clean_length(
                (end_offset - interval_start_off).max(old_len),
            );
        }

        // Set any mappings for this range to read-only, so that a permission fault is triggered the
        // next time the page is written to in order for us to track it as dirty.
        self.range_change_update_locked(
            locked,
            VmCowRange::new(start_offset, end_offset - start_offset),
            RangeChangeOp::RemoveWrite,
            None,
        );
        Ok(())
    }

    /// Transitions AwaitingClean pages in the range to Clean, and removes zero intervals whose
    /// writeback began and completed.
    pub fn writeback_end_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned());
        assert!(self.page_source.is_some());
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        ensure!(self.page_source_type() == PageSourceType::UserPager, Status::NOT_SUPPORTED);
        let start_offset = range.offset;
        let end_offset = range.end();
        let high_priority_count = locked.high_priority_count;

        // This tracks the end offset until which all zero intervals can be marked clean. When zero
        // intervals are truncated or split their AwaitingCleanLength is not updated, so it is
        // carried across intervals as a running maximum.
        let mut interval_awaiting_clean_end = 0u64;
        // Mark any AwaitingClean pages Clean. Zero intervals are handled outside the traversal to
        // allow for safely modifying the page list.
        let mut remove_start = start_offset;
        while remove_start < end_offset {
            let mut interval_offset = u64::MAX;
            let _ = locked.page_list.remove_pages(remove_start, end_offset, |p, off| {
                // Transition pages from AwaitingClean to Clean.
                if p.is_page() && is_page_awaiting_clean(p.page()) {
                    self.update_dirty_state_locked(
                        p.page(),
                        off,
                        DirtyState::Clean,
                        false,
                        high_priority_count,
                    );
                    return Status::NEXT;
                }
                // Exit out to handle any zero intervals.
                if p.is_interval_zero() {
                    if !p.is_zero_interval_dirty() {
                        debug_assert!(p.is_zero_interval_untracked());
                        return Status::NEXT;
                    }
                    // If we see an end interval then we have started processing somewhere inside a
                    // zero interval. In this case there's nothing for us to do.
                    if p.is_interval_end() {
                        return Status::NEXT;
                    }
                    interval_offset = off;
                    return Status::STOP;
                }
                // This was either a marker (which is already clean), or a non-AwaitingClean page.
                debug_assert!(p.is_marker() || !is_page_awaiting_clean(p.page()));
                Status::NEXT
            });
            // If no interval found then we are done.
            if interval_offset >= end_offset {
                break;
            }
            let interval = locked.page_list.lookup(interval_offset).expect("interval");
            debug_assert!(
                interval.is_interval_zero()
                    && interval.is_zero_interval_dirty()
                    && !interval.is_interval_end()
            );
            let is_slot = interval.is_interval_slot();
            let awaiting_clean_length = interval.zero_interval_awaiting_clean_length();
            // First determine where this interval ends, which is where we would resume iteration.
            let mut interval_end = interval_offset;
            if !is_slot {
                let _ = locked.page_list.for_every_page_in_range(
                    interval_offset + PAGE_SIZE,
                    VmPageList::MAX_SIZE,
                    |slot, offset| {
                        debug_assert!(slot.is_interval_end());
                        interval_end = offset;
                        Status::STOP
                    },
                );
                debug_assert!(interval_end > interval_offset);
            }
            remove_start = interval_end + PAGE_SIZE;

            // See if we can advance interval_awaiting_clean_end to include the AwaitingCleanLength
            // of this interval.
            interval_awaiting_clean_end =
                interval_awaiting_clean_end.max(interval_offset + awaiting_clean_length);

            // Handle interval slots separately for simplicity.
            if is_slot {
                if interval_offset < interval_awaiting_clean_end {
                    let _ = locked.page_list.remove_content(interval_offset);
                }
                continue;
            }
            if interval_end <= end_offset && interval_awaiting_clean_end > interval_end {
                // The entire interval is clean (and within the range of our operation), so can
                // remove it.
                let _ = locked.page_list.remove_content(interval_offset);
                let _ = locked.page_list.remove_content(interval_end);
            } else {
                // The entire interval cannot be marked clean. Move forward the start by awaiting
                // clean length. Cleaning is best effort, so ignore any errors.
                let clean_length = interval_awaiting_clean_end.min(end_offset) - interval_offset;
                let _ = locked.page_list.clip_interval_start(interval_offset, clean_length);
            }
        }
        Ok(())
    }

    /// Detaches the page source, removing all clean pages so future faults fail deterministically.
    /// Dirty pages are retained so they can be written back.
    pub fn detach_source(self: &Arc<Self>) {
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        let source = self.page_source.as_ref().expect("page source");
        source.detach();
        // Unmap everything at once as an optimization.
        let size = locked.size;
        self.range_change_update_locked(
            &mut locked,
            VmCowRange::new(0, size),
            RangeChangeOp::Unmap,
            Some(&mut deferred),
        );
        let mut page_remover = BatchPqRemove::new(self, deferred.freed_list());
        let _ = locked.page_list.remove_pages(0, size, |p, off| {
            // A marker is a clean zero page. Replace it with an empty slot.
            if p.is_marker() {
                *p = VmPageOrMarker::empty();
                return Status::NEXT;
            }
            // Zero intervals are dirty so they cannot be removed.
            if p.is_interval_zero() {
                debug_assert!(!p.is_zero_interval_clean());
                return Status::NEXT;
            }
            let page = p.page();
            // We cannot remove the page if it is dirty-tracked but not clean, or if it's pinned.
            if (is_page_dirty_tracked(page) && !is_page_clean(page)) || pin_count(page) > 0 {
                return Status::NEXT;
            }
            page_remover.push(off, p.release_page());
            Status::NEXT
        });
        page_remover.flush();
    }

    /// Applies `op` to the mappings of `range`.
    pub fn range_change_update_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        range: VmCowRange,
        op: RangeChangeOp,
        deferred: Option<&mut DeferredOps>,
    ) {
        // Only roots are supported, so there are no children to defer operations to; the
        // DeferredOps serves only to serialize operations on the hierarchy.
        debug_assert!(
            !self.root_has_page_source() || deferred.is_some() || op == RangeChangeOp::RemoveWrite
        );
        if !range.is_empty() {
            let range = range.expand_till_page_aligned();
            self.range_change_update_mappings_locked(locked, range, op);
        }
    }

    /// Applies `op` to the mappings of `range`, skipping pinned pages.
    fn range_change_update_mappings_locked(
        &self,
        locked: &VmCowPagesLocked,
        range: VmCowRange,
        op: RangeChangeOp,
    ) {
        debug_assert!(range.is_page_aligned());
        if locked.mappings.is_empty() {
            return;
        }
        let update = |range: VmCowRange| {
            for mapping in locked.mappings.values() {
                mapping.range_change_update(locked, range, op);
            }
        };
        if locked.pinned_page_count == 0 {
            update(range);
            return;
        }
        // Skip pinned pages, which may be in use by the kernel.
        let mut maybe_unpinned_start = range.offset;
        let _ =
            locked.page_list.for_every_page_in_range(range.offset, range.end(), |slot, offset| {
                if !slot.is_page() || pin_count(slot.page()) == 0 {
                    return Status::NEXT;
                }
                if offset > maybe_unpinned_start {
                    update(VmCowRange::new(maybe_unpinned_start, offset - maybe_unpinned_start));
                }
                maybe_unpinned_start = offset + PAGE_SIZE;
                Status::NEXT
            });
        if range.end() > maybe_unpinned_start {
            update(VmCowRange::new(maybe_unpinned_start, range.end() - maybe_unpinned_start));
        }
    }

    /// Attempts to reclaim `page` at `offset`, which may also reclaim other pages.
    pub fn reclaim_page(
        self: &Arc<Self>,
        page: VmPagePtr,
        offset: u64,
        hint_action: EvictionAction,
    ) -> VmCowReclaimResult {
        // See if we can reclaim by eviction.
        if self.can_evict() {
            // Evict pages in aligned batches of 16, which will correspond to a single
            // VmPageListNode.
            const EVICTION_LENGTH: u64 = 16 * PAGE_SIZE;
            let offset = offset & !(EVICTION_LENGTH - 1);
            return self.reclaim_range_for_eviction(offset, EVICTION_LENGTH, hint_action);
        }
        // No other strategies, so to avoid this page remaining in a reclamation list we simulate
        // an access, if the page is still ours.
        let locked = self.lock();
        match locked.page_list.lookup(offset) {
            Some(slot) if slot.is_page() && slot.page() == page => {
                page_queues().mark_accessed(page);
                Err(VmCowReclaimFailure::Other)
            }
            _ => Err(VmCowReclaimFailure::IncorrectPage),
        }
    }

    /// fxdax: holds the object's pages from eviction, or, unless `hold`, lets go of a hold
    /// (`VmCowPages::eviction_holds`): `BAD_STATE` if none is held. A DAX window's second stage
    /// still mapping pages the window has stopped mapping, as a VM process does until it makes the
    /// change, holds their object so until then: its pages, which the guest may still reach, are
    /// neither freed nor reused meanwhile (TASK-92.50). An object that evicts none is held all the
    /// same. A hold taken before the window stops mapping the pages, which takes the lock, is seen
    /// by every eviction that finds them no longer mapped there, which takes it after. It doesn't
    /// hold pages the object borrowed, which their lender takes back without it: fxdax's extents
    /// borrow none, as it supplies none shared (`VmObjectPaged::supply_shared_pages`).
    pub fn hold_from_eviction(&self, hold: bool) -> Result<(), Status> {
        if hold {
            self.eviction_holds.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        let mut holds = self.eviction_holds.load(Ordering::SeqCst);
        loop {
            ensure!(holds > 0, Status::BAD_STATE);
            let let_go = self.eviction_holds.compare_exchange_weak(
                holds,
                holds - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            match let_go {
                Ok(_) => return Ok(()),
                Err(now) => holds = now,
            }
        }
    }

    /// Evicts the clean, reclaimable pages in the range.
    pub fn reclaim_range_for_eviction(
        self: &Arc<Self>,
        offset: u64,
        length: u64,
        eviction_action: EvictionAction,
    ) -> VmCowReclaimResult {
        self.reclaim_chosen_for_eviction(offset, length, eviction_action, |_, _| true)
    }

    /// fxdax: evicts on demand (`EvictionAction::Demand`) those of `pages`, at consecutive offsets
    /// from `offset`, that are there still, and returns how many it evicted: for eviction of an
    /// arena's own pages, which chooses them by their age without this object's lock
    /// (`Evictor::evict_arena_on_demand`, TASK-79.76). A page found in one's place, as it was
    /// evicted or freed since and another supplied there, newer, is left.
    pub fn evict_pages_on_demand(self: &Arc<Self>, offset: u64, pages: &[VmPagePtr]) -> u64 {
        let length = pages.len() as u64 * PAGE_SIZE;
        let chosen = |page_offset: u64, page: VmPagePtr| {
            !rules().evictions_take_only_the_pages_chosen
                || pages[((page_offset - offset) / PAGE_SIZE) as usize] == page
        };
        let evicted =
            self.reclaim_chosen_for_eviction(offset, length, EvictionAction::Demand, chosen);
        evicted.map_or(0, |evicted| evicted.num_pages)
    }

    /// Evicts the clean, reclaimable pages in the range that `chosen`, given each one's offset,
    /// chooses. fxdax: every one, for `reclaim_range_for_eviction`, or those an eviction of an
    /// arena's own pages chose (`evict_pages_on_demand`, TASK-79.76).
    fn reclaim_chosen_for_eviction(
        self: &Arc<Self>,
        offset: u64,
        length: u64,
        eviction_action: EvictionAction,
        chosen: impl Fn(u64, VmPagePtr) -> bool,
    ) -> VmCowReclaimResult {
        debug_assert!(self.can_evict());
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        let length = length.min(locked.size.saturating_sub(offset));
        if length == 0 {
            return Err(VmCowReclaimFailure::IncorrectPage);
        }
        // fxdax: read with the lock held, before the range is unmapped: a held object's pages
        // stay mapped, as none is evicted, rather than each mapping's second stage waited for
        // (TASK-92.50).
        let eviction_holds = self.eviction_holds.load(Ordering::SeqCst);
        // Remove any mappings to the range and harvest accessed bits to get up-to-date page queue
        // info.
        if eviction_holds == 0 {
            self.range_change_update_locked(
                &mut locked,
                VmCowRange::new(offset, length),
                RangeChangeOp::UnmapAndHarvest,
                Some(&mut deferred),
            );
        }

        let high_priority_count = locked.high_priority_count;
        let mut num_failed_queue = 0u32;
        let can_reclaim_page = |page: VmPagePtr, num_failed_queue: &mut u32| {
            debug_assert!(is_page_dirty_tracked(page));
            // Not allowed to reclaim range if high priority. fxdax: nor while held from eviction.
            if high_priority_count != 0 || eviction_holds != 0 {
                page_queues().mark_accessed(page);
                return false;
            }
            // We cannot evict the page unless it is clean.
            if !is_page_clean(page) {
                debug_assert!(page_queues().debug_page_is_pager_backed_dirty(page));
                return false;
            }
            // Do not evict if the always_need hint is set, unless we are told to ignore it. Move
            // the page out of the way so the eviction loop can make progress.
            // SAFETY: The lock is held.
            if unsafe { page.object().always_need() }
                && eviction_action == EvictionAction::FollowHint
            {
                page_queues().mark_accessed(page);
                return false;
            }
            // Pages must be reclaimable, but fxdax's on demand, whatever their age.
            if eviction_action != EvictionAction::Demand && !PageQueues::is_page_reclaimable(page) {
                *num_failed_queue += 1;
                return false;
            }
            // Pinned pages could be in use by DMA so we cannot safely reclaim them.
            if pin_count(page) != 0 {
                page_queues().mark_accessed(page);
                return false;
            }
            true
        };

        let mut page_remover = BatchPqRemove::new(self, deferred.freed_list());
        let mut num_evicted_pages = 0u64;
        let mut num_failed_pages = 0u32;
        let _ = locked.page_list.remove_pages(offset, offset + length, |p, off| {
            // Borrowed pages are aged and evicted by their lender, and giving one back would not
            // free it. fxdax: and a page not chosen is left as it is.
            if p.is_page() && !self.is_borrowed(p.page()) && chosen(off, p.page()) {
                if !can_reclaim_page(p.page(), &mut num_failed_queue) {
                    num_failed_pages += 1;
                    return Status::NEXT;
                }
                num_evicted_pages += 1;
                page_remover.push_content(p, off);
            }
            Status::NEXT
        });
        page_remover.flush();

        if num_evicted_pages == 0 {
            // If all of the failures were from "wrong queue", consider the range accessed.
            if num_failed_pages == num_failed_queue {
                return Err(VmCowReclaimFailure::EvictAccessed);
            }
            return Err(VmCowReclaimFailure::Other);
        }
        locked.reclamation_event_count += 1;
        Ok(VmCowReclaimSuccess { r#type: ReclaimType::Evict, num_pages: num_evicted_pages })
    }

    /// Moves the clean pages of the range to be reclaimed first, for the DONT_NEED hint.
    pub fn promote_range_for_reclamation(&self, range: VmCowRange) -> Result<(), Status> {
        // Hints only apply to pager backed VMOs.
        if !self.can_evict() {
            return Ok(());
        }
        // Zero lengths have no work to do.
        if range.is_empty() {
            return Ok(());
        }
        let mut locked = self.lock();
        ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
        let mut start_offset = round_down_page_size(range.offset);
        let end_offset = round_up_page_size(range.end());
        let mut cursor = self.get_lookup_cursor_locked(
            &locked,
            VmCowRange::new(start_offset, end_offset - start_offset),
        )?;
        // Do not consider pages accessed as the goal is reclaim them, not consider them used.
        cursor.disable_mark_accessed();
        while start_offset < end_offset {
            // Lookup the page if it exists, but do not let it get allocated or say we are writing
            // to it. On success or failure this causes the cursor to go to the next offset.
            if let Some(page) = cursor.maybe_page(&mut locked, false) {
                // Don't move a pinned page or a dirty page to the DontNeed queue. Note that this
                // does not unset the always_need bit if it has been previously set. The
                // always_need hint is sticky. Only roots are supported, but a page we borrowed is
                // not ours to hint.
                if self.page_source.is_some()
                    && !self.is_borrowed(page)
                    && pin_count(page) == 0
                    && is_page_clean(page)
                {
                    page_queues().move_to_reclaim_dont_need(page);
                }
            }
            // Can't really do anything in case an error is encountered while looking up the page.
            // Simply ignore it and move on to the next page. Hints are best effort anyway.
            start_offset += PAGE_SIZE;
        }
        Ok(())
    }

    /// Pages in the range, protecting its pages from reclamation with `set_always_need`, for the
    /// ALWAYS_NEED hint and prefetching.
    pub fn protect_range_from_reclamation(
        self: &Arc<Self>,
        range: VmCowRange,
        set_always_need: bool,
        ignore_errors: bool,
    ) -> Result<(), Status> {
        // Hints only apply to pager backed VMOs.
        if !self.can_evict() {
            return Ok(());
        }
        // Validate that the range is completely in range at the start of the operation. Although
        // we tolerate the VMO shrinking during the operation, the range must be valid at the point
        // we started.
        {
            let locked = self.lock();
            ensure!(range.is_bounded_by(locked.size), Status::OUT_OF_RANGE);
            // Zero lengths have no work to do.
            if range.is_empty() {
                return Ok(());
            }
        }
        let mut range = range.expand_till_page_aligned();
        let mut page_request = MultiPageRequest::default();
        while !range.is_empty() {
            let status = {
                let mut deferred = DeferredOps::new(self);
                let mut locked = self.lock();
                // The size might have changed since we dropped the lock. Adjust the range if
                // required.
                if range.offset >= locked.size {
                    // No more pages to hint.
                    return Ok(());
                }
                // Shrink the range if required. Proceed with hinting on the remaining pages in the
                // range; we've already hinted on the preceding pages, so just go on ahead instead
                // of returning an error. The range was valid at the time we started hinting.
                if !range.is_bounded_by(locked.size) {
                    range = VmCowRange::new(range.offset, locked.size - range.offset);
                }
                let mut cursor = self.get_lookup_cursor_locked(&locked, range)?;
                let mut status = Ok(());
                while !range.is_empty() {
                    // Lookup the page, this will fault in the page from the parent if necessary,
                    // but will not allocate pages directly in this if it is a child.
                    let result = cursor.require_page(
                        self,
                        &mut locked,
                        false,
                        range.len / PAGE_SIZE,
                        &mut deferred,
                        &mut page_request,
                    );
                    let page = match result {
                        Ok(result) => result.page,
                        Err(error) => {
                            status = Err(error);
                            break;
                        }
                    };
                    // Check to see if the page is owned by the root VMO. Hints only apply to the
                    // root, as that is where the page source is. There could equivalently be no
                    // owner if this is the zero page, which should also be ignored, and a page we
                    // borrowed is its lender's, whose state is not ours to change.
                    // SAFETY: The lock is held.
                    let owned =
                        !unsafe { page.object().get_object() }.is_null() && !self.is_borrowed(page);
                    if owned && self.page_source.is_some() && set_always_need {
                        // Nothing more to do beyond marking the page always_need true. The lookup
                        // must have already marked the page accessed, moving it to the head of the
                        // first page queue.
                        // SAFETY: The lock is held.
                        unsafe { page.object().set_always_need(true) };
                    }
                    range = range.trimmed_from_start(PAGE_SIZE);
                }
                status
            };
            match status {
                Ok(()) => {}
                Err(Status::SHOULD_WAIT) => match page_request.wait(true) {
                    // If the wait succeeded, the offset will now have a backing page, so we need
                    // to try the same offset again with a new cursor.
                    Ok(()) => continue,
                    Err(error) if !ignore_errors => return Err(error),
                    Err(_) => {
                        page_request.cancel_requests();
                        range = range.trimmed_from_start(PAGE_SIZE);
                    }
                },
                // Should only get here if an error was encountered, check if we should ignore or
                // return it.
                Err(error) if !ignore_errors => return Err(error),
                Err(_) => {
                    // Ignore the error, move to the next offset.
                    page_request.cancel_requests();
                    range = range.trimmed_from_start(PAGE_SIZE);
                }
            }
        }
        Ok(())
    }

    fn sharing(&self) -> Sharing {
        match self.sharing.load(Ordering::Acquire) {
            0 => Sharing::None,
            1 => Sharing::Lender,
            _ => Sharing::Borrower,
        }
    }

    /// Makes this object a lender or a borrower, which it then stays.
    fn begin_sharing(&self, sharing: Sharing) -> Result<(), Status> {
        debug_assert_ne!(sharing, Sharing::None);
        match self.sharing.compare_exchange(
            Sharing::None as u8,
            sharing as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(current) if current == sharing as u8 => Ok(()),
            Err(_) => Err(Status::BAD_STATE),
        }
    }

    /// Returns whether `page`, which is in our page list, is borrowed rather than ours.
    fn is_borrowed(&self, page: VmPagePtr) -> bool {
        // SAFETY: Our lock is held. A page in our list is either ours, whose backlink our lock
        // protects, or borrowed, whose backlink cannot change while we hold it, as its lender
        // takes our lock to take it back.
        !self.is_backlink(unsafe { page.object().get_object() })
    }

    /// Returns the lender of a page we borrowed, and the page's offset in the lender.
    fn lender_of(&self, page: VmPagePtr) -> (Arc<VmCowPages>, u64) {
        debug_assert!(self.is_borrowed(page));
        // SAFETY: As in `is_borrowed`. The backlink is a weak reference to the lender, borrowed
        // here and not consumed, and the lender is alive until it has taken back its loans, which
        // it does when it dies.
        unsafe {
            let object = page.object();
            let lender = ManuallyDrop::new(Weak::from_raw(object.get_object().cast::<Self>()));
            (lender.upgrade().expect("a lender outlives its loans"), object.get_page_offset())
        }
    }

    /// Gives back the page we borrowed at `offset`, which has been removed from our page list and
    /// unmapped. Its lender may then free it.
    fn return_borrowed_page(&self, offset: u64, page: VmPagePtr) {
        let (lender, lender_offset) = self.lender_of(page);
        lender.end_loan(lender_offset, self, offset);
    }

    /// Replaces the page we borrowed at `offset` with a copy of our own, from `alloc_list` if it
    /// has pages, unmapping the offset. The copy is Clean, as the borrowed page was.
    fn replace_borrowed_page_locked(
        &self,
        locked: &mut VmCowPagesLocked,
        offset: u64,
        alloc_list: Option<&mut VmPageDoublyLinkedList>,
        deferred: &mut DeferredOps,
        page_request: &mut AnonymousPageRequest,
    ) -> Result<(), Status> {
        let borrowed = locked.page_list.lookup(offset).expect("a borrowed page").page();
        debug_assert!(self.is_borrowed(borrowed));
        let copy = self.allocate_uninitialized_page(alloc_list, page_request)?;
        initialize_vm_page(copy);
        copy_page(copy, borrowed);
        self.update_dirty_state_locked(
            copy,
            offset,
            DirtyState::Clean,
            true,
            locked.high_priority_count,
        );
        let mut old = self.add_page_locked(
            locked,
            offset,
            VmPageOrMarker::from_page(copy),
            CanOverwriteSlot::PageOrRef,
            Some(deferred),
        )?;
        let page = old.release_page();
        debug_assert_eq!(page, borrowed);
        self.return_borrowed_page(offset, page);
        Ok(())
    }

    /// Removes the page at `offset` if it is still `page`, which its lender is taking back.
    fn take_back(self: &Arc<Self>, offset: u64, page: VmPagePtr) {
        let mut deferred = DeferredOps::new(self);
        let mut locked = self.lock();
        if !locked
            .page_list
            .lookup(offset)
            .is_some_and(|slot| slot.is_page() && slot.page() == page)
        {
            return;
        }
        self.range_change_update_locked(
            &mut locked,
            VmCowRange::new(offset, PAGE_SIZE),
            RangeChangeOp::Unmap,
            Some(&mut deferred),
        );
        // The page was supplied, so its absence again means its content must be supplied again.
        let released = locked.page_list.remove_content(offset).release_page();
        debug_assert_eq!(released, page);
    }

    /// Records that `borrower` holds our page at `offset` at `borrower_offset`. Our lock is held.
    fn begin_loan(&self, offset: u64, borrower: &Arc<VmCowPages>, borrower_offset: u64) {
        let mut loans = self.loans.lock();
        loans
            .by_offset
            .entry(offset)
            .or_default()
            .push(Loan { borrower: Arc::downgrade(borrower), offset: borrower_offset });
        self.loan_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Forgets that `borrower` holds our page at `offset` at `borrower_offset`, now that it gave it
    /// back, unless we already took it back.
    fn end_loan(&self, offset: u64, borrower: &VmCowPages, borrower_offset: u64) {
        let mut loans = self.loans.lock();
        let Some(list) = loans.by_offset.get_mut(&offset) else { return };
        let Some(index) = list.iter().position(|loan| {
            loan.offset == borrower_offset && std::ptr::eq(loan.borrower.as_ptr(), borrower)
        }) else {
            return;
        };
        list.swap_remove(index);
        if list.is_empty() {
            loans.by_offset.remove(&offset);
        }
        self.loan_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Takes back every loan of our `page` at `offset`, removing it from its borrowers, so that it
    /// can be dirtied, pinned or freed. Our lock is held, under which each borrower's is taken.
    fn revoke_loans(&self, offset: u64, page: VmPagePtr) {
        if self.loan_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        let loans = {
            let mut loans = self.loans.lock();
            let Some(list) = loans.by_offset.remove(&offset) else { return };
            self.loan_count.fetch_sub(list.len(), Ordering::Relaxed);
            list
        };
        debug_assert!(!self.is_borrowed(page));
        for loan in loans {
            // A borrower that is gone gave back its pages as it died.
            if let Some(borrower) = loan.borrower.upgrade() {
                borrower.take_back(loan.offset, page);
            }
        }
    }

    /// Lends our pages in `range` to `borrower` at `borrower_offset`, both backed by a user pager.
    /// As in `supply_pages_locked`, only the borrower's absent content is populated, and existing
    /// content is left unchanged. Every page in the range must be present, Clean and unpinned, or
    /// nothing is lent and `NOT_FOUND` returned.
    pub fn lend_pages(
        self: &Arc<Self>,
        range: VmCowRange,
        borrower: &Arc<VmCowPages>,
        borrower_offset: u64,
    ) -> Result<(), Status> {
        debug_assert!(range.is_page_aligned() && is_page_rounded(borrower_offset));
        ensure!(!Arc::ptr_eq(self, borrower), Status::INVALID_ARGS);
        if self.page_source_type() != PageSourceType::UserPager
            || borrower.page_source_type() != PageSourceType::UserPager
        {
            return Err(Status::NOT_SUPPORTED);
        }
        // Lock order is lender then borrower, which is why an object only lends or borrows. Check
        // both before making either one, so that a failed call changes neither.
        ensure!(
            self.sharing() != Sharing::Borrower && borrower.sharing() != Sharing::Lender,
            Status::BAD_STATE
        );
        self.begin_sharing(Sharing::Lender)?;
        borrower.begin_sharing(Sharing::Borrower)?;
        let _deferred = DeferredOps::new(self);
        let locked = self.lock();
        let mut borrower_deferred = DeferredOps::new(borrower);
        let mut borrower_locked = borrower.lock();
        ensure!(
            locked.life_cycle == LifeCycle::Alive && borrower_locked.life_cycle == LifeCycle::Alive,
            Status::BAD_STATE
        );
        let borrower_range = VmCowRange::new(borrower_offset, range.len);
        if !range.is_bounded_by(locked.size) || !borrower_range.is_bounded_by(borrower_locked.size)
        {
            return Err(Status::OUT_OF_RANGE);
        }
        let source = borrower.page_source.as_ref().expect("page source");
        ensure!(!source.is_detached(), Status::BAD_STATE);

        // Find the pages to lend, which must all be there.
        let mut pages = Vec::with_capacity((range.len / PAGE_SIZE) as usize);
        let status = locked.page_list.for_every_page_and_gap_in_range(
            range.offset,
            range.end(),
            |p, off| {
                if off != range.offset + pages.len() as u64 * PAGE_SIZE
                    || !p.is_page()
                    || !is_page_clean(p.page())
                    || pin_count(p.page()) != 0
                {
                    return Status::NOT_FOUND;
                }
                pages.push(p.page());
                Status::NEXT
            },
            |_, _| Status::NOT_FOUND,
        );
        status?;
        ensure!(pages.len() as u64 * PAGE_SIZE == range.len, Status::NOT_FOUND);

        // Add each page to the borrower where its content is absent, notifying its page source of
        // each run of pages added.
        let mut run_start = borrower_offset;
        let mut run_len = 0;
        let mut finish_run = |borrower_locked: &mut VmCowPagesLocked, run_start, run_len| {
            if run_len > 0 {
                borrower.range_change_update_locked(
                    borrower_locked,
                    VmCowRange::new(run_start, run_len),
                    RangeChangeOp::Unmap,
                    Some(&mut borrower_deferred),
                );
                source.on_pages_supplied(run_start, run_len);
            }
        };
        let mut result = Ok(());
        for (i, page) in pages.into_iter().enumerate() {
            let offset = range.offset + i as u64 * PAGE_SIZE;
            let dst_offset = borrower_offset + i as u64 * PAGE_SIZE;
            match borrower.begin_add_page_locked(
                &mut borrower_locked,
                dst_offset,
                CanOverwriteSlot::Empty,
            ) {
                Ok(transaction) => {
                    // SAFETY: The slot is valid until the transaction completes.
                    let old = VmPageOrMarkerRef::new(unsafe { &mut *transaction.slot })
                        .swap_content(VmPageOrMarker::from_page(page));
                    debug_assert!(old.is_empty());
                    self.begin_loan(offset, borrower, dst_offset);
                    page_queues().mark_accessed(page);
                    run_len += PAGE_SIZE;
                }
                Err(Status::ALREADY_EXISTS) => {
                    finish_run(&mut borrower_locked, run_start, run_len);
                    run_start = dst_offset + PAGE_SIZE;
                    run_len = 0;
                }
                Err(status) => {
                    result = Err(status);
                    break;
                }
            }
        }
        finish_run(&mut borrower_locked, run_start, run_len);
        result
    }

    /// Returns the number of committed pages, not counting pages borrowed from another object,
    /// which count as their lender's.
    pub fn committed_page_count_locked(&self, locked: &VmCowPagesLocked) -> u64 {
        let mut count = 0;
        let _ = locked.page_list.for_every_page(|p, _| {
            if p.is_page() && !self.is_borrowed(p.page()) {
                count += 1;
            }
            Status::NEXT
        });
        count
    }

    /// Returns the number of pages borrowed from other objects.
    pub fn borrowed_page_count_locked(&self, locked: &VmCowPagesLocked) -> u64 {
        let mut count = 0;
        let _ = locked.page_list.for_every_page(|p, _| {
            if p.is_page() && self.is_borrowed(p.page()) {
                count += 1;
            }
            Status::NEXT
        });
        count
    }

    pub fn reclamation_event_count_locked(&self, locked: &VmCowPagesLocked) -> u64 {
        locked.reclamation_event_count
    }

    /// Returns how many loans of our pages there are.
    pub fn debug_loan_count(&self) -> usize {
        self.loans.lock().by_offset.values().map(Vec::len).sum()
    }

    pub fn debug_get_page(&self, offset: u64) -> Option<VmPagePtr> {
        let locked = self.lock();
        locked.page_list.lookup(offset).filter(|s| s.is_page()).map(|s| s.page())
    }
}

impl Drop for VmCowPages {
    fn drop(&mut self) {
        let locked = self.lock.get_mut();
        debug_assert!(
            locked.page_list.has_no_page_ref_or_marker() || locked.life_cycle == LifeCycle::Init
        );
        debug_assert!(matches!(locked.life_cycle, LifeCycle::Init | LifeCycle::Dead));
        // A VmCowPages that never became alive may still have content. Any page source must still
        // be closed.
        if locked.life_cycle == LifeCycle::Init {
            if let Some(source) = &self.page_source {
                source.close();
            }
            let mut freed = VmPageDoublyLinkedList::new();
            locked.page_list.remove_all_content(|mut p| {
                if p.is_page() {
                    let page = p.release_page();
                    page_queues().remove(page);
                    freed.push_back(page);
                }
            });
            // SAFETY: The pages were owned by this object.
            unsafe { pmm::node().free_list(&mut freed, PmmOptDelayReuse::Default) };
        }
    }
}
