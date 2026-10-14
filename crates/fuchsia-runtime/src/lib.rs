// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Fuchsia-specific process runtime functions.
//!
//! A stand-in for `src/lib/fuchsia-runtime/src/lib.rs`, with the functions whose runtime the
//! in-process kernel provides (`zircon::runtime`).

use zx::sys::zx_handle_t;
use zx::{Unowned, Vmar};

unsafe extern "C" {
    pub fn zx_vmar_root_self() -> zx_handle_t;
}

// Link the kernel, whose runtime defines the functions above. The guest kernel has none.
#[cfg(not(target_os = "none"))]
use zircon as _;

/// Get a reference to the handle of the current address space.
pub fn vmar_root_self() -> Unowned<'static, Vmar> {
    unsafe {
        let handle = zx_vmar_root_self();
        Unowned::from_raw_handle(handle)
    }
}
