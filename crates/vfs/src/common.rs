// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Code shared between several modules of the vfs library.
//!
//! A stand-in for `src/storage/lib/vfs/rust/src/common.rs`, with only `IntoAny`.

use std::sync::Arc;

/// Helper trait to allow casting into `Any` without requiring the underlying object implement the
/// trait without preventing it from being object safe (thus disallowing dynamic dispatch).
/// Since we provide a generic implementation, the size of each concrete type is known.
pub trait IntoAny: std::any::Any {
    /// Cast the given object into a `dyn std::any::Any`.
    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync + 'static>;
}

impl<T: 'static + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync + 'static> {
        self as Arc<dyn std::any::Any + Send + Sync + 'static>
    }
}
