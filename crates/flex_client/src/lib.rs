// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An excerpt of `src/lib/fdomain/client/src/flex.rs`, FDomain's `flex_fidl` crate, which lets
//! code build with either FIDL or FDomain: without FDomain, as here, it is FIDL's items and
//! endpoints.

pub use ::fidl::*;

pub mod fidl {
    pub use ::fidl::endpoints::*;
}
