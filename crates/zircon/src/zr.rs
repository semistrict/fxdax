// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Stand-ins for the kernel's `zr` support crate.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;

/// Storage for a value that is initialized and accessed through raw pointers,
/// as when it is owned by code on the other side of an FFI boundary.
pub struct Opaque<T>(UnsafeCell<MaybeUninit<T>>);

impl<T> Opaque<T> {
    pub const fn uninit() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
    }

    pub const fn get(&self) -> *mut T {
        UnsafeCell::raw_get(&self.0).cast()
    }
}

/// Asserts a condition at compile time.
macro_rules! static_assert {
    ($cond:expr $(,)?) => {
        const _: () = assert!($cond);
    };
}
pub(crate) use static_assert;
