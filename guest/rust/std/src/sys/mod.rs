// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the kernel gives std (guest/linux/fs/fxfs/rt.h), and libm's functions (cmath).

// libm's, which the kernel's link needs only for what is called.
#[allow(dead_code)]
pub mod cmath;
pub mod sync;

use core::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    pub fn fxfs_rt_log(level: c_int, msg: *const c_char, len: usize);
    pub fn fxfs_rt_monotonic_ns() -> u64;
    pub fn fxfs_rt_realtime_ns() -> u64;
    pub fn fxfs_rt_random_bytes(buf: *mut u8, len: usize);
    /// Lets other tasks run on this CPU, if the scheduler wants it to.
    pub fn fxfs_rt_yield();
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

/// Waits a moment, spinning, and lets other tasks run if the scheduler wants it to: the kernel
/// preempts no task, so a wait that only spun could keep the task it waits for from running.
pub fn relax() {
    core::hint::spin_loop();
    // SAFETY: Called from a task's context, as Rust's code runs in.
    unsafe { fxfs_rt_yield() }
}
