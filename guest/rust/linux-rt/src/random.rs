// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Randomness, from the kernel's generator.

use crate::ffi;

/// Fills `buf` with random bytes.
pub fn fill_random(buf: &mut [u8]) {
    // SAFETY: `buf` is writable for its length.
    unsafe { ffi::fxfs_rt_random_bytes(buf.as_mut_ptr().cast(), buf.len()) }
}
