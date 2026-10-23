// Copyright 2018 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Requests for pages from a page source, such as a userspace pager.
//!
//! Ported from `zircon/kernel/vm/page_source.cc` and
//! `zircon/kernel/vm/include/vm/page_source.h`.
//!
//! At the high level the goal of the objects here is to
//! 1. Trigger external entities to do work based on VMO operations, such as asking a pager to
//!    supply a missing page of data.
//! 2. Have a way for external entities to let the VMO system know these requests have been
//!    fulfilled.
//! 3. Provide a way for the high level caller, who may not know what actions are being performed
//!    on what entities, to wait until their operation can be completed.
//!
//! The different objects can be summarized as:
//!  * PageRequest: Caller allocated object that the caller uses to perform the Wait.
//!  * PageSource: Performs request and overlap tracking, forwarding unique ranges of requests to
//!    the underlying PageProvider.
//!  * PageProvider: Asynchronously performs requests. Requests are completed by actions being
//!    performed on the VMO.
//!
//! A typical flow would be
//!  * User allocates PageRequest on the stack, and passes it in to some VMO operation
//!  * VMO code needs something to happen and calls a PageSource method, passing in PageRequest it
//!    had been given.
//!  * PageSource populates fields of the PageRequest and adds it to the list of requests it is
//!    tracking, and determines how this request overlaps with any others. Based on overlap, it may
//!    or may not notify the underlying PageProvider that some work needs to be done (the page
//!    provider will complete this asynchronously somehow).
//!  * VMO returns ZX_ERR_SHOULD_WAIT and then the top level calls PageRequest::Wait
//!  * PageRequest::Wait uses the PageSource to wait for the operation to complete
//!  # As an optional path, if the PageRequest was not Waited on for some reason, the PageRequest
//!    will also inform the PageSource that this request is no longer needed and can be canceled.
//! For the other side, while the Wait is happening some other thread will
//!  * Call a VMO operation, such as VmObject::SupplyPages
//!  * VMO will perform the operation, and then let the PageSource know by the corresponding
//!    interface method, such as OnPagesSupplied.
//!  * PageSource will update request tracking, and notify any PageRequests that were waiting and
//!    can be woken up.
//!
//! Deviations:
//! - A `PageRequest`'s state lives in a heap allocation, so that the page source and provider can
//!   refer to it by pointer while the owning `PageRequest` value moves. Its fields are protected by
//!   the page source lock, as in Zircon, and so are accessed through `unsafe`.
//! - The intrusive trees and lists of requests are a `BTreeMap` keyed by end offset and
//!   `VecDeque`s. Membership flags stand in for `InContainer` checks.
//! - `PageRequestInterface` is only implemented by `PageSource`, so requests refer to the source
//!   directly.
//! - No kcounters, ktrace or the `vm_page_source` console command.
//! - A request may be waited on by a future rather than a thread (`PageRequest::wait_async`, and
//!   `PageProvider::wait_on_event_async`; see `kernel::wakers`).
//! - fxdax: DIRTY requests alone can be failed (`on_dirty_failed`), as dirtying their pages failed
//!   for want of memory (TASK-79.75).

use crate::debug::{ltracef, ltracef_level};
use crate::kernel::event::{AutounsignalEvent, Event};
use crate::vm::anonymous_page_request::AnonymousPageRequest;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use parking_lot::{ArcMutexGuard, Mutex, RawMutex};
use std::cell::UnsafeCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::ops::Bound;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use zx_status::Status;

/// A future waiting on a page request's event, as `PageProvider::wait_on_event_async` returns.
pub type EventWaitFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Status>> + Send + 'a>>;

const LOCAL_TRACE: u32 = 0;

/// Identifies the VMO a request is for, for debugging.
#[derive(Clone, Copy, Debug, Default)]
pub struct VmoDebugInfo {
    pub vmo_id: u64,
    pub vmo_name: [u8; 8],
}

/// The different types of page requests that can exist.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRequestType {
    /// Request to provide the initial contents for the page.
    Read = 0,
    /// Request to alter contents of the page, i.e. transition it from clean to dirty.
    Dirty,
    /// Request to write back modified page contents back to the source.
    Writeback,
}

impl PageRequestType {
    /// Number of page request types.
    pub const COUNT: usize = 3;
    const ALL: [PageRequestType; Self::COUNT] = [Self::Read, Self::Dirty, Self::Writeback];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "READ",
            Self::Dirty => "DIRTY",
            Self::Writeback => "WRITEBACK",
        }
    }
}

/// These properties are constant per PageProvider type, so a given VmCowPages can query and cache
/// these properties once (if it has a PageSource) and know they won't change after that.
#[derive(Clone, Copy, Debug)]
pub struct PageSourceProperties {
    /// Whether the PageSource is really a user pager, as opposed to contiguous page reclaim.
    ///
    /// If this is true, the VmCowPages may include both immutable page content and page content
    /// that was potentially modified and written back previously.
    ///
    /// If this is false, the PageSource (and PageProvider) must be used to allocate all pages.
    pub is_user_pager: bool,
    /// For every entry, if true the PageSource supports the given request type.
    pub supports_request_type: [bool; PageRequestType::COUNT],
}

/// Interface for providing pages to a VMO through page requests.
pub trait PageProvider: Send + Sync {
    /// The returned properties can assumed to be const and never change. As such the caller may
    /// cache them.
    fn properties(&self) -> PageSourceProperties;

    /// Informs the backing source of a page request. The provider has ownership of `request`
    /// until the async request is cancelled.
    fn send_async_request(&self, request: PageRequestRef);

    /// Informs the backing source that a page request has been fulfilled. This must be called for
    /// all requests that are raised.
    fn clear_async_request(&self, request: PageRequestRef);

    /// Swaps the backing memory for a request. Assumes that `old` and `new_req` have the same
    /// type, offset, and length.
    fn swap_async_request(&self, old: PageRequestRef, new_req: PageRequestRef);

    /// Frees `pages`. Only providers that allocate pages themselves need implement this.
    fn free_pages(&self, _pages: &mut VmPageDoublyLinkedList) {
        panic!("this page provider does not free pages");
    }

    /// For asserting purposes only. Gives the provider a chance to check that a page is consistent
    /// with any rules it has about which pages can go where.
    fn debug_is_page_ok(&self, page: VmPagePtr, offset: u64) -> bool;

