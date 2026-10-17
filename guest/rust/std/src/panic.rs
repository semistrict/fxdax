// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Panics, which abort: nothing unwinds, so there is nothing to catch.

use alloc_crate::boxed::Box;
use core::any::Any;
pub use core::panic::*;

/// Runs `f`, whose panics abort.
pub fn catch_unwind<F: FnOnce() -> R + UnwindSafe, R>(f: F) -> Result<R, Box<dyn Any + Send>> {
    Ok(f())
}

/// Panics with `payload`'s message, if it has one.
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    match payload.downcast_ref::<&str>() {
        Some(message) => panic!("{message}"),
        None => panic!("resumed panic"),
    }
}

/// Panics, with `payload` if it is a string.
pub fn panic_any<M: 'static + Any + Send>(payload: M) -> ! {
    resume_unwind(Box::new(payload))
}
