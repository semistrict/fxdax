// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fuchsia-async, with the Fuchsia executor's packet receivers for the in-process kernel's ports.
//!
//! On hosts fuchsia-async runs its portable executor, which has no port, so code that receives
//! packets from the kernel, such as Fxfs's pager, cannot use it as it is. Crates with such code
//! depend on this crate under the name `fuchsia-async` (a Cargo `package` rename), and get
//! everything fuchsia-async has, plus the packet receiving API of its Fuchsia executor.

pub use fuchsia_async::*;

mod packets;

pub use packets::{EHandle, PacketReceiver, ReceiverRegistration};