    /// Called once no more calls to `send_async_request` will be made. It will be called before
    /// `on_close` and will only be called once.
    fn on_detach(&self);

    /// After `on_close` is called, no more calls will be made except for `wait_on_event`.
    fn on_close(&self);

    /// Waits on an `event` associated with a page request. The waiting thread can return early
    /// from the wait due to a suspend signal only if `suspendable` is true.
    fn wait_on_event(&self, event: &Event, suspendable: bool) -> Result<(), Status>;

    /// Waits on an `event` associated with a page request, as `wait_on_event` does, as a future
    /// rather than by blocking the calling thread.
    fn wait_on_event_async<'a>(
        &'a self,
        event: &'a Event,
        suspendable: bool,
    ) -> EventWaitFuture<'a>;

    /// Dumps relevant state for debugging purposes.
    fn dump(&self, depth: u32, max_items: u32);

    /// Retrieves the koid for this page provider, if it has one.
    fn get_koid(&self) -> Option<u64> {
        None
    }
}

/// The state of a page request, shared between its owner, the page source and the provider.
struct PageRequestState {
    /// Set on construction if the user of the PageRequest supports, and wants to be, woken early.
    early_wake: bool,
    /// Event signaled when the request is fulfilled.
    event: AutounsignalEvent,
    /// The page source this request is currently associated with. Only modified by `init`, which
    /// only the owner calls, so the owner may read it without the source's lock.
    src: UnsafeCell<Option<Arc<PageSource>>>,
    /// Whether the request is in its provider's list of requests. Protected by the provider.
    provider_link: UnsafeCell<bool>,
    /// Everything else, protected by `src`'s lock.
    fields: UnsafeCell<PageRequestFields>,
}

struct PageRequestFields {
    /// The type of the page request.
    type_: PageRequestType,
    /// Whether a PageProvider owns the request (SendAsyncRequest has been called). While owned,
    /// type_, offset_ and len_ must be kept constant so the PageProvider can read them.
    provider_owned: bool,
    /// The offset into the request at which the event should next be signaled. This is request
    /// relative, so a value of 0 indicates that it should be signaled when the page at offset is
    /// provided. After being triggered, the wake_offset increments by the amount provided so that
    /// it can potentially get triggered again.
    wake_offset: u64,
    /// Tracks any error that we will send to the waiter when the request is completed. This allows
    /// for partial failure of a request, where we report the status of the first page in the
    /// request so that any partially provided pages can be processed.
    complete_status: Result<(), Status>,
    /// PageRequests are active if offset is not u64::MAX.
    offset: u64,
    /// The total length of the request.
    len: u64,
    /// The vmobject this page request is for.
    vmo_debug_info: VmoDebugInfo,
    /// Keeps track of the size of the request that still needs to be fulfilled. This can become
    /// incorrect if some pages get supplied, decommitted, and then re-supplied. If that happens,
    /// then it will cause the page request to complete prematurely. However, page source clients
    /// should be operating in a loop to handle evictions, so this will simply result in some
    /// redundant read requests to the page source.
    pending_size: u64,
    /// Requests overlapping this one, which complete when it does.
    overlap: VecDeque<PageRequestRef>,
    /// Whether this request is in another request's `overlap` list.
    in_overlap: bool,
    /// Whether this request is in its source's outstanding requests.
    in_tree: bool,
}

/// A reference to a page request, by which page sources and providers track it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageRequestRef(NonNull<PageRequestState>);

// SAFETY: The state is protected by the locks documented on `PageRequestState`.
unsafe impl Send for PageRequestRef {}
unsafe impl Sync for PageRequestRef {}

impl PageRequestRef {
    fn state(&self) -> &PageRequestState {
        // SAFETY: A request is only referenced while its owner keeps it alive, since dropping the
        // owner cancels the request, removing every reference.
        unsafe { self.0.as_ref() }
    }

    /// # Safety
    ///
    /// The caller must hold the lock of the request's page source.
    #[allow(clippy::mut_from_ref)]
    unsafe fn fields(&self) -> &mut PageRequestFields {
        // SAFETY: Guaranteed by the caller.
        unsafe { &mut *self.state().fields.get() }
    }

    fn event(&self) -> &Event {
        self.state().event.as_event()
    }

    /// The following may be used by a provider on a request it owns, whose fields are then
    /// constant.
    pub fn request_type(&self) -> PageRequestType {
        // SAFETY: The fields are constant while the provider owns the request.
        let fields = unsafe { self.fields() };
        debug_assert!(fields.provider_owned);
        fields.type_
    }

    pub fn offset(&self) -> u64 {
        // SAFETY: As above.
        let fields = unsafe { self.fields() };
        debug_assert!(fields.provider_owned);
        fields.offset
    }

    #[allow(clippy::len_without_is_empty, reason = "a request is never empty")]
    pub fn len(&self) -> u64 {
        // SAFETY: As above.
        let fields = unsafe { self.fields() };
        debug_assert!(fields.provider_owned);
        fields.len
    }

    pub fn vmo_id(&self) -> u64 {
        // SAFETY: As above.
        let fields = unsafe { self.fields() };
        debug_assert!(fields.provider_owned);
        fields.vmo_debug_info.vmo_id
    }

    /// Whether the request is in its provider's list of requests.
    ///
    /// # Safety
    ///
    /// The caller must be the provider, holding the lock protecting its list.
    pub unsafe fn in_provider_list(&self) -> bool {
        // SAFETY: Guaranteed by the caller.
        unsafe { *self.state().provider_link.get() }
    }

    /// # Safety
    ///
    /// As for `in_provider_list`.
    pub unsafe fn set_in_provider_list(&self, value: bool) {
        // SAFETY: Guaranteed by the caller.
        unsafe { *self.state().provider_link.get() = value };
    }
}

impl PageRequestFields {
    fn is_initialized(&self) -> bool {
        self.offset != u64::MAX
    }

    fn get_end(&self) -> u64 {
        // Assert on overflow, since it means vmobject made an out-of-bounds request.
        self.offset.checked_add(self.len).expect("page request range overflows")
    }

    fn range_overlaps(&self, start: u64, end: u64) -> bool {
        end > self.offset && start < self.get_end()
    }

