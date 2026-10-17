//! Event objects.
//!
//! Ported from `zircon/kernel/object/event_dispatcher.cc`.

use super::dispatcher::{Dispatcher, DispatcherBase};
use super::handle::KernelHandle;
use super::rights::ZX_DEFAULT_EVENT_RIGHTS;
use crate::ensure;
use std::sync::Arc;
use zx_status::Status;
use zx_types::{ZX_EVENT_SIGNALED, ZX_OBJ_TYPE_EVENT, zx_obj_type_t, zx_rights_t, zx_signals_t};

pub struct EventDispatcher {
    base: DispatcherBase,
}

impl Dispatcher for EventDispatcher {
    fn base(&self) -> &DispatcherBase {
        &self.base
    }

    fn get_type(&self) -> zx_obj_type_t {
        ZX_OBJ_TYPE_EVENT
    }

    fn default_rights(&self) -> zx_rights_t {
        ZX_DEFAULT_EVENT_RIGHTS
    }

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
