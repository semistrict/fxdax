//! The C functions of guest/linux/fs/fxfs/rt.h.

use core::ffi::{c_char, c_int, c_void};

/// A thread's way to sleep until it is woken.
#[repr(C)]
pub struct Parker {
    _private: [u8; 0],
}

/// A one-shot timer.
#[repr(C)]
pub struct Timer {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub fn fxfs_rt_alloc(size: usize, align: usize) -> *mut c_void;
    pub fn fxfs_rt_free(ptr: *mut c_void);

    pub fn fxfs_rt_log(level: c_int, msg: *const c_char, len: usize);
    pub fn fxfs_rt_panic(msg: *const c_char, len: usize) -> !;

    pub fn fxfs_rt_parker_new() -> *mut Parker;
    pub fn fxfs_rt_parker_free(parker: *mut Parker);
    pub fn fxfs_rt_park(parker: *mut Parker);
    pub fn fxfs_rt_unpark(parker: *mut Parker);

    pub fn fxfs_rt_schedule(run: extern "C" fn(arg: *mut c_void), arg: *mut c_void) -> c_int;
    pub fn fxfs_rt_schedule_urgent(run: extern "C" fn(arg: *mut c_void), arg: *mut c_void)
    -> c_int;

    pub fn fxfs_rt_timer_new(fire: extern "C" fn(arg: *mut c_void), arg: *mut c_void)
    -> *mut Timer;
    pub fn fxfs_rt_timer_start(timer: *mut Timer, ns: u64);
    pub fn fxfs_rt_timer_free(timer: *mut Timer);

    pub fn fxfs_rt_monotonic_ns() -> u64;

    pub fn fxfs_rt_random_bytes(buf: *mut c_void, len: usize);
}
