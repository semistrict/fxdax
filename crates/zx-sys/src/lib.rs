// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Zircon syscall declarations, and their types.
//!
//! `definitions.rs` is Fuchsia's `sdk/rust/zx-sys/src/definitions.rs`, which zither generates from
//! the zx FIDL library, with its `extern "C"` block declared on every target. fxdax defines none of
//! the syscalls: neither Fxfs in the guest kernel nor Fxfs on the host calls any (TASK-96.1), and a
//! build that did would fail to link.

mod definitions;

pub use definitions::*;
