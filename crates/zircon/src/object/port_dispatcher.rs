//! Ports: queues of packets that threads wait on.
//!
//! Ported from `zircon/kernel/object/port_dispatcher.cc` and
//! `zircon/kernel/object/include/object/port_dispatcher.h`.
//!
//! The PortDispatcher implements the port kernel object which is the cornerstone for waiting on
//! object changes in Zircon. It handles:
//!  1- Object state change notification: zx_object_wait_async()
//!  2- Manual queuing: zx_port_queue()
//!  3- Kernel packets, such as page requests from a pager proxy.
//!
//! Locking: the port's lock guards its packet queue and list of observers, a pending observer's
//! reference to its dispatcher, and whether a packet is queued or canceled. It can be acquired
//! while holding a dispatcher's lock, but not the other way around.
//!
//! Deviations:
//! - Packets are `Arc`s. A packet's contents have their own lock, taken after the port's.
//! - Interrupt binding is not supported.
//! - There is no limit on queued user packets or observers.

use super::dispatcher::{Dispatcher, DispatcherBase, SignalObserver};
use super::handle::KernelHandle;
use super::rights::ZX_DEFAULT_PORT_RIGHTS;
use crate::kernel::deadline::{Deadline, current_mono_time};
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::{Arc, Weak};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_PORT, ZX_WAIT_ASYNC_BOOT_TIMESTAMP, ZX_WAIT_ASYNC_TIMESTAMP, zx_obj_type_t,
    zx_packet_type_t, zx_port_packet_t, zx_rights_t, zx_signals_t,
};

/// Frees packets that are dequeued or canceled from a port.
pub trait PortAllocator: Send + Sync {
    fn free(&self, port_packet: &Arc<PortPacket>);
}

/// Who frees a packet once it leaves the port.
enum PacketOwner {
    /// A user packet, dropped once dequeued.
    Default,
    /// A packet owned by an allocator, which is told when it leaves the port.
    Allocator(Weak<dyn PortAllocator>),
    /// A packet owned by a port observer.
    Observer,
}

struct PortPacketState {
    packet: zx_port_packet_t,
    /// The observer this packet belongs to, kept alive while the packet is queued.
    observer: Option<Arc<PortObserver>>,
    /// Whether the packet has been canceled, standing in for Zircon's canceled packet type.
    canceled: bool,
    /// Whether the packet is in a port's queue.
    in_container: bool,
}

/// A packet that can be queued on a port.
pub struct PortPacket {
    /// The handle an observer's packet was registered through, or 0.
    handle: u64,
    owner: PacketOwner,
    state: Mutex<PortPacketState>,
}

impl PortPacket {
    fn with_owner(handle: u64, owner: PacketOwner) -> Self {
        Self {
            handle,
            owner,
            state: Mutex::new(PortPacketState {
                packet: zx_port_packet_t {
                    key: 0,
                    packet_type: zx_packet_type_t::ZX_PKT_TYPE_USER,
                    status: 0,
                    union: [0; 32],
                },
                observer: None,
                canceled: false,
                in_container: false,
            }),
        }
    }

    /// Makes a packet owned by `allocator`, which is told when the packet leaves the port.
    pub fn new(allocator: Weak<dyn PortAllocator>) -> Self {
        Self::with_owner(0, PacketOwner::Allocator(allocator))
    }

    pub fn set_packet(&self, packet: zx_port_packet_t) {
        let mut state = self.state.lock();
        state.packet = packet;
        // Zircon marks a canceled packet by its type, which setting the packet overwrites.
        state.canceled = false;
    }

    pub fn key(&self) -> u64 {
        self.state.lock().packet.key
    }

    fn is_ephemeral(&self) -> bool {
        !matches!(self.owner, PacketOwner::Observer)
    }

    fn free(self: &Arc<Self>) {
        if let PacketOwner::Allocator(allocator) = &self.owner
            && let Some(allocator) = allocator.upgrade()
        {
            allocator.free(self);
        }
    }

    fn cancel(&self) {
        self.state.lock().canceled = true;
    }

    fn is_canceled(&self) -> bool {
        self.state.lock().canceled
    }
}

/// Writes the signal part of a packet.
fn set_signal(packet: &mut zx_port_packet_t, f: impl FnOnce(&mut zx_types::zx_packet_signal_t)) {
    let mut signal: zx_types::zx_packet_signal_t =
        // SAFETY: The union is at least as large as the signal packet, which is plain data.
        unsafe { std::ptr::read_unaligned(packet.union.as_ptr().cast()) };
    f(&mut signal);
    // SAFETY: As above.
    unsafe { std::ptr::write_unaligned(packet.union.as_mut_ptr().cast(), signal) };
}

