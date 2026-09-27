//! The per-page structure and lists of pages.
//!
//! Ported from `zircon/kernel/vm/include/vm/page.h` and
//! `zircon/kernel/vm/page.rs`.
//!
//! Every page of physical memory has a `VmPage` in the PMM's page array,
//! which lives for the life of the kernel. The fields are protected by
//! whichever lock owns the page at the time (the PMM lock while free, the
//! owning `VmCowPages` lock while in an object, the page queues lock for the
//! queue linkage), so they are accessed through `unsafe` accessors, as in
//! Zircon.
//!
//! Deviations:
//! - The union of per-state data only keeps the `object` arm. Loaned, zram,
//!   slab and MMU pages are not supported.
//! - `VmPageDoublyLinkedList` is a small intrusive list over `queue_node`
//!   rather than `fbl::DoublyLinkedList`.

use crate::kernel::types::PAddr;
use crate::vm::page_state::VmPageState;
use std::cell::UnsafeCell;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU8, Ordering};

pub mod object {
    pub const PIN_COUNT_BITS: u32 = 5;
    pub const MAX_PIN_COUNT: u32 = (1 << PIN_COUNT_BITS) - 1;
    pub const DIRTY_STATE_BITS: u32 = 2;
    pub const MAX_DIRTY_STATES: u32 = 1 << DIRTY_STATE_BITS;
    pub const DIRTY_STATES_MASK: u32 = MAX_DIRTY_STATES - 1;
}

/// Linkage for the one list a page can be in: a PMM free list, a page queue,
/// or a caller's list of allocated pages.
pub struct QueueNode {
    prev: *mut VmPage,
    next: *mut VmPage,
    in_container: bool,
}

impl QueueNode {
    const fn new() -> Self {
        Self { prev: ptr::null_mut(), next: ptr::null_mut(), in_container: false }
    }

    pub fn in_container(&self) -> bool {
        self.in_container
    }
}

/// State kept while a page is attached to a VM object.
pub struct VmPageObjectState {
    /// Back pointer to the `VmCowPages` containing this page. Valid while the
    /// page is in a `VmCowPages`, null otherwise. Modified only under the page
    /// queues lock while the page is in a `VmCowPages`.
    object: *mut (),
    /// The offset of this page in its `VmCowPages`.
    page_offset: u64,
    /// How many objects can access this page when it has no unique owner.
    pub share_count: u32,
    /// Which page queue this page is in.
    page_queue: AtomicU8,
    pin_count: u8,
    always_need: bool,
    dirty_state: u8,
}

impl VmPageObjectState {
    const fn new() -> Self {
        Self {
            object: ptr::null_mut(),
            page_offset: 0,
            share_count: 0,
            page_queue: AtomicU8::new(0),
            pin_count: 0,
            always_need: false,
            dirty_state: 0,
        }
    }

    pub fn get_object(&self) -> *mut () {
        self.object
    }

    pub fn set_object(&mut self, object: *mut ()) {
        self.object = object;
    }

    pub fn get_page_offset(&self) -> u64 {
        self.page_offset
    }

    pub fn set_page_offset(&mut self, page_offset: u64) {
        self.page_offset = page_offset;
    }

    pub fn get_page_queue_ref(&self) -> &AtomicU8 {
        &self.page_queue
    }

    pub fn pin_count(&self) -> u8 {
        self.pin_count
    }

    pub fn set_pin_count(&mut self, count: u8) {
        debug_assert!(u32::from(count) <= object::MAX_PIN_COUNT);
        self.pin_count = count;
    }

    pub fn always_need(&self) -> bool {
        self.always_need
    }

    pub fn set_always_need(&mut self, needed: bool) {
        self.always_need = needed;
    }

    pub fn dirty_state(&self) -> u8 {
        self.dirty_state
    }

    pub fn set_dirty_state(&mut self, state: u8) {
        debug_assert!(u32::from(state) < object::MAX_DIRTY_STATES);
        self.dirty_state = state;
    }
}

/// The core per-page structure.
pub struct VmPage {
    pub(crate) queue_node: UnsafeCell<QueueNode>,
    paddr: PAddr,
    object: UnsafeCell<VmPageObjectState>,
    state: AtomicU8,
}

