// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! An in-process Zircon kernel.
//!
//! This ports the parts of the Zircon kernel that give Fuchsia its memory
//! semantics (pager-backed VMOs, dirty tracking, page queues and eviction)
//! so they can run inside a host process. The syscall interface is Zircon's
//! own: the functions under [`syscalls`] are exported with their C names and
//! the `zx` crate calls them through `zx-sys`.
//!
//! Each module notes the Zircon source it was ported from and where it
//! deviates.

pub mod arch;
pub mod boot_options;
mod debug;
#[cfg(test)]
mod fbl;
pub mod hypervisor;
mod init;
pub mod kernel;
pub mod object;
pub mod runtime;
mod sync;
pub mod syscalls;
#[cfg(test)]
mod unittest;
pub mod user_copy;
pub mod vm;
mod zr;

/// Returns `Err($error)` unless `$cond` holds: Zircon's `if (!cond) { return ZX_ERR_...; }`.
macro_rules! ensure {
    ($cond:expr, $error:expr $(,)?) => {
        if !$cond {
            return Err($error);
        }
    };
}
pub(crate) use ensure;
