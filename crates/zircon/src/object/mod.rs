//! Kernel objects, ported from Zircon's `kernel/object`.

pub mod dispatcher;
pub mod event_dispatcher;
pub mod handle;
pub mod port_dispatcher;
pub mod rights;
pub mod wait_signal_observer;
