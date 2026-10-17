// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel primitives the VM relies on: time, deadlines, events and the current thread's faults.
//!
//! Zircon implements these on its scheduler. Here they are built from host
//! threads, so a blocked kernel operation blocks the calling host thread, unless
//! it waits as a future (see `wakers`).

pub mod deadline;
pub mod event;
pub mod range_check;
pub mod thread;
pub mod timer;
pub mod types;
pub mod wakers;
