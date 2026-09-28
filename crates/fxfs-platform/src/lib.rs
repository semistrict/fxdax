// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs's pager-backed files on the in-process kernel: the part of Fxfs's Fuchsia platform
//! (`src/storage/fxfs/platform/src/lib.rs`) that keeps file contents in pager VMOs and writes them
//! back, without fuchsia.io serving.

pub mod fuchsia;

pub use self::fuchsia::*;
