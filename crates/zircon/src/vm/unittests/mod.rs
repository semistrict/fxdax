// Copyright 2020 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Unit tests ported from `zircon/kernel/vm/unittests`, and of what the port adds to Zircon.

mod arena_unittest;
mod aspace_unittest;
mod evictor_unittest;
mod mapping_unittest;
mod memory_out_unittest;
mod range_change_unittest;
mod shared_pages_unittest;
#[cfg(feature = "shuttle")]
mod shuttle_tests;
mod test_helper;
mod vmpl_unittest;
