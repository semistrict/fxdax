// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: the deterministic simulation of live read-only followers (backlog doc-8, testing
//! 3; TASK-42.1). The real Fxfs writes a filesystem to an extent store, and followers mount it
//! read-only through the store's views, while readers read it, all under Shuttle's scheduler
//! (awslabs/shuttle), which chooses every interleaving of their tasks and replays a failing one.
//! Its tests need the `shuttle` feature, which puts Fxfs's executor, tasks, timers and locks, and
//! the store's, on Shuttle's.

pub mod device;
#[cfg(feature = "shuttle")]
pub mod history;
#[cfg(feature = "shuttle")]
pub mod model;
#[cfg(feature = "shuttle")]
pub mod sim;

#[cfg(all(test, feature = "shuttle"))]
mod failure;
#[cfg(all(test, feature = "shuttle"))]
mod graveyard;
#[cfg(all(test, feature = "shuttle"))]
mod probe;
