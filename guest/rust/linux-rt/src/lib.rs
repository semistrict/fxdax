//! A runtime for Rust in the Linux kernel, without std, over the C functions of
//! guest/linux/fs/fxfs/rt.h:
//!
//! - the global allocator, over kvmalloc;
//! - a logger for the `log` crate, over printk;
//! - the panic handler, which panics the kernel;
//! - an executor: `block_on` sleeps the calling thread until a future is done, `spawn` runs
//!   futures on kernel workers, and `Timer` wakes them after a while;
//! - randomness, from the kernel's generator.
//!
//! The kernel's threads have small stacks (64 KiB with fxdax's patch, guest/linux/
//! task-stack-size.patch, rather than 16), and Fxfs's futures are large, so futures are polled
//! from the heap.

#![no_std]

extern crate alloc;

mod allocator;
mod executor;
mod ffi;
mod logger;
mod panic;
mod random;
mod time;

pub use executor::{Task, block_on, spawn, spawn_urgent};
pub use logger::init_logger;
pub use random::fill_random;
pub use time::{Timer, monotonic};