/// An observer registered with zx_object_wait_async, which queues a packet on the port when its
/// signals match.
pub struct PortObserver {
    options: u32,
    packet: Arc<PortPacket>,
    port: Arc<PortDispatcher>,
    /// Guarded by the port's lock. Does not have an ownership stake in the dispatcher.
    dispatcher: Mutex<Option<Weak<dyn Dispatcher>>>,
}

impl PortObserver {
    fn unlink_dispatcher_locked(&self) -> Option<Weak<dyn Dispatcher>> {
        self.dispatcher.lock().take()
    }
}

impl SignalObserver for PortObserver {
    fn on_match(&self, signals: zx_signals_t) {
        if (self.options & (ZX_WAIT_ASYNC_TIMESTAMP | ZX_WAIT_ASYNC_BOOT_TIMESTAMP)) != 0 {
            let now = current_mono_time();
            set_signal(&mut self.packet.state.lock().packet, |signal| signal.timestamp = now);
        }
        // Queuing the packet transfers the observer to the packet. The last handle to the port may
        // have been closed so it can fail with ZX_ERR_BAD_HANDLE, and nothing is to be done.
        let status = self.port.queue_and_remove_observer(&self.packet, signals, Some(self));
        debug_assert!(
            matches!(status, Ok(()) | Err(Status::BAD_HANDLE | Status::CANCELED)),
            "status {status:?}"
        );
    }

    fn on_cancel(&self, _signals: zx_signals_t) {
        self.port.maybe_reap(self, &self.packet);
    }

    fn matches_key(&self, port: *const (), key: u64) -> bool {
        key == self.packet.key() && port == Arc::as_ptr(&self.port).cast()
    }
}

struct PortLocked {
    zero_handles: bool,
    packets: VecDeque<Arc<PortPacket>>,
    /// Posts to the semaphore threads dequeuing wait on.
    sema: u64,
    /// Keeps track of outstanding observers so they can be removed from dispatchers once handle
    /// count drops to zero.
    observers: Vec<Arc<PortObserver>>,
}

pub struct PortDispatcher {
    base: DispatcherBase,
    lock: Mutex<PortLocked>,
    sema_cond: Condvar,
    /// So observers can refer to the port they belong to.
    this: Weak<PortDispatcher>,
}

impl Dispatcher for PortDispatcher {
    fn base(&self) -> &DispatcherBase {
        &self.base
    }

    fn get_type(&self) -> zx_obj_type_t {
        ZX_OBJ_TYPE_PORT
    }

    fn default_rights(&self) -> zx_rights_t {
        ZX_DEFAULT_PORT_RIGHTS
    }

    fn is_waitable(&self) -> bool {
        false
    }

    fn on_zero_handles(&self) {
        let mut locked = self.lock.lock();
        debug_assert!(!locked.zero_handles);
        locked.zero_handles = true;

        // Free any queued packets.
        while let Some(packet) = locked.packets.pop_front() {
            let mut state = packet.state.lock();
            state.in_container = false;
            // If the packet is ephemeral, regardless of allocator, free it outside of the lock.
            // Otherwise, reset the observer if it is present.
            if packet.is_ephemeral() {
                drop(state);
                parking_lot::MutexGuard::unlocked(&mut locked, || packet.free());
            } else {
                state.observer = None;
            }
        }

        // For each of our outstanding observers, remove them from their dispatchers. We could be
        // racing with the dispatcher calling OnMatch/OnCancel/MaybeReap.
        while let Some(observer) = locked.observers.pop() {
            let dispatcher = observer.unlink_dispatcher_locked().and_then(|weak| weak.upgrade());
            // Don't hold the lock while calling RemoveObserver because we don't want to create a
            // PortDispatcher-to-Dispatcher lock dependency.
            parking_lot::MutexGuard::unlocked(&mut locked, || {
                if let Some(dispatcher) = dispatcher {
                    let observer: Arc<dyn SignalObserver> = observer;
                    dispatcher.base().remove_observer(&observer);
                }
            });
        }
    }
}

impl PortDispatcher {
    pub fn create(options: u32) -> Result<(KernelHandle<PortDispatcher>, zx_rights_t), Status> {
        if options != 0 {
            return Err(Status::INVALID_ARGS);
        }
        let port = Arc::new_cyclic(|this| PortDispatcher {
            base: DispatcherBase::new(0),
            lock: Mutex::new(PortLocked {
                zero_handles: false,
                packets: VecDeque::new(),
                sema: 0,
                observers: Vec::new(),
            }),
            sema_cond: Condvar::new(),
            this: this.clone(),
        });
        Ok((KernelHandle::new(port), ZX_DEFAULT_PORT_RIGHTS))
    }

    /// Queues a packet on the port.
    pub fn queue(&self, port_packet: &Arc<PortPacket>) -> Result<(), Status> {
        self.queue_and_remove_observer(port_packet, 0, None)
    }

