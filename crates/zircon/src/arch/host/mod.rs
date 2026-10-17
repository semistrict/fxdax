// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Address spaces mapped into the host process, standing in for an architecture's MMU and
//! exception handling.

pub mod exceptions;
pub mod mmu;
pub mod user_copy;
