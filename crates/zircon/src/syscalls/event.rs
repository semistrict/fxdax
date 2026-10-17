// Copyright 2026 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Event syscalls, ported from `zircon/kernel/lib/syscalls/event.rs`.

use super::copy_out;
use crate::object::event_dispatcher::EventDispatcher;
use zircon_macros::syscall;
use zx_types::*;

#[syscall]
pub unsafe extern "C" fn zx_event_create(options: u32, out: *mut zx_handle_t) -> zx_status_t {
    let (handle, rights) = EventDispatcher::create(options)?;
    let value = handle.make_and_add_handle(rights)?;
    // SAFETY: `out` is the caller's.
    unsafe { copy_out(out, value) }
}
