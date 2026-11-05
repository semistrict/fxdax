// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! guest/sim tests dax.c against a model of Fxfs, guest/sim/fxfs/fxfs.c, a second writing of what
//! the window's protocol assumes of it: where the model differs from Fxfs, a run there proves
//! nothing, or fails a correct dax.c. This runs the model beside Fxfs (fxfs-posix) on the same
//! operations, and compares what each file comes to (`diff_tests`), so that a drift shows as it is
//! made, not as a sim failure hours of reducing later (TASK-149.29).

pub mod model;

#[cfg(test)]
mod diff_tests;
