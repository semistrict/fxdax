// Copyright 2026 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Basic kernel types.
//!
//! Ported from `zircon/kernel/kernel/types.rs`.

/// A physical address. Here physical memory is the host memory backing the
/// PMM, and a physical address is an offset into it; see `vm::physmap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PAddr(pub usize);
