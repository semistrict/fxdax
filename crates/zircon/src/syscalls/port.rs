// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Port syscalls, ported from `zircon/kernel/lib/syscalls/port.cc`.

use super::{copy_in, copy_out};
use crate::ensure;
use crate::kernel::deadline::Deadline;
use crate::object::handle::handle_table;
use crate::object::port_dispatcher::PortDispatcher;
use zircon_macros::syscall;
use zx_status::Status;
use zx_types::*;

#[syscall]
pub unsafe extern "C" fn zx_port_create(options: u32, out: *mut zx_handle_t) -> zx_status_t {
    let (handle, rights) = PortDispatcher::create(options)?;
    let value = handle.make_and_add_handle(rights)?;
    // SAFETY: `out` is the caller's.
    unsafe { copy_out(out, value) }
}

#[syscall]
pub unsafe extern "C" fn zx_port_queue(
    handle: zx_handle_t,
    packet: *const zx_port_packet_t,
) -> zx_status_t {
    let port =
        handle_table().get_dispatcher_with_rights::<PortDispatcher>(handle, ZX_RIGHT_WRITE)?;
    // SAFETY: A packet is plain data.
    let packet = unsafe { copy_in(packet) }?;
    port.queue_user(&packet)
}

#[syscall]
pub unsafe extern "C" fn zx_port_wait(
    handle: zx_handle_t,
    deadline: zx_instant_mono_t,
    packet: *mut zx_port_packet_t,
) -> zx_status_t {
    let port =
        handle_table().get_dispatcher_with_rights::<PortDispatcher>(handle, ZX_RIGHT_READ)?;
    let pp = port.dequeue(&Deadline::no_slack(deadline))?;
    // SAFETY: `packet` is the caller's.
    unsafe { copy_out(packet, pp) }
}

#[syscall]
pub unsafe extern "C" fn zx_port_cancel(
    handle: zx_handle_t,
    source: zx_handle_t,
    key: u64,
) -> zx_status_t {
    let port =
        handle_table().get_dispatcher_with_rights::<PortDispatcher>(handle, ZX_RIGHT_WRITE)?;
    handle_table().with_handle(source, |watched| {
        ensure!(watched.has_rights(ZX_RIGHT_WAIT), Status::ACCESS_DENIED);
        let dispatcher = watched.dispatcher();
        ensure!(dispatcher.is_waitable(), Status::NOT_SUPPORTED);
        let port_ptr: *const () = std::sync::Arc::as_ptr(&port).cast();
        let had_observer = dispatcher.base().cancel_by_key(watched.id(), port_ptr, key);
        let packet_removed = port.cancel_queued(watched.id(), key);
        if had_observer || packet_removed { Ok(()) } else { Err(Status::NOT_FOUND) }
    })?
}

#[syscall]
pub unsafe extern "C" fn zx_port_cancel_key(
    handle: zx_handle_t,
    options: u32,
    key: u64,
) -> zx_status_t {
    ensure!(options == 0, Status::INVALID_ARGS);
    let port =
        handle_table().get_dispatcher_with_rights::<PortDispatcher>(handle, ZX_RIGHT_WRITE)?;
    port.cancel_key(key)
}
