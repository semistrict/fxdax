// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A stand-in for Fuchsia's FIDL runtime crate (`src/lib/fidl/rust/fidl`).
//!
//! Only the items the vendored parts of other libraries use are here. Handles are the in-process
//! kernel's, as they are on Fuchsia, rather than the emulated handles upstream uses on hosts.

/// As `fuchsia_handles` in `src/lib/fidl/rust/fidl/src/handle.rs` aliases the zircon library.
pub use zx::{
    AsHandleRef, Handle, HandleDisposition, HandleInfo, HandleOp, HandleRef, Koid, MessageBufEtc,
    NullableHandle, ObjectType, Peered, Rights, Signals, Status,
};

/// From `src/lib/fidl/rust/fidl/src/endpoints.rs`.
pub mod endpoints {
    /// The fake "client" produced by `ProxyHasDomain`. Analogous to an FDomain client.
    pub struct ZirconClient;
}
