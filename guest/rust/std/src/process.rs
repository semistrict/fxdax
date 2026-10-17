// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Processes, of which the kernel has none to run or exit.

/// Panics the kernel.
pub fn abort() -> ! {
    panic!("abort")
}

/// Panics the kernel, which a filesystem cannot exit.
pub fn exit(code: i32) -> ! {
    panic!("exit({code})")
}
