// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Zircon syscall declarations, resolved on the host by the in-process kernel in the `zircon`
//! crate.
//!
//! `definitions.rs` is Fuchsia's `sdk/rust/zx-sys/src/definitions.rs`, which zither generates from
//! the zx FIDL library, with its `extern "C"` block declared on every target.

// The syscalls are defined there; naming the crate keeps it linked. The guest kernel has none.
#[cfg(not(target_os = "none"))]
extern crate zircon;

mod definitions;

pub use definitions::*;