    /// Queues a user packet.
    pub fn queue_user(&self, packet: &zx_port_packet_t) -> Result<(), Status> {
        let port_packet = Arc::new(PortPacket::with_owner(0, PacketOwner::Default));
        let mut user_packet = *packet;
        user_packet.packet_type = zx_packet_type_t::ZX_PKT_TYPE_USER;
        port_packet.set_packet(user_packet);
        self.queue(&port_packet)
    }

    /// Queues a packet based on a signal observation and deregisters the observer from the port.
    fn queue_and_remove_observer(
        &self,
        port_packet: &Arc<PortPacket>,
        observed: zx_signals_t,
        observer: Option<&PortObserver>,
    ) -> Result<(), Status> {
        {
            let mut locked = self.lock.lock();
            let status = Self::queue_packet_locked(&mut locked, port_packet, observed);
            if let Some(observer) = observer {
                Self::maybe_reap_locked(&mut locked, observer, port_packet);
            }
            status?;
            locked.sema += 1;
        }
        self.sema_cond.notify_one();
        Ok(())
    }

    fn queue_packet_locked(
        locked: &mut PortLocked,
        port_packet: &Arc<PortPacket>,
        observed: zx_signals_t,
    ) -> Result<(), Status> {
        if locked.zero_handles {
            return Err(Status::BAD_HANDLE);
        }
        let mut state = port_packet.state.lock();
        if state.canceled {
            return Err(Status::CANCELED);
        }
        if observed != 0 {
            set_signal(&mut state.packet, |signal| {
                signal.observed = observed;
                // |count| previously stored the number of pending messages on a channel. It is now
                // deprecated, but we set it to 1 for backwards compatibility.
                signal.count = 1;
            });
        }
        debug_assert!(!state.in_container);
        state.in_container = true;
        drop(state);
        locked.packets.push_back(port_packet.clone());
        Ok(())
    }

    /// Waits for a packet until `deadline`.
    pub fn dequeue(&self, deadline: &Deadline) -> Result<zx_port_packet_t, Status> {
        let mut locked = self.lock.lock();
        loop {
            // Wait until the queue has a packet.
            while locked.sema == 0 {
                match deadline.remaining() {
                    None => self.sema_cond.wait(&mut locked),
                    Some(remaining) if remaining.is_zero() => return Err(Status::TIMED_OUT),
                    Some(remaining) => {
                        self.sema_cond.wait_for(&mut locked, remaining);
                    }
                }
            }
            locked.sema -= 1;

            let Some(port_packet) = locked.packets.pop_front() else {
                // The packet must have been removed before we were able to dequeue. Loop back and
                // wait again.
                continue;
            };
            let mut state = port_packet.state.lock();
            state.in_container = false;
            debug_assert!(!state.canceled);
            let packet = state.packet;
            state.observer = None;
            drop(state);
            drop(locked);
            // If the packet is ephemeral, free it outside of the lock.
            if port_packet.is_ephemeral() {
                port_packet.free();
            }
            return Ok(packet);
        }
    }

    fn maybe_reap(&self, observer: &PortObserver, port_packet: &Arc<PortPacket>) {
        let mut locked = self.lock.lock();
        Self::maybe_reap_locked(&mut locked, observer, port_packet);
    }

    /// Unlinks an observer from its dispatcher and removes it from our observer list if it is not
    /// already removed.
    fn maybe_reap_locked(
        locked: &mut PortLocked,
        observer: &PortObserver,
        port_packet: &Arc<PortPacket>,
    ) {
        // If the packet's already canceled, we do not have to do anything. The canceling thread is
        // responsible for reaping the observer - see cancel_key().
        if port_packet.is_canceled() {
            return;
        }
        // We may be racing with on_zero_handles. Whichever one of us unlinks the dispatcher will be
        // responsible for ensuring the observer is cleaned up.
        if observer.unlink_dispatcher_locked().is_some() {
            let index = locked
                .observers
                .iter()
                .position(|o| std::ptr::eq(Arc::as_ptr(o), observer))
                .expect("observer is registered with the port");
            let observer = locked.observers.swap_remove(index);
            // If the packet is queued, the observer lives until it is dequeued or canceled.
            let mut state = port_packet.state.lock();
            if state.in_container {
                state.observer = Some(observer);
            }
        }
    }