// SAFETY: Access to the interior mutable fields is serialized by the locks
// described in the module documentation.
unsafe impl Sync for VmPage {}
unsafe impl Send for VmPage {}

impl VmPage {
    pub(crate) const fn new(paddr: PAddr) -> Self {
        Self {
            queue_node: UnsafeCell::new(QueueNode::new()),
            paddr,
            object: UnsafeCell::new(VmPageObjectState::new()),
            state: AtomicU8::new(VmPageState::Free as u8),
        }
    }

    pub fn paddr(&self) -> PAddr {
        self.paddr
    }

    pub fn state(&self) -> VmPageState {
        VmPageState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// # Safety
    ///
    /// The caller must own the page.
    pub unsafe fn set_state(&self, state: VmPageState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub fn is_free(&self) -> bool {
        self.state() == VmPageState::Free
    }

    /// Loaned pages are not supported, so no page is ever loaned.
    pub fn is_loaned(&self) -> bool {
        false
    }

    /// # Safety
    ///
    /// The caller must hold the lock protecting the page's object state.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn object(&self) -> &mut VmPageObjectState {
        // SAFETY: Guaranteed by the caller.
        unsafe { &mut *self.object.get() }
    }

    /// The page queue is read without locks, so it is always accessible.
    pub fn page_queue(&self) -> &AtomicU8 {
        // SAFETY: Only the atomic is referenced.
        unsafe { &(*self.object.get()).page_queue }
    }

    /// # Safety
    ///
    /// The caller must hold the lock protecting the page's list membership.
    pub unsafe fn in_container(&self) -> bool {
        // SAFETY: Guaranteed by the caller.
        unsafe { (*self.queue_node.get()).in_container }
    }
}

/// A pointer to a `VmPage` in the PMM's page array.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VmPagePtr(NonNull<VmPage>);

// SAFETY: Pages live for the life of the kernel; see `VmPage`.
unsafe impl Send for VmPagePtr {}
unsafe impl Sync for VmPagePtr {}

impl VmPagePtr {
    pub fn new(ptr: NonNull<VmPage>) -> Self {
        Self(ptr)
    }

    pub fn as_non_null(self) -> NonNull<VmPage> {
        self.0
    }

    pub fn as_raw(self) -> *mut VmPage {
        self.0.as_ptr()
    }

    /// Pages are never freed, so a reference is always valid.
    pub fn as_ref(&self) -> &'static VmPage {
        // SAFETY: The page array lives for the life of the kernel.
        unsafe { &*self.0.as_ptr() }
    }

    pub fn paddr(self) -> PAddr {
        self.as_ref().paddr()
    }

    pub fn state(self) -> VmPageState {
        self.as_ref().state()
    }
}

impl std::ops::Deref for VmPagePtr {
    type Target = VmPage;

    fn deref(&self) -> &VmPage {
        self.as_ref()
    }
}

/// An intrusive doubly linked list of pages, linked through `queue_node`.
///
/// A page can be in at most one list. The list does not own the pages; the
/// owner of the list is responsible for the pages in it, and must empty it
/// before dropping it.
pub struct VmPageDoublyLinkedList {
    head: *mut VmPage,
    tail: *mut VmPage,
}

// SAFETY: See `VmPage`.
unsafe impl Send for VmPageDoublyLinkedList {}
unsafe impl Sync for VmPageDoublyLinkedList {}

impl Default for VmPageDoublyLinkedList {
    fn default() -> Self {
        Self::new()
    }
}

