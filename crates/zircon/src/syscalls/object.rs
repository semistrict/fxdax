//! Object syscalls, ported from `zircon/kernel/lib/syscalls/object.cc`,
//! `zircon/kernel/lib/syscalls/object_info.rs`, `zircon/kernel/lib/syscalls/object_property.rs`
//! and `zircon/kernel/lib/syscalls/object_wait.rs`.
//!
//! Deviation: only the info topics and properties for the objects this kernel provides are
//! supported, and only the current version of `ZX_INFO_VMO`.

use super::{copy_in_vec, copy_out, copy_out_optional, copy_out_slice, status};
use crate::ensure;
use crate::kernel::deadline::Deadline;
use crate::kernel::event::Event;
use crate::object::dispatcher::downcast;
use crate::object::dispatcher::{SignalObserver, TriggerMode};
use crate::object::handle::handle_table;
use crate::object::port_dispatcher::PortDispatcher;
use crate::object::stream_dispatcher::StreamDispatcher;
use crate::object::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::object::vm_object_dispatcher::VmObjectDispatcher;
use crate::object::wait_signal_observer::WaitSignalObserver;
use std::sync::Arc;
use zx_status::Status;
use zx_types::*;

/// Writes a single record of info, as `single_record_result` in `object_info.rs` does.
///
/// # Safety
///
/// The pointers are the caller's.
unsafe fn single_record_result<T: Copy>(
    buffer: *mut u8,
    buffer_size: usize,
    actual: *mut usize,
    avail: *mut usize,
    record: &T,
) -> Result<(), Status> {
    let written = if buffer_size >= size_of::<T>() {
        // SAFETY: The pointers are the caller's.
        unsafe { copy_out(buffer.cast::<T>(), *record) }?;
        1
    } else {
        0
    };
    // SAFETY: The pointers are the caller's.
    unsafe {
        copy_out_optional(actual, written)?;
        copy_out_optional(avail, 1)?;
    }
    ensure!(written != 0, Status::BUFFER_TOO_SMALL);
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_get_info(
    handle: zx_handle_t,
    topic: u32,
    buffer: *mut u8,
    buffer_size: usize,
    actual: *mut usize,
    avail: *mut usize,
) -> zx_status_t {
    status((|| {
        let (dispatcher, rights) = handle_table().get_dispatcher_and_rights(handle)?;
        // SAFETY: The pointers are the caller's.
        unsafe {
            match topic {
                ZX_INFO_HANDLE_VALID => Ok(()),
                ZX_INFO_HANDLE_BASIC => single_record_result(
                    buffer,
                    buffer_size,
                    actual,
                    avail,
                    &dispatcher.get_handle_info(rights),
                ),
                ZX_INFO_HANDLE_COUNT => {
                    ensure!((rights & ZX_RIGHT_INSPECT) != 0, Status::ACCESS_DENIED);
                    let count = zx_info_handle_count_t {
                        handle_count: dispatcher.base().current_handle_count(),
                    };
                    single_record_result(buffer, buffer_size, actual, avail, &count)
                }
                ZX_INFO_VMAR => {
                    let vmar = downcast::<VmAddressRegionDispatcher>(dispatcher)
                        .ok_or(Status::WRONG_TYPE)?;
                    single_record_result(buffer, buffer_size, actual, avail, &vmar.get_vmar_info())
                }
                ZX_INFO_VMO => {
                    let vmo =
                        downcast::<VmObjectDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                    let info = vmo.get_vmo_info(rights);
                    single_record_result(buffer, buffer_size, actual, avail, &info)
                }
                ZX_INFO_STREAM => {
                    let stream =
                        downcast::<StreamDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                    ensure!((rights & ZX_RIGHT_INSPECT) != 0, Status::ACCESS_DENIED);
                    single_record_result(buffer, buffer_size, actual, avail, &stream.get_info())
                }
                _ => Err(Status::NOT_SUPPORTED),
            }
        }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_get_property(
    handle: zx_handle_t,
    property: u32,
    value: *mut u8,
    value_size: usize,
) -> zx_status_t {
    status((|| {
        let (dispatcher, rights) = handle_table().get_dispatcher_and_rights(handle)?;
        ensure!((rights & ZX_RIGHT_GET_PROPERTY) != 0, Status::ACCESS_DENIED);
        ensure!(!value.is_null(), Status::INVALID_ARGS);
        let bytes: Vec<u8> = match property {
            ZX_PROP_NAME => {
                ensure!(value_size >= ZX_MAX_NAME_LEN, Status::BUFFER_TOO_SMALL);
                dispatcher.get_name()?.to_vec()
            }
            ZX_PROP_VMO_CONTENT_SIZE => {
                ensure!(value_size >= size_of::<u64>(), Status::BUFFER_TOO_SMALL);
                let vmo = downcast::<VmObjectDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                vmo.get_stream_size().to_ne_bytes().to_vec()
            }
            ZX_PROP_STREAM_MODE_APPEND => {
                ensure!(value_size >= size_of::<u8>(), Status::BUFFER_TOO_SMALL);
                let stream = downcast::<StreamDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                vec![stream.is_in_append_mode() as u8]
            }
            _ => return Err(Status::NOT_SUPPORTED),
        };
        ensure!(value_size >= bytes.len(), Status::BUFFER_TOO_SMALL);
        // SAFETY: `value` has `value_size` bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), value, bytes.len()) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_set_property(
    handle: zx_handle_t,
    property: u32,
    value: *const u8,
    value_size: usize,
) -> zx_status_t {
    status((|| {
        let (dispatcher, rights) = handle_table().get_dispatcher_and_rights(handle)?;
        ensure!((rights & ZX_RIGHT_SET_PROPERTY) != 0, Status::ACCESS_DENIED);
        // SAFETY: Bytes are plain data.
        let bytes = unsafe { copy_in_vec(value, value_size) }?;
        match property {
            ZX_PROP_NAME => {
                let len = bytes.len().min(ZX_MAX_NAME_LEN - 1);
                dispatcher.set_name(&bytes[..len])
            }
            ZX_PROP_VMO_CONTENT_SIZE => {
                ensure!((rights & ZX_RIGHT_WRITE) != 0, Status::ACCESS_DENIED);
                let Some(stream_size) = bytes.first_chunk::<8>() else {
                    return Err(Status::BUFFER_TOO_SMALL);
                };
                let vmo = downcast::<VmObjectDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                vmo.set_stream_size(u64::from_ne_bytes(*stream_size))
            }
            ZX_PROP_STREAM_MODE_APPEND => {
                let Some(&value) = bytes.first() else {
                    return Err(Status::BUFFER_TOO_SMALL);
                };
                let stream = downcast::<StreamDispatcher>(dispatcher).ok_or(Status::WRONG_TYPE)?;
                stream.set_append_mode(value != 0);
                Ok(())
            }
            _ => Err(Status::NOT_SUPPORTED),
        }
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_signal(
    handle: zx_handle_t,
    clear_mask: u32,
    set_mask: u32,
) -> zx_status_t {
    status((|| {
        let (dispatcher, rights) = handle_table().get_dispatcher_and_rights(handle)?;
        ensure!((rights & ZX_RIGHT_SIGNAL) != 0, Status::ACCESS_DENIED);
        dispatcher.user_signal_self(clear_mask, set_mask)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_wait_one(
    handle: zx_handle_t,
    signals: zx_signals_t,
    deadline: zx_instant_mono_t,
    observed: *mut zx_signals_t,
) -> zx_status_t {
    status((|| {
        let event = Arc::new(Event::new(false));
        let mut wait_signal_observer = WaitSignalObserver::new(event.clone());
        handle_table().with_handle(handle, |handle| {
            ensure!(handle.has_rights(ZX_RIGHT_WAIT), Status::ACCESS_DENIED);
            ensure!(handle.dispatcher().is_waitable(), Status::NOT_SUPPORTED);
            wait_signal_observer.begin(handle, signals);
            Ok(())
        })??;

        // Event::wait() will return ZX_OK if already signaled, even if the deadline has passed. It
        // will return ZX_ERR_TIMED_OUT after the deadline passes if the event has not been
        // signaled.
        let wait_result = event.wait(&Deadline::no_slack(deadline));

        // Regardless of wait outcome, we must call end().
        let signals_state = wait_signal_observer.end();
        // SAFETY: `observed` is the caller's.
        unsafe { copy_out_optional(observed, signals_state) }?;
        ensure!((signals_state & ZX_SIGNAL_HANDLE_CLOSED) == 0, Status::CANCELED);
        wait_result
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_wait_many(
    user_items: *mut zx_wait_item_t,
    count: usize,
    deadline: zx_instant_mono_t,
) -> zx_status_t {
    status((|| {
        if count == 0 {
            // Nothing signals this event, so the wait lasts until the deadline.
            Event::new(false).wait(&Deadline::no_slack(deadline))?;
            return Err(Status::TIMED_OUT);
        }

        ensure!(count <= ZX_WAIT_MANY_MAX_ITEMS, Status::OUT_OF_RANGE);

        // SAFETY: `user_items` is the caller's.
        let mut items = unsafe { copy_in_vec(user_items, count) }?;

        let event = Arc::new(Event::new(false));
        let mut observers: Vec<WaitSignalObserver> =
            (0..count).map(|_| WaitSignalObserver::new(event.clone())).collect();

        // We may need to unwind.
        let mut num_added = 0;
        let begin_result = (|| {
            for (ix, item) in items.iter().enumerate() {
                handle_table().with_handle(item.handle, |handle| {
                    ensure!(handle.has_rights(ZX_RIGHT_WAIT), Status::ACCESS_DENIED);
                    ensure!(handle.dispatcher().is_waitable(), Status::NOT_SUPPORTED);
                    observers[ix].begin(handle, item.waitfor);
                    Ok(())
                })??;
                num_added += 1;
            }
            Ok(())
        })();

        if let Err(err) = begin_result {
            for obs in &mut observers[..num_added] {
                obs.end();
            }
            return Err(err);
        }

        // Event::wait() will return ZX_OK if already signaled, even if the deadline has passed. It
        // will return ZX_ERR_TIMED_OUT after the deadline passes if the event has not been
        // signaled.
        let wait_result = event.wait(&Deadline::no_slack(deadline));

        // Regardless of wait outcome, we must call end().
        let mut combined = 0;
        for (ix, obs) in observers.iter_mut().enumerate() {
            let pending = obs.end();
            items[ix].pending = pending;
            combined |= pending;
        }

        // SAFETY: `user_items` is the caller's.
        unsafe { copy_out_slice(user_items, &items) }?;

        ensure!((combined & ZX_SIGNAL_HANDLE_CLOSED) == 0, Status::CANCELED);

        wait_result
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_object_wait_async(
    handle: zx_handle_t,
    port: zx_handle_t,
    key: u64,
    signals: zx_signals_t,
    options: u32,
) -> zx_status_t {
    status((|| {
        if (options
            & !(ZX_WAIT_ASYNC_TIMESTAMP | ZX_WAIT_ASYNC_BOOT_TIMESTAMP | ZX_WAIT_ASYNC_EDGE))
            != 0
        {
            return Err(Status::INVALID_ARGS);
        }
        // It is invalid to request both a monotonic and boot timestamp.
        if (options & ZX_WAIT_ASYNC_TIMESTAMP) != 0 && (options & ZX_WAIT_ASYNC_BOOT_TIMESTAMP) != 0
        {
            return Err(Status::INVALID_ARGS);
        }
        let port =
            handle_table().get_dispatcher_with_rights::<PortDispatcher>(port, ZX_RIGHT_WRITE)?;
        handle_table().with_handle(handle, |handle| {
            ensure!(handle.has_rights(ZX_RIGHT_WAIT), Status::ACCESS_DENIED);
            let dispatcher = handle.dispatcher();
            ensure!(dispatcher.is_waitable(), Status::NOT_SUPPORTED);
            let observer: Arc<dyn SignalObserver> =
                port.make_observer(options, handle.id(), dispatcher, key, signals);
            let trigger_mode = if (options & ZX_WAIT_ASYNC_EDGE) != 0 {
                TriggerMode::Edge
            } else {
                TriggerMode::Level
            };
            dispatcher.base().add_observer(observer, handle.id(), signals, trigger_mode);
            Ok(())
        })?
    })())
}
