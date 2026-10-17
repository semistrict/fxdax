// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `std` for Rust in the Linux kernel (TASK-26.1 spike): what crates written for std, Fxfs's
//! dependencies among them, need of it, so that they build unchanged for the kernel's targets.
//!
//! It is `core` and `alloc` under std's paths, and the parts of std the kernel can back: locks,
//! the clocks, randomness, `io`'s traits, and hash maps. A sysroot holding it as `std` makes every
//! crate built against the sysroot use it. Where the kernel has nothing to back std with, such as
//! threads, files and the environment, it says so: spawning a thread or opening a file fails, the
//! environment is empty, and a backtrace is never captured.
//!
//! Panics abort, so nothing unwinds: locks are never poisoned and `thread::panicking` is false.
//! The locks spin, which a kernel's should not: milestone 1 backs them with the kernel's mutexes
//! and wait queues.

#![no_std]
#![allow(hidden_glob_reexports)]
// What std's own sources use (scripts/import-guest-std.py): hash maps, and float methods, which std
// adds to the primitives.
#![allow(internal_features)]
#![feature(
    const_default,
    const_trait_impl,
    core_float_math,
    core_intrinsics,
    extend_one,
    f128,
    rustc_attrs,
    try_reserve_kind,
)]

extern crate alloc as alloc_crate;

pub use core::*;

// What `alloc` has under std's paths, and its macros: `vec` is a module and a macro.
pub use alloc_crate::{borrow, boxed, format, rc, slice, str, string, vec};

pub mod alloc;
pub mod backtrace;
pub mod collections;
pub mod env;
pub mod f32;
pub mod f64;
pub mod ffi;
pub mod fmt;
pub mod fs;
pub mod hash;
pub mod io;
pub mod os;
pub mod panic;
pub mod path;
pub mod prelude;
pub mod process;
pub mod sync;
mod sys;
pub mod task;
pub mod thread;
pub mod time;

/// Prints to the kernel's log.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::io::_print(6, $crate::format_args!($($arg)*)) };
}

/// Prints a line to the kernel's log.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::io::_print(6, $crate::format_args!("{}\n", $crate::format_args!($($arg)*))) };
}

/// Prints to the kernel's log, as an error.
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::io::_print(3, $crate::format_args!($($arg)*)) };
}

/// Prints a line to the kernel's log, as an error.
#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => { $crate::io::_print(3, $crate::format_args!("{}\n", $crate::format_args!($($arg)*))) };
}

/// Prints an expression and its value to the kernel's log, and returns the value.
#[macro_export]
macro_rules! dbg {
    ($val:expr $(,)?) => {
        match $val {
            tmp => {
                $crate::eprintln!(
                    "[{}:{}] {} = {:#?}",
                    $crate::file!(),
                    $crate::line!(),
                    $crate::stringify!($val),
                    &tmp
                );
                tmp
            }
        }
    };
}