    /// Converts a [start,end) range in provider (aka VMO) space to the sub range that overlaps
    /// with this request and returns it relative to this requests offset.
    fn trim_range_to_request_space(&self, start: u64, end: u64) -> (u64, u64) {
        // The operation started partway into this request, or before it.
        let req_offset = start.saturating_sub(self.offset);
        let req_end = if end < self.get_end() {
            // The operation ended partway into this request.
            end.checked_sub(self.offset).expect("range ends before request")
        } else {
            // The operation ended past the end of this request.
            self.len
        };
        debug_assert!(req_end >= req_offset);
        (req_offset, req_end)
    }
}

/// Object which is used to make delayed page requests to a PageSource.
pub struct PageRequest {
    state: Box<PageRequestState>,
}

// SAFETY: See `PageRequestState`.
unsafe impl Send for PageRequest {}

impl Default for PageRequest {
    fn default() -> Self {
        Self::new(false)
    }
}

impl PageRequest {
    /// If `early_wake` is true then the caller is asking to be woken up once some of the request
    /// is satisfied, potentially before all of it is satisfied. This is intended to allow users to
    /// process partial amounts of data as they come in before continuing to Wait for the rest with
    /// only a single PageRequest sent to the PageSource.
    pub fn new(early_wake: bool) -> Self {
        Self {
            state: Box::new(PageRequestState {
                early_wake,
                event: AutounsignalEvent::new(false),
                src: UnsafeCell::new(None),
                provider_link: UnsafeCell::new(false),
                fields: UnsafeCell::new(PageRequestFields {
                    type_: PageRequestType::Read,
                    provider_owned: false,
                    wake_offset: u64::MAX,
                    complete_status: Ok(()),
                    offset: u64::MAX,
                    len: 0,
                    vmo_debug_info: VmoDebugInfo::default(),
                    pending_size: 0,
                    overlap: VecDeque::new(),
                    in_overlap: false,
                    in_tree: false,
                }),
            }),
        }
    }

    /// Returns the reference by which page sources and providers know this request.
    pub fn as_ref(&self) -> PageRequestRef {
        PageRequestRef(NonNull::from(&*self.state))
    }

    fn src(&self) -> Option<&Arc<PageSource>> {
        // SAFETY: `src` is only modified by `init`, which only the owner calls.
        unsafe { (*self.state.src.get()).as_ref() }
    }

    /// Returns ZX_OK on success, or a permitted error code if the backing page provider explicitly
    /// failed this page request. Returns ZX_ERR_INTERNAL_INTR_RETRY if `suspendable` is true and
    /// the thread was suspended; the thread cannot be suspended in the wait if `suspendable` is
    /// false. If this page request is allowed to early wake then this can return success with the
    /// request still active and queued with a PageSource. In this case it is invalid to attempt to
    /// use this request with any other PageSource or for any other range without first doing
    /// CancelRequest.
    pub fn wait(&mut self, suspendable: bool) -> Result<(), Status> {
        let src = self.src().expect("waiting on an uninitialized request").clone();
        let status = src.wait_on_request(self.as_ref(), suspendable);
        self.waited(&src, status)
    }

    /// Waits for the request, as `wait` does, as a future rather than by blocking the calling
    /// thread. Dropping the future before it completes leaves the request queued, as an early
    /// woken request is, until it is canceled or the request is dropped.
    pub async fn wait_async(&mut self, suspendable: bool) -> Result<(), Status> {
        let src = self.src().expect("waiting on an uninitialized request").clone();
        let status = src.wait_on_request_async(self.state.event.as_event(), suspendable).await;
        self.waited(&src, status)
    }

    /// Cancels the request after a wait on `src` that failed other than as the provider failed it.
    fn waited(&mut self, src: &PageSource, status: Result<(), Status>) -> Result<(), Status> {
        if let Err(error) = status
            && !PageSource::is_valid_internal_failure_code(error)
        {
            src.cancel_request(self.as_ref());
        }
        status
    }

    /// Asks the underlying PageSource to abort this request.
    pub fn cancel_request(&mut self) {
        // Nothing to cancel if the request isn't initialized yet. We may inspect src here as src
        // is only set by Init, and Init and CancelRequest must only be called by the owner of the
        // PageRequest, providing implicit serialization.
        if let Some(src) = self.src().cloned() {
            src.cancel_request(self.as_ref());
        }
    }

    /// PageRequests are initialized separately to being constructed to facilitate any PageSource
    /// specific logic. This method makes three assumptions on how it is called:
    ///  1. If previously initialized it has been separately uninitialized via `CancelRequest` or
    ///     similar.
    ///  2. It is invoked under the src lock.
    ///  3. It is called on the thread that owns the PageRequest and is not thread safe with
    ///     parallel invocations of CancelRequest.
    fn init(
        request: PageRequestRef,
        src: Arc<PageSource>,
        offset: u64,
        type_: PageRequestType,
        vmo_debug_info: VmoDebugInfo,
    ) {
        // SAFETY: Invoked under the src lock, by the owner.
        let fields = unsafe { request.fields() };
        debug_assert!(!fields.is_initialized());
        fields.vmo_debug_info = vmo_debug_info;
        fields.len = 0;
        fields.offset = offset;
        if request.state().early_wake {
            fields.wake_offset = 0;
        }
        fields.type_ = type_;
        // SAFETY: Only the owner calls `init`.
        unsafe { *request.state().src.get() = Some(src) };
        fields.complete_status = Ok(());
        request.event().unsignal();
    }
}

impl Drop for PageRequest {
    fn drop(&mut self) {
        self.cancel_request();
    }
}

/// Wrapper around PageRequest that performs construction on first access. This is useful when a
/// PageRequest needs to be allocated eagerly in case it is used, even if the common case is that
/// it will not be needed.
#[derive(Default)]
pub struct LazyPageRequest {
    /// Early wake parameter to be passed onto the PageRequest constructor.
    early_wake: bool,
    request: Option<PageRequest>,
}

impl LazyPageRequest {
    /// Construct a page request that optionally supports early waking. See PageRequest::new.
    pub fn new(early_wake: bool) -> Self {
        Self { early_wake, request: None }
    }

    /// Initialize and return the internal PageRequest.
    pub fn get(&mut self) -> &mut PageRequest {
        let early_wake = self.early_wake;
        self.request.get_or_insert_with(|| PageRequest::new(early_wake))
    }

    pub fn is_initialized(&self) -> bool {
        self.request.is_some()
    }
}

