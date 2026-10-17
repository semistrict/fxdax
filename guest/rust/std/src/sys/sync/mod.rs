// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! std's locks over futexes (std's own, which scripts/import-guest-std.py imports), whose waits
//! sleep on the kernel's wait queues.

pub mod futex;
// std's, which allows unsafe operations in unsafe functions throughout.
#[allow(unsafe_op_in_unsafe_fn)]
mod condvar;
mod mutex;
// std's, whose downgrading the guest's std doesn't offer.
#[allow(dead_code)]
mod rwlock;

pub use condvar::Condvar;
pub use mutex::Mutex;
pub use rwlock::RwLock;
