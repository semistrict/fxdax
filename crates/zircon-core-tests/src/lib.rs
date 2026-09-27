//! Tests of the in-process Zircon kernel through the `zx` crate.
//!
//! The tests under `tests/` exercise the kernel as a Fuchsia program would,
//! through the syscalls. Where Zircon has core tests for a syscall
//! (`zircon/system/utest/core`), they are ported from those, with their
//! helpers here.

#![allow(non_camel_case_types)]

pub mod test_thread;
pub mod userpager;