/// Wrapper around tracking multiple different page requests that might need waiting. Only one
/// individual request is allowed to considered 'active' at a time as the one that next needs
/// waiting on. Tracking whether a request is active is, depending on the request type, partially
/// automatic and partially requiring additional input from the user.
#[derive(Default)]
pub struct MultiPageRequest {
    read_active: bool,
    dirty_active: bool,
    anonymous: AnonymousPageRequest,
    page_request: LazyPageRequest,
}

impl MultiPageRequest {
    pub fn new(early_wake: bool) -> Self {
        Self { page_request: LazyPageRequest::new(early_wake), ..Default::default() }
    }

    /// Wait on the currently active page request.
    pub fn wait(&mut self, suspendable: bool) -> Result<(), Status> {
        if self.take_pager_request() {
            self.page_request.get().wait(suspendable)
        } else {
            self.anonymous.allocate()
        }
    }

    /// Waits on the currently active page request, as `wait` does, as a future rather than by
    /// blocking the calling thread.
    pub async fn wait_async(&mut self, suspendable: bool) -> Result<(), Status> {
        if self.take_pager_request() {
            self.page_request.get().wait_async(suspendable).await
        } else {
            self.anonymous.allocate_async().await
        }
    }

    /// Whether the active request is the page request, rather than the anonymous one, which is
    /// then no longer active.
    fn take_pager_request(&mut self) -> bool {
        if self.anonymous.is_active() {
            debug_assert!(!self.dirty_active && !self.read_active);
            return false;
        }
        // Exactly one of read and dirty should be considered active.
        debug_assert!(self.dirty_active ^ self.read_active);
        self.read_active = false;
        self.dirty_active = false;
        true
    }

    /// Retrieve the anonymous page request. The caller may or may not arm the
    /// AnonymousPageRequest, if it does the anonymous request becomes considered active and no
    /// other request may be retrieved.
    pub fn get_anonymous(&mut self) -> &mut AnonymousPageRequest {
        debug_assert!(self.no_request_active());
        &mut self.anonymous
    }

    /// Retrieve and commit to initializing the page request for read. After calling this it is
    /// assumed that the page request will be made waitable and no other request may be retrieved.
    pub fn get_read_request(&mut self) -> &mut PageRequest {
        debug_assert!(self.no_request_active());
        self.read_active = true;
        self.page_request.get()
    }

    /// Retrieve a lazy accessor to page request. If a dirty request is generated the caller must
    /// then call made_dirty_request so that this helper knows that the page request is active and
    /// should be waited on.
    pub fn get_lazy_dirty_request(&mut self) -> &mut LazyPageRequest {
        debug_assert!(self.no_request_active());
        &mut self.page_request
    }

    /// Indicate that the page request retrieved by get_lazy_dirty_request was used and should be
    /// waited on.
    pub fn made_dirty_request(&mut self) {
        debug_assert!(self.no_request_active());
        self.dirty_active = true;
    }

    /// Cancel all requests and have no active request.
    pub fn cancel_requests(&mut self) {
        self.anonymous.cancel();
        // In case a request is still initialized, despite being waited on, explicitly cancel
        // regardless of active status.
        if self.page_request.is_initialized() {
            self.page_request.get().cancel_request();
        }
        self.read_active = false;
        self.dirty_active = false;
    }

    fn no_request_active(&self) -> bool {
        !self.anonymous.is_active() && !self.read_active && !self.dirty_active
    }
}

struct PageSourceLocked {
    detached: bool,
    closed: bool,
    /// Trees of outstanding requests which have been sent to the PageProvider, one for each
    /// supported page request type. These lists are keyed by the end offset of the requests (not
    /// the start offsets).
    outstanding_requests: [BTreeMap<u64, PageRequestRef>; PageRequestType::COUNT],
}

impl PageSourceLocked {
    /// Returns the outstanding request of `type_` with the smallest end greater than `offset`.
    fn upper_bound(&self, type_: PageRequestType, offset: u64) -> Option<PageRequestRef> {
        self.outstanding_requests[type_ as usize]
            .range((Bound::Excluded(offset), Bound::Unbounded))
            .next()
            .map(|(_, request)| *request)
    }

    fn insert(&mut self, request: PageRequestRef) {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        debug_assert!(!fields.in_tree);
        fields.in_tree = true;
        let previous =
            self.outstanding_requests[fields.type_ as usize].insert(fields.get_end(), request);
        debug_assert!(previous.is_none());
    }

    fn erase(&mut self, request: PageRequestRef) {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        debug_assert!(fields.in_tree);
        fields.in_tree = false;
        let removed = self.outstanding_requests[fields.type_ as usize].remove(&fields.get_end());
        debug_assert_eq!(removed, Some(request));
    }
}

/// How a new request range relates to an existing request.
enum ContinuationType {
    NotContinuation,
    SameRequest,
    SameSource,
}

/// Object which provides pages to a vm_object.
///
/// A page source is responsible for fulfilling page requests from a VMO with backing pages. The
/// PageSource class mostly contains generic functionality around managing the lifecycle of VMO
/// page requests. The PageSource contains a reference to a PageProvider implementation, which is
/// responsible for actually providing the pages. (E.g. for VMOs backed by a userspace pager, the
/// PageProvider is a PagerProxy instance which talks to the userspace pager service.)
///
/// For asynchronous requests, the lifecycle is as follows:
///   1) A vm object requests a page with PageSource::GetPages.
///   2) PageSource starts tracking the request's PageRequest and then forwards the request to
///      PageProvider::SendAsyncRequest.
///   3) The caller waits for the request with PageRequest::Wait.
///   4) At some point, whatever is backing the PageProvider provides pages to the vm object (e.g.
///      with VmObjectPaged::SupplyPages).
///   5) The vm object calls PageSource::OnPagesSupplied, which signals any PageRequests that have
///      been fulfilled.
///   6) The caller wakes up and queries the vm object again, by which point the requested page
///      will be present.
pub struct PageSource {
    /// Lock used by the VMO to perform synchronization across its hierarchy. This lock does not
    /// strictly belong here, but this is a convenient and efficient place to put it.
    paged_vmo_mutex: Arc<Mutex<()>>,
    lock: Mutex<PageSourceLocked>,
    /// We cache the immutable page_provider.properties() to avoid many virtual calls.
    page_provider_properties: PageSourceProperties,
    /// PageProvider instance that will provide pages asynchronously (e.g. a userspace pager, see
    /// PagerProxy for details).
    page_provider: Arc<dyn PageProvider>,
}

