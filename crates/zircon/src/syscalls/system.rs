//! System and time syscalls, ported from `zircon/kernel/lib/syscalls/system.rs`,
//! `zircon/kernel/lib/syscalls/cprng.rs`, `zircon/kernel/lib/syscalls/nanosleep.rs` and the
//! vDSO's time functions.

use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::Event;
use zx_types::*;

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
    // SAFETY: `buffer` has `buffer_size` bytes.
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_size) };
    let mut filled = 0;
    while filled < buffer.len() {
        // SAFETY: getentropy writes at most 256 bytes to the given buffer.
        let chunk = (buffer.len() - filled).min(256);
        let result = unsafe { libc::getentropy(buffer[filled..].as_mut_ptr().cast(), chunk) };
        assert_eq!(result, 0, "getentropy: {}", std::io::Error::last_os_error());
        filled += chunk;
    }
}
