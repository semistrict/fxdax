//! Zircon syscall declarations, resolved on the host by the in-process
//! kernel in the `zircon` crate. See `build.rs`.

// The syscalls are defined there; naming the crate keeps it linked.
extern crate zircon;

include!(concat!(env!("OUT_DIR"), "/definitions.rs"));
