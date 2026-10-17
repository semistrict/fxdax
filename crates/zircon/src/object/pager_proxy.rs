//! The page provider behind a pager's VMOs, which forwards page requests to the pager's port.
//!
//! Ported from `zircon/kernel/object/pager_proxy.cc` and
//! `zircon/kernel/object/include/object/pager_proxy.h`.
//!
//! Page requests are delivered as packets on the port. The proxy has a single packet, so while it
//! is queued further requests wait in a list, and the next is queued when the pager dequeues the
//! packet.
//!
//! Deviations:
//! - The proxy refers to its pager weakly, where Zircon holds a raw pointer cleared when the pager
//!   closes.
//! - Waits cannot be interrupted by thread suspension.
//! - Cancelling the complete packet when the pager closes breaks the reference cycle with the page
//!   source, as the comment there intends.
//! - No kcounters, ktrace, memory stall accounting or handle dumps on timeouts.
//! - A page request may be waited on by a future (`wait_on_event_async`), which does not record a
//!   blocked thread.

use super::dispatcher::SoloDispatcher;
use super::pager_dispatcher::PagerDispatcher;
use super::port_dispatcher::{PortAllocator, PortDispatcher, PortPacket};
use super::thread_dispatcher::{AutoBlocked, Blocked};
use crate::boot_options::BootOptions;
use crate::debug::ltracef;
use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::Event;
use crate::vm::page::VmPagePtr;
use crate::vm::page_source::{
    EventWaitFuture, PageProvider, PageRequest, PageRequestRef, PageRequestType, PageSource,
    PageSourceProperties,
};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};
use zx_status::Status;
use zx_types::*;

const LOCAL_TRACE: u32 = 0;

struct PagerProxyLocked {
    /// Back pointer to the dispatcher that owns this proxy, cleared when the dispatcher closes.
    pager: Option<Weak<PagerDispatcher>>,
    /// The page source this proxy provides pages for. Holding it forms a reference cycle, which
    /// is broken when the page source closes.
    page_source: Option<Arc<PageSource>>,
    /// Whether the packet is currently in use, either queued on the port or being read by the
    /// pager.
    packet_busy: bool,
    /// The request the packet currently describes.
    active_request: Option<PageRequestRef>,
    /// Requests waiting for the packet.
    pending_requests: VecDeque<PageRequestRef>,
    /// Set when the page source closes, after which no more requests will be sent.
    page_source_closed: bool,
    /// Set when the complete packet has been queued and not yet freed. Cleanup is delayed until
    /// it is freed.
    complete_pending: bool,
    /// The last time an overtime dump was printed.
    last_overtime_dump: zx_instant_mono_t,
    /// Owns the request whose packet tells the pager the VMO is detached.
    _complete_request: PageRequest,
}

pub struct PagerProxy {
    pager_koid: zx_koid_t,
    port: Arc<PortDispatcher>,
    key: u64,
    options: u32,
    /// The packet requests are delivered in.
    packet: Arc<PortPacket>,
    /// Identifies `_complete_request`.
    complete_request: PageRequestRef,
    mtx: Mutex<PagerProxyLocked>,
}

impl PagerProxy {
    /// Makes the proxy trap dirty transitions.
    pub const TRAP_DIRTY: u32 = 1 << 0;

    pub fn new(
        pager: &Arc<PagerDispatcher>,
        port: Arc<PortDispatcher>,
        key: u64,
        options: u32,
    ) -> Arc<Self> {
        let complete_request = PageRequest::default();
        let complete_ref = complete_request.as_ref();
        let proxy = Arc::new_cyclic(|this: &Weak<Self>| {
            let allocator: Weak<dyn PortAllocator> = this.clone();
            Self {
                pager_koid: pager.get_koid(),
                port,
                key,
                options,
                packet: Arc::new(PortPacket::new(allocator)),
                complete_request: complete_ref,
                mtx: Mutex::new(PagerProxyLocked {
                    pager: Some(Arc::downgrade(pager)),
                    page_source: None,
                    packet_busy: false,
                    active_request: None,
                    pending_requests: VecDeque::new(),
                    page_source_closed: false,
                    complete_pending: false,
                    last_overtime_dump: 0,
                    _complete_request: complete_request,
                }),
            }
        });
        ltracef!("{:p} key {:x} options {:x}\n", Arc::as_ptr(&proxy), key, options);
        proxy
    }

