//! What the kernel gives std (guest/linux/fs/fxfs/rt.h).

use core::ffi::{c_char, c_int};

unsafe extern "C" {
    pub fn fxfs_rt_log(level: c_int, msg: *const c_char, len: usize);
    pub fn fxfs_rt_monotonic_ns() -> u64;
    pub fn fxfs_rt_realtime_ns() -> u64;
    pub fn fxfs_rt_random_bytes(buf: *mut u8, len: usize);
}

/// Random bytes, from the kernel's generator.
pub fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    // SAFETY: Fills `bytes`.
    unsafe { fxfs_rt_random_bytes(bytes.as_mut_ptr(), bytes.len()) };
    u64::from_ne_bytes(bytes)
}
