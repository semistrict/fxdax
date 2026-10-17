// Copyright 2026 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Copied from `zircon/kernel/vm/fault.rs`, with the values its bindings take from
//! `zircon/kernel/vm/include/vm/fault.h`.

/// page fault flags
pub mod flag {
    pub const WRITE: u32 = 1 << 0;
    /// Indicates the fault occurred while the CPU was executing in user mode.
    pub const USER: u32 = 1 << 1;
    pub const GUEST: u32 = 1 << 2;
    pub const INSTRUCTION: u32 = 1 << 3;
    pub const NOT_PRESENT: u32 = 1 << 4;
    /// hardware is requesting a fault
    pub const HW_FAULT: u32 = 1 << 5;
    /// software fault
    pub const SW_FAULT: u32 = 1 << 6;
    pub const ACCESS: u32 = 1 << 7;
    pub const FAULT_MASK: u32 = HW_FAULT | SW_FAULT;
}
