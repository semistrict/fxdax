// Copyright 2020 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Waiting on an object's signals from a thread.
//!
//! Ported from `zircon/kernel/object/wait_signal_observer.cc`.

use super::dispatcher::{Dispatcher, SignalObserver, TriggerMode};
use super::handle::Handle;
use crate::kernel::event::Event;
use parking_lot::Mutex;
use std::sync::Arc;
use zx_types::{ZX_SIGNAL_HANDLE_CLOSED, zx_signals_t};

struct Observer {
    event: Arc<Event>,
    signals: Mutex<zx_signals_t>,
}

impl SignalObserver for Observer {
    fn on_match(&self, signals: zx_signals_t) {
        *self.signals.lock() = signals;
        self.event.signal();
    }

    fn on_cancel(&self, signals: zx_signals_t) {
        *self.signals.lock() = signals | ZX_SIGNAL_HANDLE_CLOSED;
        self.event.signal();
    }
}

/// Signals an event when a dispatcher's signals match, for as long as it is registered.
pub struct WaitSignalObserver {
    observer: Arc<Observer>,
    dispatcher: Option<Arc<dyn Dispatcher>>,
}

impl WaitSignalObserver {
    pub fn new(event: Arc<Event>) -> Self {
        Self { observer: Arc::new(Observer { event, signals: Mutex::new(0) }), dispatcher: None }
    }

    /// Starts observing `signals` on the object `handle` refers to.
    pub fn begin(&mut self, handle: &Handle, signals: zx_signals_t) {
        debug_assert!(self.dispatcher.is_none());
        let dispatcher = handle.dispatcher().clone();
        let observer: Arc<dyn SignalObserver> = self.observer.clone();
        dispatcher.base().add_observer(observer, handle.id(), signals, TriggerMode::Level);
        self.dispatcher = Some(dispatcher);
    }

    /// Stops observing, returning the signals observed.
    pub fn end(&mut self) -> zx_signals_t {
        let dispatcher = self.dispatcher.take().expect("observer was begun");
        let observer: Arc<dyn SignalObserver> = self.observer.clone();
        let (removed, current) = dispatcher.base().remove_observer(&observer);
        if removed {
            // Not matched or canceled, so report the signals as they are now.
            *self.observer.signals.lock() = current;
        }
        *self.observer.signals.lock()
    }
}
