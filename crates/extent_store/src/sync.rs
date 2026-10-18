// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store's locks and condition variables: std's, or with the `shuttle` feature Shuttle's
//! (awslabs/shuttle), whose scheduler then chooses every interleaving of the threads using a store,
//! and replays a failing one from its seed (tests/shuttle.rs, TASK-44.4). Arcs are std's either
//! way.
//!
//! And the rules the store's concurrency relies on, which those tests turn off one at a time to
//! see a test fail: a test that never fails proves nothing.

#[cfg(feature = "shuttle")]
pub(crate) use shuttle::sync::{Condvar, Mutex, MutexGuard};
#[cfg(not(feature = "shuttle"))]
pub(crate) use std::sync::{Condvar, Mutex, MutexGuard};

/// The rules the store's concurrency relies on, each of which its Shuttle tests show to be needed
/// by turning it off.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// A free keeps the extent's data for the uploader until it releases the commit that freed it.
    pub keep_frees: bool,
    /// Closing the open segment between commits waits for the writes in it to finish.
    pub close_waits_for_writers: bool,
    /// The uploader is given commits and closed segments only once they are durable.
    pub durable_only: bool,
}

impl Rules {
    pub const ALL: Self =
        Self { keep_frees: true, close_waits_for_writers: true, durable_only: true };
}

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(not(feature = "shuttle"))]
pub(crate) fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(feature = "shuttle")]
pub fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules in force, for the Shuttle tests that turn one off.
#[cfg(feature = "shuttle")]
pub fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}
