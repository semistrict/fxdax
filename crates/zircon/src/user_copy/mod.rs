// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pointers to user memory, which are only accessed by copies that recover from faults.
//!
//! Ported from `zircon/kernel/lib/user_copy/src/mod.rs`.
//!
//! Deviations:
//! - No string views yet.
//! - `user_iovec` copies from `arch::host::user_copy` rather than `arch_rs`.
//! - Tests are in `tests`, rather than `tests/kernel.rs`.
//! - `user_ptr` adds the copies that capture faults of
//!   `zircon/kernel/lib/user_copy/include/lib/user_copy/user_ptr.h`.

#[cfg(test)]
mod tests;
mod user_iovec;
mod user_ptr;

#[allow(unused_imports)]
pub use user_iovec::{
    UserInIovec, UserInOutIovec, UserInOutVector, UserInVector, UserOutIovec, UserOutVector,
    make_user_in_iovec, make_user_out_iovec,
};

#[allow(unused_imports)]
pub use user_ptr::{UserInOutPtr, UserInPtr, UserOutPtr};
