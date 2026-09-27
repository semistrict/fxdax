//! An in-process Zircon kernel.
//!
//! This ports the parts of the Zircon kernel that give Fuchsia its memory
//! semantics (pager-backed VMOs, dirty tracking, page queues and eviction)
//! so they can run inside a host process. The syscall interface is Zircon's
//! own: the functions under [`syscalls`] are exported with their C names and
//! the vendored `zx` crate calls them through `zx-sys`.
//!
//! Each module notes the Zircon source it was ported from and where it
//! deviates.

pub mod boot_options;
mod debug;
#[cfg(test)]
mod fbl;
pub mod kernel;
pub mod object;
pub mod syscalls;
#[cfg(test)]
mod unittest;
pub mod vm;
mod zr;