impl PageSource {
    pub fn new(page_provider: Arc<dyn PageProvider>) -> Arc<Self> {
        let source = Arc::new(Self {
            paged_vmo_mutex: Arc::new(Mutex::new(())),
            lock: Mutex::new(PageSourceLocked {
                detached: false,
                closed: false,
                outstanding_requests: Default::default(),
            }),
            page_provider_properties: page_provider.properties(),
            page_provider,
        });
        ltracef!("{:p}\n", Arc::as_ptr(&source));
        source
    }

    /// Sends a request to the backing source to provide the requested page at `offset`.
    ///
    /// Returns ZX_ERR_NOT_FOUND if the request cannot be fulfilled.
    /// Returns ZX_ERR_SHOULD_WAIT if the request will be asynchronously fulfilled and the caller
    /// should wait on `req`.
    pub fn get_pages(
        self: &Arc<Self>,
        offset: u64,
        len: u64,
        req: &mut PageRequest,
        vmo_debug_info: VmoDebugInfo,
    ) -> Status {
        self.populate_request(req.as_ref(), offset, len, vmo_debug_info, PageRequestType::Read)
    }

    /// Frees `pages`.
    pub fn free_pages(&self, pages: &mut VmPageDoublyLinkedList) {
        self.page_provider.free_pages(pages);
    }

    /// For asserting purposes only.
    pub fn debug_is_page_ok(&self, page: VmPagePtr, offset: u64) -> bool {
        self.page_provider.debug_is_page_ok(page, offset)
    }

    /// Updates the request tracking metadata to account for pages [offset, offset + len) having
    /// been supplied to the owning vmo.
    ///
    /// Note that the range [offset, offset + len) should not have been previously supplied. The
    /// page request tracking in PageSource works by tracking only a fulfilled length, and not exact
    /// fulfilled offsets, to save on memory required for metadata. So in order to prevent
    /// over-accounting errors, the caller must ensure that they are only calling OnPagesSupplied for
    /// newly supplied ranges.
    pub fn on_pages_supplied(&self, offset: u64, len: u64) {
        let mut locked = self.lock.lock();
        self.resolve_requests_locked(&mut locked, PageRequestType::Read, offset, len, Ok(()));
    }

    /// Fails outstanding page requests in the range [offset, offset + len). Events associated with
    /// the failed page requests are signaled with the `error_status`, and any waiting threads are
    /// unblocked.
    pub fn on_pages_failed(&self, offset: u64, len: u64, error_status: Status) {
        ltracef_level!(2, "{:p} offset {:x}, len {:x}\n", self, offset, len);
        debug_assert!(Self::is_valid_internal_failure_code(error_status));
        // vmobject should have already validated overflow
        debug_assert!(offset.checked_add(len).is_some());

        let mut locked = self.lock.lock();
        if locked.detached {
            return;
        }
        for type_ in PageRequestType::ALL {
            if !self.supports_page_request_type(type_) {
                continue;
            }
            self.resolve_requests_locked(&mut locked, type_, offset, len, Err(error_status));
        }
    }

    /// Returns true if `error_status` is a valid ZX_PAGER_OP_FAIL failure error code (input,
    /// specified by user mode pager). These codes can be used with `on_pages_failed` (and so can
    /// any failure codes for which is_valid_internal_failure_code() returns true).
    pub fn is_valid_external_failure_code(error_status: Status) -> bool {
        matches!(
            error_status,
            Status::IO
                | Status::IO_DATA_INTEGRITY
                | Status::BAD_STATE
                | Status::NO_SPACE
                | Status::BUFFER_TOO_SMALL
        )
    }

    /// Returns true if `error_status` is a valid provider failure error code, which can be used
    /// with `on_pages_failed`. This returns true for every error code that
    /// is_valid_external_failure_code() returns true for, plus ZX_ERR_NO_MEMORY.
    pub fn is_valid_internal_failure_code(error_status: Status) -> bool {
        error_status == Status::NO_MEMORY || Self::is_valid_external_failure_code(error_status)
    }

    pub fn supports_page_request_type(&self, type_: PageRequestType) -> bool {
        self.properties().supports_request_type[type_ as usize]
    }

    /// Whether transitions from clean to dirty should be trapped.
    pub fn should_trap_dirty_transitions(&self) -> bool {
        self.supports_page_request_type(PageRequestType::Dirty)
    }

    /// Request the page provider for clean pages in the range [offset, offset + len) to become
    /// dirty, in order for a write to proceed. Returns ZX_ERR_SHOULD_WAIT if the request will be
    /// asynchronously fulfilled; the caller should wait on `request`. Depending on the state of
    /// pages in the range, the `request` might be generated for a range that is a subset of
    /// [offset, offset + len).
    pub fn request_dirty_transition(
        self: &Arc<Self>,
        request: &mut PageRequest,
        offset: u64,
        len: u64,
        vmo_debug_info: VmoDebugInfo,
    ) -> Status {
        self.populate_request(request.as_ref(), offset, len, vmo_debug_info, PageRequestType::Dirty)
    }

    /// Updates the request tracking metadata to account for pages [offset, offset + len) having
    /// been dirtied in the owning VMO. As with on_pages_supplied, the range should not have been
    /// previously dirtied.
    pub fn on_pages_dirtied(&self, offset: u64, len: u64) {
        let mut locked = self.lock.lock();
        self.resolve_requests_locked(&mut locked, PageRequestType::Dirty, offset, len, Ok(()));
    }

    /// fxdax: fails the outstanding DIRTY requests in [offset, offset + len) with `error_status`,
    /// as their pages could not be dirtied (TASK-79.75). Unlike `on_pages_failed`, it leaves the
    /// requests of other types, whose failure it is not. As with on_pages_dirtied, the range should
    /// not have been previously dirtied.
    pub fn on_dirty_failed(&self, offset: u64, len: u64, error_status: Status) {
        debug_assert!(Self::is_valid_internal_failure_code(error_status));
        let mut locked = self.lock.lock();
        let failed = Err(error_status);
        self.resolve_requests_locked(&mut locked, PageRequestType::Dirty, offset, len, failed);
    }

