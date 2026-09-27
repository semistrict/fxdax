//! Tests of the in-process Zircon kernel through the `zx` crate.
//!
//! The tests under `tests/` exercise the kernel as a Fuchsia program would,
//! through the syscalls. Where Zircon has core tests for a syscall
//! (`zircon/system/utest/core`), they are ported from those.
