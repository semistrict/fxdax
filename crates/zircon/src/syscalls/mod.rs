// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The Zircon syscall interface.
//!
//! Ported from Zircon's `kernel/lib/syscalls`. Each syscall is exported under its C name
//! (`#[syscall]`, which makes its body's `Result` its status) so that `zx-sys`'s declarations
//! resolve to it; `build.rs` checks their types against the
//! declarations and generates the syscalls not defined here to return `ZX_ERR_NOT_SUPPORTED`.
//!
//! Deviation: user pointers are pointers into the calling process, which is the kernel's own.
//! They are only accessed by user copies (`arch::host::user_copy`), which resolve faults on the
//! address spaces and fail with `ZX_ERR_INVALID_ARGS` on any other fault, so that a null pointer
//! where a result is required fails as it would on Zircon.

// Each syscall's pointers must be valid as its documentation in the Zircon syscall reference
// requires, as for any Zircon syscall.
#![allow(clippy::missing_safety_doc)]

mod event;
mod handle_ops;
mod object;
mod pager;
mod port;
mod stream;
mod system;
mod vmar;
mod vmo;

pub use event::*;
pub use handle_ops::*;
pub use object::*;
pub use pager::*;
pub use port::*;
pub use stream::*;
pub use system::*;
pub use vmar::*;
pub use vmo::*;

use crate::arch::host::user_copy::{arch_copy_from_user, arch_copy_to_user};
use std::mem::MaybeUninit;
use zx_status::Status;
use zx_types::zx_status_t;

/// Syscalls this kernel does not provide.
#[allow(unused_variables)]
mod unsupported {
    use zx_types::*;

    include!(concat!(env!("OUT_DIR"), "/unsupported.rs"));
}

/// Checks that the syscalls defined here have the types `zx-sys` declares.
#[allow(dead_code)]
mod declarations {
    use zx_types::*;

    include!(concat!(env!("OUT_DIR"), "/declarations.rs"));
}

/// Called by unsupported syscalls that cannot return a status.
fn unsupported(name: &str) -> ! {
    panic!("{name} is not supported by the in-process kernel");
}

/// Converts a syscall's result to its status.
fn status(result: Result<(), Status>) -> zx_status_t {
    match result {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

/// Writes a syscall's output, as `user_out_ptr<T>::copy_to_user` does.
///
/// # Safety
///
/// `ptr` must not point at memory the kernel owns.
unsafe fn copy_out<T: Copy>(ptr: *mut T, value: T) -> Result<(), Status> {
    // SAFETY: Guaranteed by the caller.
    unsafe { copy_out_slice(ptr, std::slice::from_ref(&value)) }
}

/// Writes a syscall's optional output, if it was asked for.
///
/// # Safety
///
/// `ptr` must not point at memory the kernel owns.
unsafe fn copy_out_optional<T: Copy>(ptr: *mut T, value: T) -> Result<(), Status> {
    if ptr.is_null() {
        return Ok(());
    }
    // SAFETY: Guaranteed by the caller.
    unsafe { copy_out(ptr, value) }
}

/// Writes a syscall's array output, as `user_out_ptr<T>::copy_array_to_user` does.
///
/// # Safety
///
/// `ptr` must not point at memory the kernel owns.
unsafe fn copy_out_slice<T: Copy>(ptr: *mut T, values: &[T]) -> Result<(), Status> {
    // SAFETY: `values` is kernel memory, and the user memory is guaranteed by the caller.
    unsafe { arch_copy_to_user(ptr.cast(), values.as_ptr().cast(), size_of_val(values)) }
}

/// Reads a syscall's input, as `user_in_ptr<T>::copy_from_user` does.
///
/// # Safety
///
/// Any bytes must be a valid `T`.
unsafe fn copy_in<T: Copy>(ptr: *const T) -> Result<T, Status> {
    let mut value = MaybeUninit::<T>::uninit();
    // SAFETY: `value` is kernel memory for a `T`.
    unsafe {
        arch_copy_from_user(value.as_mut_ptr().cast(), ptr.cast(), size_of::<T>())?;
        Ok(value.assume_init())
    }
}

/// Reads a syscall's array input, as `user_in_ptr<T>::copy_array_from_user` does.
///
/// # Safety
///
/// Any bytes must be a valid `T`.
unsafe fn copy_in_vec<T: Copy>(ptr: *const T, count: usize) -> Result<Vec<T>, Status> {
    let len = count.checked_mul(size_of::<T>()).ok_or(Status::INVALID_ARGS)?;
    let mut values = Vec::<T>::with_capacity(count);
    // SAFETY: `values` has room for `count` elements, which are initialized by the copy.
    unsafe {
        arch_copy_from_user(values.as_mut_ptr().cast(), ptr.cast(), len)?;
        values.set_len(count);
    }
    Ok(values)
}