    /// Detaches the source from the VMO. All future calls into the page source will fail. All
    /// pending read transactions are aborted. Pending flush transactions will still be serviced.
    pub fn detach(&self) {
        ltracef!("{:p}\n", self);
        let mut locked = self.lock.lock();
        if locked.detached {
            return;
        }
        locked.detached = true;

        // Cancel all requests except writebacks, which can be completed after detach.
        for type_ in PageRequestType::ALL {
            if type_ == PageRequestType::Writeback || !self.supports_page_request_type(type_) {
                continue;
            }
            while let Some((_, req)) = locked.outstanding_requests[type_ as usize].pop_first() {
                // SAFETY: The lock is held.
                let fields = unsafe { req.fields() };
                fields.in_tree = false;
                ltracef!("dropping request with offset {:x} len {:x}\n", fields.offset, fields.len);
                // Tell the clients the request is complete - they'll fail when they reattempt the
                // page request for the same pages after failing this time.
                self.complete_request_locked(req);
            }
        }

        // No writebacks supported yet.
        debug_assert!(locked.outstanding_requests[PageRequestType::Writeback as usize].is_empty());

        self.page_provider.on_detach();
    }

    /// Closes the source. Will call Detach() if the source is not already detached. All pending
    /// transactions will be aborted and all future calls will fail.
    pub fn close(&self) {
        ltracef!("{:p}\n", self);
        // This will be a no-op if the page source has already been detached.
        self.detach();

        let mut locked = self.lock.lock();
        if locked.closed {
            return;
        }
        locked.closed = true;
        self.page_provider.on_close();
    }

    /// The returned properties can assumed to be const and never change. As such the caller may
    /// cache them.
    pub fn properties(&self) -> PageSourceProperties {
        self.page_provider_properties
    }

    /// Prints state of the page source and any pending requests.
    pub fn dump(&self, depth: u32, max_items: u32) {
        self.dump_self(depth, max_items);
        self.page_provider.dump(depth, max_items);
    }

    /// Similar to Dump, but only dumps information about this exact object, and will not forward
    /// the Dump request to the related PageProvider.
    pub fn dump_self(&self, depth: u32, max_items: u32) {
        let locked = self.lock.lock();
        let indent = " ".repeat(depth as usize * 2);
        eprintln!(
            "{indent}page_source {:p} detached {} closed {}",
            self, locked.detached, locked.closed
        );
        for type_ in PageRequestType::ALL {
            for req in locked.outstanding_requests[type_ as usize].values().take(max_items as usize)
            {
                // SAFETY: The lock is held.
                let fields = unsafe { req.fields() };
                eprintln!(
                    "{indent}  vmo k{} {} req [{:#x}, {:#x}) pending {:#x} overlap {} {}",
                    fields.vmo_debug_info.vmo_id,
                    type_.as_str(),
                    fields.offset,
                    fields.get_end(),
                    fields.pending_size,
                    fields.overlap.len(),
                    if fields.provider_owned { "[sent]" } else { "" }
                );
            }
        }
    }

    pub fn is_detached(&self) -> bool {
        self.lock.lock().detached
    }

    /// Method for the VmCowPages to retrieve the lock for paged VMOs. The guard keeps the lock
    /// alive, as the DeferredOps holding it may outlive the page source.
    pub fn paged_vmo_lock(&self) -> ArcMutexGuard<RawMutex, ()> {
        self.paged_vmo_mutex.lock_arc()
    }

    /// Retrieves any koid for the linked page provider.
    pub fn get_provider_koid(&self) -> Option<u64> {
        self.page_provider.get_koid()
    }

    /// Helper to perform early waking on a request and any overlapping requests. The provided
    /// range should be in local request space, and this method is only valid to be called if
    /// `request.wake_offset == req_start`.
    fn early_wake_request_locked(&self, request: PageRequestRef, req_start: u64, req_end: u64) {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        debug_assert!(req_end >= req_start);
        debug_assert_eq!(req_start, fields.wake_offset);
        // By default set the wake_offset to the end of the supplied range so the caller can just
        // wait again. In practice they are likely to call GetPages first, which will reinitialize
        // wake_offset to what is actually the next desired offset, which may or may not be the
        // same as what is set here.
        fields.wake_offset = req_end;
        request.event().signal_result(fields.complete_status);
        // For simplicity convert the request relative range back into a provider (aka VMO) range.
        let provider_start = req_start + fields.offset;
        let provider_end = req_end + fields.offset;
        for overlap_ref in &fields.overlap {
            // SAFETY: The lock is held.
            let overlap = unsafe { overlap_ref.fields() };
            if !overlap_ref.state().early_wake {
                continue;
            }
            // If the parent range has processed beyond the overlap wake offset then this is most
            // likely a sign of pages having not been linearly supplied, so want to wake up this
            // request anyway.
            if provider_start > overlap.offset
                && provider_start - overlap.offset > overlap.wake_offset
            {
                // In the case that something unusual has happened we do not want to keep on waking
                // up the request for any future completions, since it is waiting for wake_offset
                // and either it already got supplied and we missed it, which will get fixed by
                // doing this signal, or it's not been supplied and this (and any future) signal is
                // a waste. To prevent future wakes we set the wake_offset to a large so that we do
                // not continuously signal as the parent request is given more content. The largest
                // obvious safe value, i.e. would not overflow anywhere, is the end of the parent
                // request range.
                overlap.wake_offset = fields.len;
                overlap_ref.event().signal_result(overlap.complete_status);
                continue;
            }
            // If there is otherwise no overlap, then can skip.
            if !overlap.range_overlaps(provider_start, provider_end) {
                continue;
            }
            // Get the overlapping portion and see if it intersects with the wake_offset.
            let (overlap_start, overlap_end) =
                overlap.trim_range_to_request_space(provider_start, provider_end);
            if overlap_start <= overlap.wake_offset && overlap_end > overlap.wake_offset {
                overlap.wake_offset = overlap_end;
                overlap_ref.event().signal_result(overlap.complete_status);
            }
        }
    }

