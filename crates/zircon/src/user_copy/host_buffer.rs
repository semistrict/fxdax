// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Buffers of host memory that the kernel's futures copy into and out of.
//!
//! Not ported: a deviation of the port's, for the futures that stand in for syscalls (see
//! `kernel::wakers`). A buffer is an address and length rather than a reference, as the memory may
//! be shared with something that changes it concurrently, such as a guest; it is only accessed
//! through user copies, as user memory is, so no reference to it is ever formed.

/// `len` bytes of host memory at `addr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostBuffer {
    pub addr: usize,
    pub len: usize,
}