    /// Sets the page source, which must be done just after construction.
    pub fn set_page_source_unchecked(&self, src: Arc<PageSource>) {
        self.mtx.lock().page_source = Some(src);
    }

    fn queue_packet_locked(&self, locked: &mut PagerProxyLocked, request: PageRequestRef) {
        if locked.packet_busy {
            // SAFETY: The provider list is protected by `mtx`, which is held.
            unsafe { request.set_in_provider_list(true) };
            locked.pending_requests.push_back(request);
            return;
        }
        debug_assert!(locked.active_request.is_none());
        locked.packet_busy = true;
        locked.active_request = Some(request);

        let (command, offset, length) = if request != self.complete_request {
            let command = match request.request_type() {
                PageRequestType::Read => zx_page_request_command_t::ZX_PAGER_VMO_READ,
                PageRequestType::Dirty => {
                    debug_assert!(self.options & Self::TRAP_DIRTY != 0);
                    zx_page_request_command_t::ZX_PAGER_VMO_DIRTY
                }
                PageRequestType::Writeback => unreachable!(),
            };
            let (offset, length) = (request.offset(), request.len());
            // The vm subsystem should guarantee this
            debug_assert!(offset.checked_add(length).is_some());
            (command, offset, length)
        } else {
            (zx_page_request_command_t::ZX_PAGER_VMO_COMPLETE, 0, 0)
        };

        let mut page_request = zx_packet_page_request_t::default();
        page_request.command = command;
        page_request.offset = offset;
        page_request.length = length;
        let mut packet = zx_port_packet_t {
            key: self.key,
            packet_type: zx_packet_type_t::ZX_PKT_TYPE_PAGE_REQUEST,
            status: ZX_OK,
            union: [0; 32],
        };
        // SAFETY: The union is as large as the page request packet, which is plain data.
        unsafe { std::ptr::write_unaligned(packet.union.as_mut_ptr().cast(), page_request) };
        self.packet.set_packet(packet);

        // We can treat ZX_ERR_BAD_HANDLE as if the packet was queued but the pager service never
        // responds.
        assert_ne!(self.port.queue(&self.packet), Err(Status::SHOULD_WAIT));
    }

    /// Called when the pager dispatcher is going away on zero handles.
    pub fn on_dispatcher_close(&self) {
        let mut self_src = None;
        let mut locked = self.mtx.lock();

        // The PagerDispatcher is going away and there won't be a way to service any pager
        // requests. Close the PageSource from our end so that no more requests can be sent.
        // Closing the PageSource will clear/cancel any outstanding requests that it had forwarded,
        // i.e. any requests except the complete request (which is owned by us and is not visible to
        // the PageSource).
        if !locked.page_source_closed {
            // page_source is only reset to None if we already closed it.
            let src = locked.page_source.clone().expect("open page source");
            // Call close without the lock to not violate lock ordering, and to allow it to call
            // back into on_close.
            parking_lot::MutexGuard::unlocked(&mut locked, || src.close());
            self_src = Some(src);
        }

        // The pager dispatcher's reference to this object is the only one we completely control.
        // As the Pager dispatcher is going away, we are not content to keep these objects alive
        // indefinitely until messages are read, instead we want to cancel everything as soon as
        // possible to avoid memory leaks. Therefore we will attempt to cancel any queued final
        // packet.
        if locked.complete_pending {
            if self.port.cancel_queued_packet(&self.packet) {
                // We successfully cancelled the message, so we don't have to worry about free
                // being called, and can immediately break the reference cycle. (Zircon only clears
                // complete_pending here, which leaves the cycle in place.)
                locked.complete_pending = false;
                locked.active_request = None;
                locked.packet_busy = false;
                self_src = locked.page_source.take();
            } else {
                // If we failed to cancel the message, then there is a pending call to free. It will
                // cleanup the reference cycle, although only if page_source_closed is true, which
                // should be the case since we performed the close step earlier.
                debug_assert!(locked.page_source_closed);
            }
        } else {
            // Either the complete message had already been dispatched when this object was closed
            // or free was called between this object being closed and this method taking the lock.
            // In either case, the port no longer has a reference, any reference cycles have been
            // broken and cleanup is already done.
            debug_assert!(locked.page_source.is_none());
        }
        // Clear pager to ensure we don't attempt to access an already-freed pager dispatcher.
        locked.pager = None;
        drop(locked);
        drop(self_src);
    }

