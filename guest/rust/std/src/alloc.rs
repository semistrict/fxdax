// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Memory allocation: `core`'s and `alloc`'s.

pub use alloc_crate::alloc::{Global, alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc};
pub use core::alloc::*;
