// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel objects and their signals.
//!
//! Ported from `zircon/kernel/object/dispatcher.cc`,
//! `zircon/kernel/object/include/object/dispatcher.h` and
//! `zircon/kernel/object/include/object/signal_observer.h`.
//!
//! A dispatcher is the kernel side of an object that handles refer to. Each has a koid, a count
//! of the handles referring to it, and a set of signals that observers can wait on.
//!
//! Deviations:
//! - `Dispatcher` is a trait implemented by each object type, with the common state in
//!   `DispatcherBase`, and what the type says of it in `SoloDispatcher`. Objects are held in `Arc`s rather than `fbl::RefPtr`s.
//! - Observers are held by `Arc` and identified by pointer. The handle an observer was registered
//!   through is identified by the handle's id rather than its address.
//! - Peered dispatchers and their shared locks are not ported.

use crate::ensure;
use parking_lot::Mutex;
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use zx_status::Status;
use zx_types::{
    ZX_KOID_FIRST, ZX_USER_SIGNAL_ALL, zx_info_handle_basic_t, zx_koid_t, zx_obj_type_t,
    zx_rights_t, zx_signals_t,
};

/// Something waiting for a dispatcher's signals.
pub trait SignalObserver: Send + Sync {
    /// Called with the dispatcher's lock held when one of the triggering signals is active. The
    /// observer has been removed from the dispatcher.
    fn on_match(&self, signals: zx_signals_t);

    /// Called with the dispatcher's lock held when the wait is canceled, e.g. because the handle
    /// was closed. The observer has been removed from the dispatcher.
    fn on_cancel(&self, signals: zx_signals_t);

    /// Whether this observer was registered by `port` with `key`.
    fn matches_key(&self, _port: *const (), _key: u64) -> bool {
        false
    }
}

struct ObserverEntry {
    observer: Arc<dyn SignalObserver>,
    handle: u64,
    triggering_signals: zx_signals_t,
}

/// How an observer added with `add_observer` is triggered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerMode {
    /// Triggered if the signals are active when added, or become active later.
    Level,
    /// Only triggered when the signals become active after being added.
    Edge,
}

/// State common to every dispatcher.
pub struct DispatcherBase {
    koid: zx_koid_t,
    handle_count: AtomicU32,
    signals: AtomicU32,
    /// Observers of this object's signals.
    lock: Mutex<Vec<ObserverEntry>>,
}

impl DispatcherBase {
    pub fn new(signals: zx_signals_t) -> Self {
        Self {
            koid: generate_koid(),
            handle_count: AtomicU32::new(0),
            signals: AtomicU32::new(signals),
            lock: Mutex::new(Vec::new()),
        }
    }

    pub fn get_koid(&self) -> zx_koid_t {
        self.koid
    }

    pub fn increment_handle_count(&self) {
        self.handle_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns true if the count reached zero.
    pub fn decrement_handle_count(&self) -> bool {
        let previous = self.handle_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        previous == 1
    }

    pub fn current_handle_count(&self) -> u32 {
        self.handle_count.load(Ordering::Acquire)
    }

    /// Adds an observer. With `TriggerMode::Level`, it is immediately called back if any of
    /// `signals` are active.
    pub fn add_observer(
        &self,
        observer: Arc<dyn SignalObserver>,
        handle: u64,
        signals: zx_signals_t,
        trigger_mode: TriggerMode,
    ) {
        let mut observers = self.lock.lock();
        if trigger_mode == TriggerMode::Level {
            let active_signals = self.signals.load(Ordering::Acquire);
            if (active_signals & signals) != 0 {
                observer.on_match(active_signals);
                return;
            }
        }
        observers.push(ObserverEntry { observer, handle, triggering_signals: signals });
    }

    /// Removes an observer, returning whether it was still registered and the current signals.
    pub fn remove_observer(&self, observer: &Arc<dyn SignalObserver>) -> (bool, zx_signals_t) {
        let mut observers = self.lock.lock();
        let signals = self.signals.load(Ordering::Acquire);
        match observers.iter().position(|entry| Arc::ptr_eq(&entry.observer, observer)) {
            Some(index) => {
                observers.remove(index);
                (true, signals)
            }
            None => (false, signals),
        }
    }

    /// Cancels observers registered through `handle`.
    pub fn cancel(&self, handle: u64) {
        let mut observers = self.lock.lock();
        let signals = self.signals.load(Ordering::Acquire);
        let mut index = 0;
        while index < observers.len() {
            if observers[index].handle != handle {
                index += 1;
                continue;
            }
            let entry = observers.remove(index);
            entry.observer.on_cancel(signals);
        }
    }

    /// Like `cancel` but issued via zx_port_cancel().
    pub fn cancel_by_key(&self, handle: u64, port: *const (), key: u64) -> bool {
        let mut observers = self.lock.lock();
        let signals = self.signals.load(Ordering::Acquire);
        let mut remove_performed = false;
        let mut index = 0;
        while index < observers.len() {
            let entry = &observers[index];
            if entry.handle != handle || !entry.observer.matches_key(port, key) {
                index += 1;
                continue;
            }
            let entry = observers.remove(index);
            entry.observer.on_cancel(signals);
            remove_performed = true;
        }
        remove_performed
    }

    /// Sets and clears signals, notifying observers of newly active ones. Signals in
    /// `strobe_mask` notify observers without becoming active.
    pub fn update_state(&self, clear_mask: zx_signals_t, set_mask: zx_signals_t) {
        self.update_state_strobe(clear_mask, set_mask, 0);
    }

    pub fn update_state_strobe(
        &self,
        clear_mask: zx_signals_t,
        set_mask: zx_signals_t,
        strobe_mask: zx_signals_t,
    ) {
        if set_mask == 0 && strobe_mask == 0 {
            self.signals.fetch_and(!clear_mask, Ordering::AcqRel);
            return;
        }
        let mut observers = self.lock.lock();
        let mut previous = self.signals.load(Ordering::Acquire);
        let mut updated;
        loop {
            updated = (previous & !clear_mask) | set_mask;
            match self.signals.compare_exchange(
                previous,
                updated,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => previous = actual,
            }
        }
        let newly_active = set_mask & !previous;
        if newly_active == 0 && strobe_mask == 0 {
            return;
        }
        let signals = updated | strobe_mask;
        let mut index = 0;
        while index < observers.len() {
            if (observers[index].triggering_signals & signals) == 0 {
                index += 1;
                continue;
            }
            let entry = observers.remove(index);
            entry.observer.on_match(signals);
        }
    }

    pub fn poll_signals(&self) -> zx_signals_t {
        self.signals.load(Ordering::Acquire)
    }
}

/// What an object's type says of it, as Zircon's `SoloDispatcher<Self, default_rights>` and the
/// type's `get_type` do: [`solo_dispatcher!`] implements it.
pub trait SoloDispatcher {
    fn base(&self) -> &DispatcherBase;