    fn on_packet_freed_locked(&self, locked: &mut PagerProxyLocked) {
        // We are here because the active request has been freed. And packet_busy is still true, so
        // no new request will have become active yet.
        debug_assert!(locked.active_request.is_none());
        locked.packet_busy = false;
        if let Some(request) = locked.pending_requests.pop_front() {
            // SAFETY: The provider list is protected by `mtx`, which is held.
            unsafe { request.set_in_provider_list(false) };
            self.queue_packet_locked(locked, request);
        }
    }

    /// Releases this proxy from `pager`, returning the pager's reference to it.
    fn release_from_pager(&self, locked: &PagerProxyLocked) -> Option<Arc<PagerProxy>> {
        locked.pager.as_ref().and_then(Weak::upgrade).and_then(|pager| pager.release_proxy(self))
    }

    fn print_overtime(&self, waited_seconds: u64) {
        let options = BootOptions::get();
        let (do_printout, src) = {
            let mut locked = self.mtx.lock();
            let now = current_mono_time();
            let period =
                (options.userpager_overtime_wait_seconds as i64).saturating_mul(1_000_000_000);
            let do_printout = now >= locked.last_overtime_dump.saturating_add(period);
            if do_printout {
                locked.last_overtime_dump = now;
            }
            (do_printout, locked.page_source.clone())
        };
        eprintln!(
            "WARNING Page source {:?} blocked for {} seconds. {}",
            src.as_ref().map(Arc::as_ptr),
            waited_seconds,
            if do_printout { "Dump:" } else { "Dump skipped." }
        );
        // Dump out the rest of the state of the outstanding requests.
        if do_printout {
            self.dump(0, u32::MAX);
            if let Some(src) = src {
                // Use dump_self to avoid it calling our dump method that we already performed.
                src.dump_self(0, u32::MAX);
            }
        }
    }
}

impl PortAllocator for PagerProxy {
    fn free(&self, _port_packet: &Arc<PortPacket>) {
        let mut self_ref = None;
        let mut self_src = None;
        let mut locked = self.mtx.lock();
        if locked.active_request != Some(self.complete_request) {
            // This request is still active, i.e. it has not been taken back by the PageSource with
            // clear_async_request. So we are responsible for relinquishing ownership of the
            // request.
            locked.active_request = None;
            self.on_packet_freed_locked(&mut locked);
        } else {
            // Freeing the complete_request indicates we have completed a pending action that might
            // have been delaying cleanup.
            locked.complete_pending = false;
            // Should be nothing else queued.
            debug_assert!(locked.pending_requests.is_empty());
            locked.active_request = None;
            locked.packet_busy = false;
            // If the source is closed, we need to do delayed cleanup. Make sure we are not still
            // in the pager's proxy list (if the pager is not closed yet), and then break our
            // reference cycle.
            if locked.page_source_closed {
                debug_assert!(locked.page_source.is_some());
                // If the PagerDispatcher is already closed, the proxy has already been released.
                self_ref = self.release_from_pager(&locked);
                self_src = locked.page_source.take();
            }
        }
        drop(locked);
        drop(self_src);
        drop(self_ref);
    }
}

/// How long a wait on the pager has gone on, in periods of `userpager_overtime_wait_seconds`.
#[derive(Default)]
struct Overtime {
    waited: u64,
}

impl Overtime {
    /// The deadline of the next period of the wait.
    fn deadline(&self) -> Deadline {
        let options = BootOptions::get();
        if options.userpager_overtime_wait_seconds == 0 {
            Deadline::infinite()
        } else {
            Deadline::after_mono(
                (options.userpager_overtime_wait_seconds as i64).saturating_mul(1_000_000_000),
            )
        }
    }
}

