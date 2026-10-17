// Copyright 2017 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The parts of Zircon's hypervisor support that serve guests' memory, ported from
//! `zircon/kernel/hypervisor`.

pub mod aspace;

#[cfg(test)]
mod hypervisor_unittest;