    fn get_type(&self) -> zx_obj_type_t;

    /// The rights a handle to a newly created object of this type has.
    fn default_rights(&self) -> zx_rights_t;
}

/// Implements [`SoloDispatcher`] for `$type`, of object type `$obj_type` and default rights
/// `$rights`, whose `base` field is its `DispatcherBase`.
macro_rules! solo_dispatcher {
    ($type:ty, $obj_type:expr, $rights:expr) => {
        impl $crate::object::dispatcher::SoloDispatcher for $type {
            fn base(&self) -> &$crate::object::dispatcher::DispatcherBase {
                &self.base
            }

            fn get_type(&self) -> ::zx_types::zx_obj_type_t {
                $obj_type
            }

            fn default_rights(&self) -> ::zx_types::zx_rights_t {
                $rights
            }
        }
    };
}
pub(crate) use solo_dispatcher;

/// The kernel side of an object.
pub trait Dispatcher: SoloDispatcher + Any + Send + Sync {
    /// Called when the last handle to the object is closed.
    fn on_zero_handles(&self) {}

    fn get_related_koid(&self) -> zx_koid_t {
        0
    }

    /// Whether the object has signals that can be waited on.
    fn is_waitable(&self) -> bool {
        true
    }

    /// Signals userspace may set on the object in addition to the user signals.
    fn extra_user_signals(&self) -> zx_signals_t {
        0
    }

    fn get_name(&self) -> Result<[u8; zx_types::ZX_MAX_NAME_LEN], Status> {
        Err(Status::WRONG_TYPE)
    }

    fn set_name(&self, _name: &[u8]) -> Result<(), Status> {
        Err(Status::WRONG_TYPE)
    }
}

impl dyn Dispatcher {
    pub fn get_koid(&self) -> zx_koid_t {
        self.base().get_koid()
    }

    pub fn get_handle_info(&self, rights: zx_rights_t) -> zx_info_handle_basic_t {
        zx_info_handle_basic_t {
            koid: self.get_koid(),
            rights,
            type_: self.get_type(),
            related_koid: self.get_related_koid(),
            ..Default::default()
        }
    }

    /// Sets or clears user signals, as zx_object_signal does.
    pub fn user_signal_self(
        &self,
        clear_mask: zx_signals_t,
        set_mask: zx_signals_t,
    ) -> Result<(), Status> {
        ensure!(self.is_waitable(), Status::NOT_SUPPORTED);
        let allowed_signals = ZX_USER_SIGNAL_ALL | self.extra_user_signals();
        ensure!(
            (set_mask & !allowed_signals) == 0 && (clear_mask & !allowed_signals) == 0,
            Status::INVALID_ARGS
        );
        self.base().update_state(clear_mask, set_mask);
        Ok(())
    }
}

/// Downcasts a dispatcher to its concrete type.
pub fn downcast<T: Dispatcher>(dispatcher: Arc<dyn Dispatcher>) -> Option<Arc<T>> {
    let any: Arc<dyn Any + Send + Sync> = dispatcher;
    any.downcast::<T>().ok()
}

/// Returns a new kernel object id.
pub fn generate_koid() -> zx_koid_t {
    static NEXT: AtomicU64 = AtomicU64::new(ZX_KOID_FIRST);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
