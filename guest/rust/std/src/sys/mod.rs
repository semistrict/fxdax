//! What the kernel gives std (guest/linux/fs/fxfs/rt.h), and libm's functions (cmath).

// libm's, which the kernel's link needs only for what is called.
#[allow(dead_code)]
pub mod cmath;

use core::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    pub fn fxfs_rt_log(level: c_int, msg: *const c_char, len: usize);
    pub fn fxfs_rt_monotonic_ns() -> u64;
    pub fn fxfs_rt_realtime_ns() -> u64;
    pub fn fxfs_rt_random_bytes(buf: *mut u8, len: usize);
    /// The current task's slot for its thread-local values, or null in an interrupt.
    pub fn fxfs_rt_tls() -> *mut *mut c_void;
}

/// Random bytes, from the kernel's generator.
pub fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    // SAFETY: Fills `bytes`.
    unsafe { fxfs_rt_random_bytes(bytes.as_mut_ptr(), bytes.len()) };
    u64::from_ne_bytes(bytes)
}