    /// Creates an observer registered with this port, for `dispatcher` via the handle `handle`.
    pub fn make_observer(
        &self,
        options: u32,
        handle: u64,
        dispatcher: &Arc<dyn Dispatcher>,
        key: u64,
        signals: zx_signals_t,
    ) -> Arc<PortObserver> {
        let packet = Arc::new(PortPacket::with_owner(handle, PacketOwner::Observer));
        {
            let mut state = packet.state.lock();
            state.packet.key = key;
            state.packet.packet_type = zx_packet_type_t::ZX_PKT_TYPE_SIGNAL_ONE;
            state.packet.status = 0;
            set_signal(&mut state.packet, |signal| signal.trigger = signals);
        }
        let observer = Arc::new(PortObserver {
            options,
            packet,
            port: self.this.upgrade().expect("port is alive"),
            dispatcher: Mutex::new(Some(Arc::downgrade(dispatcher))),
        });
        let mut locked = self.lock.lock();
        debug_assert!(!locked.zero_handles);
        locked.observers.push(observer.clone());
        observer
    }

    /// Removes the queued packets matching `handle` (if not zero) and `key`, returning any
    /// ephemeral ones for the caller to free with the lock released.
    fn cancel_queued_packets_locked(
        locked: &mut PortLocked,
        handle: u64,
        key: u64,
        free_list: &mut Vec<Arc<PortPacket>>,
    ) -> bool {
        let mut packet_removed = false;
        let mut index = 0;
        while index < locked.packets.len() {
            let packet = &locked.packets[index];
            if (handle == 0 || packet.handle == handle) && packet.key() == key {
                let packet = locked.packets.remove(index).unwrap();
                let mut state = packet.state.lock();
                state.in_container = false;
                let observer = state.observer.take();
                if packet.is_ephemeral() {
                    // Ephemeral packets do not have an observer to clean them up. It is up to us to
                    // Free them; we cannot do so here because external allocators like PagerProxy
                    // take their own locks and re-enter this port during Free.
                    debug_assert!(observer.is_none());
                    state.canceled = true;
                    drop(state);
                    free_list.push(packet);
                }
                packet_removed = true;
            } else {
                index += 1;
            }
        }
        packet_removed
    }

    /// Removes queued packets registered through `handle` with `key`. Returns true if any were
    /// removed.
    pub fn cancel_queued(&self, handle: u64, key: u64) -> bool {
        let mut free_list = Vec::new();
        let removed = {
            let mut locked = self.lock.lock();
            Self::cancel_queued_packets_locked(&mut locked, handle, key, &mut free_list)
        };
        // Ephemeral packets are queued without a handle, so none are canceled here.
        debug_assert!(free_list.is_empty());
        removed
    }

    /// Removes `port_packet` from this port's queue. Returns false if the packet was not in this
    /// queue.
    pub fn cancel_queued_packet(&self, port_packet: &Arc<PortPacket>) -> bool {
        let mut locked = self.lock.lock();
        let mut state = port_packet.state.lock();
        if state.in_container && !state.canceled {
            let index = locked
                .packets
                .iter()
                .position(|p| Arc::ptr_eq(p, port_packet))
                .expect("packet is queued on this port");
            locked.packets.remove(index);
            state.in_container = false;
            state.observer = None;
            return true;
        }
        false
    }

    /// Cancel all pending waits and packets registered with `key`. Returns ZX_OK if any waits or
    /// packets were canceled and ZX_ERR_NOT_FOUND if no matching waits or packets were found.
    pub fn cancel_key(&self, key: u64) -> Result<(), Status> {
        let mut free_list = Vec::new();
        let observer_canceled;
        let packet_removed;
        {
            let mut locked = self.lock.lock();
            let this: *const () = (self as *const Self).cast();
            let mut canceled_observers = Vec::new();
            let mut index = 0;
            while index < locked.observers.len() {
                if locked.observers[index].matches_key(this, key) {
                    let observer = locked.observers.swap_remove(index);
                    observer.packet.cancel();
                    // Acquire a reference to the dispatcher while holding our lock, so it is alive
                    // when we remove the observer from it below.
                    let dispatcher =
                        observer.unlink_dispatcher_locked().and_then(|weak| weak.upgrade());
                    canceled_observers.push((observer, dispatcher));
                } else {
                    index += 1;
                }
            }
            observer_canceled = !canceled_observers.is_empty();
            packet_removed =
                Self::cancel_queued_packets_locked(&mut locked, 0, key, &mut free_list);

            for (observer, dispatcher) in canceled_observers {
                parking_lot::MutexGuard::unlocked(&mut locked, || {
                    if let Some(dispatcher) = dispatcher {
                        let observer: Arc<dyn SignalObserver> = observer;
                        dispatcher.base().remove_observer(&observer);
                    }
                });
            }
        }
        // Free ephemeral packets with the lock dropped.
        for packet in free_list {
            packet.free();
        }
        if observer_canceled || packet_removed { Ok(()) } else { Err(Status::NOT_FOUND) }
    }
}

impl Drop for PortDispatcher {
    fn drop(&mut self) {
        debug_assert!(self.lock.get_mut().zero_handles);
    }
}