impl PagerProxy {
    /// Accounts for a period of a wait on the pager passing: reports it, and fails the wait once it
    /// has waited longer than `userpager_overtime_timeout_seconds`.
    fn overtime(&self, overtime: &mut Overtime) -> Result<(), Status> {
        let options = BootOptions::get();
        overtime.waited += 1;
        // Error out if we've been waiting for longer than the specified timeout, to allow the rest
        // of the system to make progress (if possible).
        if options.userpager_overtime_timeout_seconds > 0
            && overtime.waited * options.userpager_overtime_wait_seconds
                >= options.userpager_overtime_timeout_seconds
        {
            let src = self.mtx.lock().page_source.as_ref().map(Arc::as_ptr);
            eprintln!(
                "ERROR Page source {:?} blocked for {} seconds. Page request timed out.",
                src, options.userpager_overtime_timeout_seconds
            );
            self.dump(0, u32::MAX);
            return Err(Status::TIMED_OUT);
        }
        // Do an informational printout of the source and ourselves if the overtime period has
        // elapsed.
        self.print_overtime(overtime.waited * options.userpager_overtime_wait_seconds);
        Ok(())
    }
}

impl PageProvider for PagerProxy {
    fn properties(&self) -> PageSourceProperties {
        PageSourceProperties {
            is_user_pager: true,
            supports_request_type: [true, self.options & Self::TRAP_DIRTY != 0, false],
        }
    }

    fn send_async_request(&self, request: PageRequestRef) {
        let mut locked = self.mtx.lock();
        assert!(!locked.page_source_closed);
        self.queue_packet_locked(&mut locked, request);
    }

    fn clear_async_request(&self, request: PageRequestRef) {
        let mut locked = self.mtx.lock();
        assert!(!locked.page_source_closed);
        if locked.active_request == Some(request) {
            // This request is being taken back by the PageSource, so we can't hold a reference to
            // it anymore. This will remain None until on_packet_freed_locked is called (and a new
            // packet gets queued as a result), either by us here below or by free, since
            // packet_busy is true and will be true until on_packet_freed_locked is called.
            locked.active_request = None;
            // Condition on whether or not we actually cancel the packet, to make sure we don't race
            // with a call to free.
            if self.port.cancel_queued_packet(&self.packet) {
                self.on_packet_freed_locked(&mut locked);
            }
        // SAFETY: The provider list is protected by `mtx`, which is held.
        } else if unsafe { request.in_provider_list() } {
            let index = locked.pending_requests.iter().position(|r| *r == request).unwrap();
            locked.pending_requests.remove(index);
            // SAFETY: As above.
            unsafe { request.set_in_provider_list(false) };
        }
    }

    fn swap_async_request(&self, old: PageRequestRef, new_req: PageRequestRef) {
        let mut locked = self.mtx.lock();
        assert!(!locked.page_source_closed);
        // SAFETY: The provider list is protected by `mtx`, which is held.
        if unsafe { old.in_provider_list() } {
            let index = locked.pending_requests.iter().position(|r| *r == old).unwrap();
            locked.pending_requests[index] = new_req;
            // SAFETY: As above.
            unsafe {
                old.set_in_provider_list(false);
                new_req.set_in_provider_list(true);
            }
        } else if locked.active_request == Some(old) {
            locked.active_request = Some(new_req);
        }
    }

    fn debug_is_page_ok(&self, _page: VmPagePtr, _offset: u64) -> bool {
        true
    }

    fn on_detach(&self) {
        let mut locked = self.mtx.lock();
        assert!(!locked.page_source_closed);
        locked.complete_pending = true;
        self.queue_packet_locked(&mut locked, self.complete_request);
    }

