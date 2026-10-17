// Copyright 2026 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! System and time syscalls, ported from `zircon/kernel/lib/syscalls/system.rs`,
//! `zircon/kernel/lib/syscalls/cprng.rs`, `zircon/kernel/lib/syscalls/nanosleep.rs` and the
//! vDSO's time functions.
//!
//! Deviation: there are no jobs, and the process is the whole system, so `zx_system_get_event`
//! doesn't check that `root_job` is the root job.

use super::{copy_out, copy_out_slice};
use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::Event;
use crate::object::handle::{Handle, handle_table};
use crate::object::memory_watchdog::get_memory_watchdog;
use crate::vm::pmm;
use zircon_macros::syscall;
use zx_status::Status;
use zx_types::*;

const ZX_DEFAULT_SYSTEM_EVENT_LOW_MEMORY_RIGHTS: zx_rights_t =
    ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_WAIT;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_system_get_page_size() -> u32 {
    page::SIZE as u32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_system_get_num_cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_system_get_physmem() -> u64 {
    crate::vm::pmm::count_total_bytes()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_clock_get_monotonic() -> zx_instant_mono_t {
    current_mono_time()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_deadline_after(nanoseconds: zx_duration_mono_t) -> zx_instant_mono_t {
    current_mono_time().saturating_add(nanoseconds)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_nanosleep(deadline: zx_instant_mono_t) -> zx_status_t {
    // Nothing signals this event, so the wait lasts until the deadline.
    let _ = Event::new(false).wait(&Deadline::no_slack(deadline));
    ZX_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zx_cprng_draw(buffer: *mut u8, buffer_size: usize) {
    if buffer_size == 0 {
        return;
    }
    let mut filled = 0;
    while filled < buffer_size {
        // getentropy writes at most 256 bytes at a time.
        let mut chunk = [0u8; 256];
        let len = (buffer_size - filled).min(chunk.len());
        // SAFETY: `chunk` has room for `len` bytes.
        let result = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), len) };
        assert_eq!(result, 0, "getentropy: {}", std::io::Error::last_os_error());
        // SAFETY: `buffer` is the caller's.
        let copied = unsafe { copy_out_slice(buffer.wrapping_add(filled), &chunk[..len]) };
        // zx_cprng_draw cannot fail, so a bad buffer is fatal as it is to Zircon's vDSO.
        copied.expect("zx_cprng_draw buffer");
        filled += len;
    }
}

#[syscall]
pub unsafe extern "C" fn zx_system_get_event(
    _root_job: zx_handle_t,
    kind: u32,
    out: *mut zx_handle_t,
) -> zx_status_t {
    match kind {
        ZX_SYSTEM_EVENT_OUT_OF_MEMORY
        | ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL => {
            // The watchdog starts with the PMM.
            pmm::node();
            let event = get_memory_watchdog().get_mem_pressure_event(kind).unwrap();
            // Do not grant default event rights, as we don't want userspace to, for
            // example, be able to signal this event.
            let value = handle_table()
                .add_handle(Handle::make(event, ZX_DEFAULT_SYSTEM_EVENT_LOW_MEMORY_RIGHTS))?;
            // SAFETY: `out` is the caller's.
            unsafe { copy_out(out, value) }
        }
        _ => Err(Status::INVALID_ARGS),
    }
}
