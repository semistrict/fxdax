//! Event objects.
//!
//! Ported from `zircon/kernel/object/event_dispatcher.cc`.

use super::dispatcher::{Dispatcher, DispatcherBase, solo_dispatcher};
use super::handle::KernelHandle;
use super::rights::ZX_DEFAULT_EVENT_RIGHTS;
use crate::ensure;
use std::sync::Arc;
use zx_status::Status;
use zx_types::{ZX_EVENT_SIGNALED, ZX_OBJ_TYPE_EVENT, zx_rights_t, zx_signals_t};

pub struct EventDispatcher {
    base: DispatcherBase,
}

solo_dispatcher!(EventDispatcher, ZX_OBJ_TYPE_EVENT, ZX_DEFAULT_EVENT_RIGHTS);

impl Dispatcher for EventDispatcher {
    fn extra_user_signals(&self) -> zx_signals_t {
        ZX_EVENT_SIGNALED
    }
}

impl EventDispatcher {
    pub fn create(options: u32) -> Result<(KernelHandle<EventDispatcher>, zx_rights_t), Status> {
        ensure!(options == 0, Status::INVALID_ARGS);
        let event = Arc::new(EventDispatcher { base: DispatcherBase::new(0) });
        Ok((KernelHandle::new(event), ZX_DEFAULT_EVENT_RIGHTS))
    }
}