    fn on_close(&self) {
        let mut self_ref = None;
        let mut self_src = None;
        let mut locked = self.mtx.lock();
        assert!(!locked.page_source_closed);
        locked.page_source_closed = true;
        // If there isn't a complete packet pending, we're free to clean up our ties with the
        // PageSource and PagerDispatcher right now, as we're not expecting a free to perform final
        // delayed clean up later. The PageSource is closing, so it won't need to send us any more
        // requests. The PagerDispatcher doesn't need to refer to us anymore as we won't be queueing
        // any more pager requests.
        if !locked.complete_pending {
            // The pager is absent if its on_zero_handles closed this proxy.
            self_ref = self.release_from_pager(&locked);
            self_src = locked.page_source.take();
        } else {
            // There is still a pending complete message that we would like to wait to be received
            // and so we do not perform cancel_queued like on_dispatcher_close does. However, we
            // must leave the reference to ourselves in pager so that on_dispatcher_close (and the
            // forced packet cancelling) can happen if needed. Otherwise final delayed cleanup will
            // happen in free.
        }
        drop(locked);
        drop(self_src);
        drop(self_ref);
    }

    fn wait_on_event(&self, event: &Event, _suspendable: bool) -> Result<(), Status> {
        let _blocked = AutoBlocked::new(Blocked::Pager);
        let mut overtime = Overtime::default();
        loop {
            match event.wait(&overtime.deadline()) {
                Err(Status::TIMED_OUT) => self.overtime(&mut overtime)?,
                result => return result,
            }
        }
    }

    fn wait_on_event_async<'a>(
        &'a self,
        event: &'a Event,
        _suspendable: bool,
    ) -> EventWaitFuture<'a> {
        // A future has no thread whose blocked state to record.
        Box::pin(async move {
            let mut overtime = Overtime::default();
            loop {
                match event.wait_async(overtime.deadline()).await {
                    Err(Status::TIMED_OUT) => self.overtime(&mut overtime)?,
                    result => return result,
                }
            }
        })
    }

    fn dump(&self, _depth: u32, max_items: u32) {
        let locked = self.mtx.lock();
        let name = match locked.pager.as_ref().and_then(Weak::upgrade) {
            Some(pager) => {
                String::from_utf8_lossy(&pager.debug_name()).trim_end_matches('\0').to_string()
            }
            None => "<closed>".to_string(),
        };
        eprintln!(
            "pager_dispatcher <{name}> page_source {:?} key {}",
            locked.page_source.as_ref().map(Arc::as_ptr),
            self.key
        );
        eprintln!(
            "  source_closed {} pager_closed {} packet_busy {} complete_pending {}",
            locked.page_source_closed,
            locked.pager.is_none(),
            locked.packet_busy,
            locked.complete_pending
        );
        match locked.active_request {
            Some(request) if request != self.complete_request => eprintln!(
                "  active {} request on pager port [{:#x}, {:#x}) (port koid {:#x})",
                request.request_type().as_str(),
                request.offset(),
                request.offset() + request.len(),
                self.port.base().get_koid()
            ),
            Some(_) => eprintln!(
                "  active COMPLETE request on pager port (port koid {:#x})",
                self.port.base().get_koid()
            ),
            None => eprintln!("  no active request on pager port"),
        }
        if locked.pending_requests.is_empty() {
            eprintln!("  no pending requests to queue on pager port");
            return;
        }
        for request in locked.pending_requests.iter().take(max_items as usize) {
            if *request != self.complete_request {
                eprintln!(
                    "  pending {} req to queue on pager port [{:#x}, {:#x})",
                    request.request_type().as_str(),
                    request.offset(),
                    request.offset() + request.len()
                );
            } else {
                eprintln!("  pending COMPLETE request to queue on pager port");
            }
        }
    }

    fn get_koid(&self) -> Option<u64> {
        Some(self.pager_koid)
    }
}

impl Drop for PagerProxy {
    fn drop(&mut self) {
        ltracef!("{:p}\n", self);
        let locked = self.mtx.get_mut();
        // In error paths shortly after construction, we can destruct without page_source_closed
        // becoming true.
        debug_assert!(!locked.complete_pending);
        // The packet names us as its allocator. Ensure that it is not somehow still in use, and
        // that a PortDispatcher would therefore not be able to call free.
        debug_assert!(!locked.packet_busy);
    }
}
