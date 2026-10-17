// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Rust's panics panic the kernel, which cannot unwind.

use crate::ffi;
use crate::logger::Message;
use core::fmt::Write;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let mut message = Message::new();
    let _ = write!(message, "{info}");
    // SAFETY: Panics with the message's bytes.
    unsafe { ffi::fxfs_rt_panic(message.bytes().as_ptr().cast(), message.bytes().len()) }
}
