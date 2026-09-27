//! System and time syscalls, ported from `zircon/kernel/lib/syscalls/system.rs`,
//! `zircon/kernel/lib/syscalls/cprng.rs`, `zircon/kernel/lib/syscalls/nanosleep.rs` and the
//! vDSO's time functions.

use super::copy_out_slice;
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