impl VmPageDoublyLinkedList {
    pub const fn new() -> Self {
        Self { head: ptr::null_mut(), tail: ptr::null_mut() }
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    fn node(page: *mut VmPage) -> *mut QueueNode {
        // SAFETY: `page` points into the page array.
        unsafe { (*page).queue_node.get() }
    }

    pub fn front(&self) -> Option<VmPagePtr> {
        NonNull::new(self.head).map(VmPagePtr)
    }

    pub fn back(&self) -> Option<VmPagePtr> {
        NonNull::new(self.tail).map(VmPagePtr)
    }

    /// Returns the page after `page`, which must be in this list.
    pub fn next(&self, page: VmPagePtr) -> Option<VmPagePtr> {
        // SAFETY: `page` is in this list, which the caller has access to.
        NonNull::new(unsafe { (*Self::node(page.as_raw())).next }).map(VmPagePtr)
    }

    /// Returns the page before `page`, which must be in this list.
    pub fn prev(&self, page: VmPagePtr) -> Option<VmPagePtr> {
        // SAFETY: `page` is in this list, which the caller has access to.
        NonNull::new(unsafe { (*Self::node(page.as_raw())).prev }).map(VmPagePtr)
    }

    pub fn push_front(&mut self, page: VmPagePtr) {
        let page = page.as_raw();
        // SAFETY: The caller owns `page` and this list.
        unsafe {
            let node = &mut *Self::node(page);
            assert!(!node.in_container, "page already in a list");
            node.in_container = true;
            node.prev = ptr::null_mut();
            node.next = self.head;
            if self.head.is_null() {
                self.tail = page;
            } else {
                (*Self::node(self.head)).prev = page;
            }
            self.head = page;
        }
    }

    pub fn push_back(&mut self, page: VmPagePtr) {
        let page = page.as_raw();
        // SAFETY: The caller owns `page` and this list.
        unsafe {
            let node = &mut *Self::node(page);
            assert!(!node.in_container, "page already in a list");
            node.in_container = true;
            node.next = ptr::null_mut();
            node.prev = self.tail;
            if self.tail.is_null() {
                self.head = page;
            } else {
                (*Self::node(self.tail)).next = page;
            }
            self.tail = page;
        }
    }

    /// Removes `page`, which must be in this list.
    pub fn erase(&mut self, page: VmPagePtr) {
        let page = page.as_raw();
        // SAFETY: `page` is in this list, which the caller owns.
        unsafe {
            let node = &mut *Self::node(page);
            debug_assert!(node.in_container);
            if node.prev.is_null() {
                debug_assert_eq!(self.head, page);
                self.head = node.next;
            } else {
                (*Self::node(node.prev)).next = node.next;
            }
            if node.next.is_null() {
                debug_assert_eq!(self.tail, page);
                self.tail = node.prev;
            } else {
                (*Self::node(node.next)).prev = node.prev;
            }
            *node = QueueNode::new();
        }
    }

    pub fn pop_front(&mut self) -> Option<VmPagePtr> {
        let page = self.front()?;
        self.erase(page);
        Some(page)
    }

    pub fn pop_back(&mut self) -> Option<VmPagePtr> {
        let page = self.back()?;
        self.erase(page);
        Some(page)
    }

    /// Moves every page of `other` to the end of this list.
    pub fn splice(&mut self, other: &mut VmPageDoublyLinkedList) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            std::mem::swap(self, other);
            return;
        }
        // SAFETY: Both lists are owned by the caller.
        unsafe {
            (*Self::node(self.tail)).next = other.head;
            (*Self::node(other.head)).prev = self.tail;
        }
        self.tail = other.tail;
        *other = Self::new();
    }

    /// Moves every page of `other` to the front of this list.
    pub fn splice_front(&mut self, other: &mut VmPageDoublyLinkedList) {
        std::mem::swap(self, other);
        self.splice(other);
    }

    /// Moves the first `count` pages to a new list, which is returned.
    pub fn split_front(&mut self, count: usize) -> VmPageDoublyLinkedList {
        let mut front = Self::new();
        for _ in 0..count {
            let Some(page) = self.pop_front() else { break };
            front.push_back(page);
        }
        front
    }

    pub fn iter(&self) -> impl Iterator<Item = VmPagePtr> + '_ {
        std::iter::successors(self.front(), move |page| self.next(*page))
    }

    /// Counts the pages by walking the list.
    pub fn size_slow(&self) -> usize {
        self.iter().count()
    }
}

impl Drop for VmPageDoublyLinkedList {
    fn drop(&mut self) {
        debug_assert!(self.is_empty(), "dropped a list that still holds pages");
    }
}
