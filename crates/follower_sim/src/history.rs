// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The history checker (backlog doc-8, testing 4; `follower_history`) of the simulation's files:
//! the writer's few files, by name, each its blocks' fill.

use crate::model::Contents;

pub type History = follower_history::History<&'static str, Contents>;
pub type Read = follower_history::Read<&'static str, Contents>;
pub type HeldRead = follower_history::HeldRead<Contents>;
