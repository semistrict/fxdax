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
