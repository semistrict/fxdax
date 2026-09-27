//! Kernel objects, ported from Zircon's `kernel/object`.

pub mod dispatcher;
pub mod event_dispatcher;
pub mod handle;
pub mod pager_dispatcher;
pub mod pager_proxy;
pub mod port_dispatcher;
pub mod rights;
pub mod vm_address_region_dispatcher;
pub mod vm_object_dispatcher;
pub mod wait_signal_observer;