    /// Helper that updates request tracking metadata to resolve requests of `type_` in the range
    /// [offset, offset + len).
    fn resolve_requests_locked(
        &self,
        locked: &mut PageSourceLocked,
        type_: PageRequestType,
        offset: u64,
        len: u64,
        error_status: Result<(), Status>,
    ) {
        ltracef_level!(2, "{:p} offset {:x}, len {:x}\n", self, offset, len);
        // vmobject should have already validated overflow
        let end = offset.checked_add(len).expect("range overflows");

        if locked.detached {
            return;
        }

        // The first possible request we could fulfill is the one with the smallest end address
        // that is greater than offset. Then keep looking as long as the target request's start
        // offset is less than the end.
        let mut next = locked.upper_bound(type_, offset);
        while let Some(cur) = next {
            // SAFETY: The lock is held.
            let fields = unsafe { cur.fields() };
            if fields.offset >= end {
                break;
            }
            next = locked.upper_bound(type_, fields.get_end());

            // Because of upper_bound and our loop condition we know the range partially overlaps
            // this request.
            debug_assert!(fields.range_overlaps(offset, end));

            // Calculate how many pages were resolved in this request by finding the start and end
            // offsets of the operation in this request.
            let (req_offset, req_end) = fields.trim_range_to_request_space(offset, end);

            if error_status.is_err() {
                if req_offset == 0 || req_offset == fields.wake_offset {
                    fields.complete_status = error_status;
                }
                for overlap_ref in &fields.overlap {
                    // SAFETY: The lock is held.
                    let overlap = unsafe { overlap_ref.fields() };
                    // If there is otherwise no overlap, then can skip.
                    if !overlap.range_overlaps(offset, end) {
                        continue;
                    }
                    let (overlap_start, _) = overlap.trim_range_to_request_space(offset, end);
                    if overlap_start == 0 || overlap_start == overlap.wake_offset {
                        overlap.complete_status = error_status;
                    }
                }
            }

            let fulfill = req_end - req_offset;

            // If we're not done, continue to the next request.
            if fulfill < fields.pending_size {
                // Only Signal if the offset being supplied is exactly at the wake_offset. The
                // wake_offset is the next one that the caller wants, and so waking up for anything
                // before this is pointless. In the case where the page source supplies this offset
                // last it does mean we will still block until the full request is provided.
                if req_offset == fields.wake_offset {
                    self.early_wake_request_locked(cur, req_offset, req_end);
                }
                fields.pending_size -= fulfill;
                continue;
            } else if fulfill > fields.pending_size {
                // This just means that part of the request was decommitted. That's not an error,
                // but it's good to know when we're tracing.
                ltracef!("{:p}, excessive page count\n", self);
            }

            ltracef_level!(2, "{:p}, signaling {:x}\n", self, fields.offset);

            // Notify anything waiting on this range.
            locked.erase(cur);
            self.complete_request_locked(cur);
        }
    }

    /// Helper that takes an existing request and a new request range and returns whether the new
    /// range is any kind of continuation of the existing request. This is used for a mixture of
    /// correctness validation and supporting early wake requests.
    fn request_continuation_type_locked(
        self: &Arc<Self>,
        request: PageRequestRef,
        offset: u64,
        type_: PageRequestType,
    ) -> ContinuationType {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        // Check for obvious mismatches in initialization.
        if !fields.is_initialized() {
            return ContinuationType::NotContinuation;
        }
        // SAFETY: The owner is the caller, and `src` is only modified by the owner.
        let same_source = unsafe { &*request.state().src.get() }
            .as_ref()
            .is_some_and(|src| Arc::ptr_eq(src, self));
        if !same_source || fields.type_ != type_ {
            return ContinuationType::NotContinuation;
        }
        // If the start of the new range overlaps at all with the existing request then we can
        // continue using the existing request. For any portion of the new range that extends beyond
        // the existing request, this is fine as once the current request is completed that range
        // can be re-generated a new request for just that range can be made.
        if offset >= fields.offset && offset < fields.offset + fields.len {
            return ContinuationType::SameRequest;
        }
        // The new request is for a completely different range, so we cannot keep using the current
        // one. A typical cause for this would be racing with a operations on a clone that bypassed
        // the need to wait for the original request. In this case we already checked that the
        // source and type are the same above.
        ContinuationType::SameSource
    }

    fn populate_request(
        self: &Arc<Self>,
        request: PageRequestRef,
        offset: u64,
        len: u64,
        vmo_debug_info: VmoDebugInfo,
        type_: PageRequestType,
    ) -> Status {
        debug_assert!(len > 0);
        debug_assert!(page::is_aligned(offset as usize));
        debug_assert!(page::is_aligned(len as usize));

        if !self.supports_page_request_type(type_) {
            return Status::NOT_SUPPORTED;
        }

        ltracef_level!(2, "{:p} offset {:x} prefetch_len {:x}", self, offset, len);

        let mut locked = self.lock.lock();
        if locked.detached {
            return Status::BAD_STATE;
        }

        // SAFETY: The lock is held.
        if unsafe { request.fields() }.is_initialized() {
            // The only time we should see an already initialized request is if the request was an
            // early waking one and this new request is a valid continuation of that one, anything
            // else is a programming error.
            debug_assert!(request.state().early_wake);
            match self.request_continuation_type_locked(request, offset, type_) {
                ContinuationType::NotContinuation => {
                    // If the request is initialized and the new request is not some kind of
                    // continuation then we consider this a hard error. Although we could just
                    // cancel the existing request and generate a new one, this case indicates a
                    // serious logic error in the page request handling code and we should not
                    // attempt to continue.
                    panic!(
                        "Request at offset {offset:x} len {len:x} is not any kind of continuation"
                    );
                }
                ContinuationType::SameRequest => {
                    // SAFETY: The lock is held.
                    let fields = unsafe { request.fields() };
                    debug_assert!(offset >= fields.offset);
                    // By default the wake_offset was previously incremented by whatever was
                    // supplied, but to accommodate a page source supply pages out of order we reset
                    // our wake offset to the next actual offset that is missing.
                    fields.wake_offset = offset - fields.offset;
                    return Status::SHOULD_WAIT;
                }
                ContinuationType::SameSource => {
                    // The requested range does not overlap the existing request, but it's for the
                    // same source so this is just a case of the original range no longer being
                    // needed and so can cancel the request and make a new one.
                    self.cancel_request_locked(&mut locked, request);
                }
            }
        }
        self.populate_request_locked(&mut locked, request, offset, len, vmo_debug_info, type_)
    }

    /// Helper that adds the span of `len` pages at `offset` to `request` and forwards it to the
    /// provider. `offset` must be page-aligned.
    ///
    /// This function will always return ZX_ERR_SHOULD_WAIT.
    fn populate_request_locked(
        self: &Arc<Self>,
        locked: &mut PageSourceLocked,
        request: PageRequestRef,
        offset: u64,
        len: u64,
        vmo_debug_info: VmoDebugInfo,
        type_: PageRequestType,
    ) -> Status {
        debug_assert!(page::is_aligned(offset as usize));
        debug_assert!(len > 0);
        debug_assert!(page::is_aligned(len as usize));

        PageRequest::init(request, self.clone(), offset, type_, vmo_debug_info);
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };

        // Assert on overflow, since it means vmobject is trying to get out-of-bounds pages.
        fields.len = fields.len.checked_add(len).expect("request length overflows");
        debug_assert!(fields.len >= page::SIZE as u64);

        let cur_end = fields.get_end();

        if let Some(node) = locked.upper_bound(fields.type_, fields.offset) {
            // SAFETY: The lock is held.
            let node = unsafe { node.fields() };
            if fields.offset >= node.offset && cur_end >= node.get_end() {
                // If the beginning part of this request is covered by an existing request, end the
                // request at the existing request's end and wait for that request to be resolved
                // first.
                fields.len = node.get_end() - fields.offset;
            } else if fields.offset < node.offset && cur_end >= node.offset {
                // If offset is less than node's offset, then we end the request when we'd start
                // overlapping.
                fields.len = node.offset - fields.offset;
            }
        }

        self.send_request_to_provider_locked(locked, request);

        Status::SHOULD_WAIT
    }

    /// Sends a request to the backing source, or adds the request to the overlap list if the
    /// needed region has already been requested from the source.
    fn send_request_to_provider_locked(
        &self,
        locked: &mut PageSourceLocked,
        request: PageRequestRef,
    ) {
        ltracef_level!(2, "{:p} {:?}\n", self, request);
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        debug_assert!(fields.is_initialized());
        debug_assert!(self.supports_page_request_type(fields.type_));
        // Find the node with the smallest endpoint greater than offset and then check to see if
        // offset falls within that node.
        let overlap = locked.upper_bound(fields.type_, fields.offset);
        if let Some(overlap) = overlap
            // SAFETY: The lock is held.
            && unsafe { overlap.fields() }.offset <= fields.offset
        {
            // GetPage guarantees that if offset lies in an existing node, then it is completely
            // contained in that node.
            fields.in_overlap = true;
            // SAFETY: The lock is held.
            unsafe { overlap.fields() }.overlap.push_back(request);
        } else {
            debug_assert!(!fields.provider_owned);
            fields.pending_size = fields.len;
            fields.provider_owned = true;
            self.page_provider.send_async_request(request);
            locked.insert(request);
        }
    }

    /// Wakes up the given PageRequest and all overlapping requests.
    fn complete_request_locked(&self, request: PageRequestRef) {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        debug_assert!(self.supports_page_request_type(fields.type_));

        // Take the request back from the provider before waking up the corresponding thread. Once
        // the request has been taken back we are also free to modify offset.
        self.page_provider.clear_async_request(request);
        fields.provider_owned = false;

        while let Some(waiter) = fields.overlap.pop_front() {
            // SAFETY: The lock is held.
            let waiter_fields = unsafe { waiter.fields() };
            debug_assert!(!waiter_fields.provider_owned);
            waiter_fields.in_overlap = false;
            waiter_fields.offset = u64::MAX;
            waiter.event().signal_result(waiter_fields.complete_status);
        }
        fields.offset = u64::MAX;
        request.event().signal_result(fields.complete_status);
    }

    /// Removes `request` from any internal tracking. Called by a PageRequest if it needs to abort
    /// itself.
    fn cancel_request(&self, request: PageRequestRef) {
        let mut locked = self.lock.lock();
        self.cancel_request_locked(&mut locked, request);
    }

    fn cancel_request_locked(&self, locked: &mut PageSourceLocked, request: PageRequestRef) {
        // SAFETY: The lock is held.
        let fields = unsafe { request.fields() };
        ltracef!("{:p} {:x}\n", self, fields.offset);

        if !fields.is_initialized() {
            return;
        }
        debug_assert!(self.supports_page_request_type(fields.type_));

        if fields.in_overlap {
            ltracef!("Overlap node\n");
            // This node is overlapping some other node, so just remove the request
            let main_node =
                locked.upper_bound(fields.type_, fields.offset).expect("overlapped request");
            // SAFETY: The lock is held.
            let main_fields = unsafe { main_node.fields() };
            let index = main_fields.overlap.iter().position(|r| *r == request).unwrap();
            main_fields.overlap.remove(index);
            fields.in_overlap = false;
        } else if let Some(new_node) = fields.overlap.pop_front() {
            ltracef!("Outstanding with overlap\n");
            // This node is an outstanding request with overlap, so replace it with the first
            // overlap node.
            // SAFETY: The lock is held.
            let new_fields = unsafe { new_node.fields() };
            debug_assert!(!new_fields.provider_owned);
            new_fields.in_overlap = false;
            std::mem::swap(&mut new_fields.overlap, &mut fields.overlap);
            new_fields.offset = fields.offset;
            new_fields.len = fields.len;
            new_fields.pending_size = fields.pending_size;
            debug_assert_eq!(new_fields.type_, fields.type_);

            locked.erase(request);
            locked.insert(new_node);

            new_fields.provider_owned = true;
            self.page_provider.swap_async_request(request, new_node);
            fields.provider_owned = false;
        } else if fields.in_tree {
            ltracef!("Outstanding no overlap\n");
            // This node is an outstanding request with no overlap
            locked.erase(request);
            self.page_provider.clear_async_request(request);
            fields.provider_owned = false;
        }

        // Request has been cleared from the PageProvider, so we're free to modify the offset
        fields.offset = u64::MAX;
    }

    /// Ask the page source to wait on this request. Called without a lock, so the implementation
    /// is tolerant of the source having already been detached/closed.
    fn wait_on_request(&self, request: PageRequestRef, suspendable: bool) -> Result<(), Status> {
        // If we have been detached the request will already have been completed in ::Detach and so
        // the provider should instantly wake from the event.
        self.page_provider.wait_on_event(request.event(), suspendable)
    }

    /// Waits on a request's `event`, as `wait_on_request` does, as a future.
    fn wait_on_request_async<'a>(
        &'a self,
        event: &'a Event,
        suspendable: bool,
    ) -> EventWaitFuture<'a> {
        self.page_provider.wait_on_event_async(event, suspendable)
    }
}

impl Drop for PageSource {
    fn drop(&mut self) {
        ltracef!("{:p}\n", self);
        let locked = self.lock.get_mut();
        debug_assert!(locked.detached);
        debug_assert!(locked.closed);
    }
}
