//! Zircon syscall declarations, resolved on the host by the in-process kernel in the `zircon`
//! crate.
//!
//! `definitions.rs` is Fuchsia's `sdk/rust/zx-sys/src/definitions.rs`, which zither generates from
//! the zx FIDL library, with its `extern "C"` block declared on every target.

// The syscalls are defined there; naming the crate keeps it linked.
extern crate zircon;

mod definitions;

pub use definitions::*;
