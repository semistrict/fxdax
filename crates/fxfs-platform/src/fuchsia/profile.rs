// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The `Recorder` trait of `src/storage/fxfs/platform/src/fuchsia/profile.rs`, through which the
// pager records page-in requests. Profile recording and replay are not ported.

use crate::fuchsia::node::FxNode;
use anyhow::Error;
use std::sync::Arc;

/// Takes messages to be written into the current profile. This should be dropped before the
/// recording is stopped to ensure that all messages have been flushed to the writer thread.
pub trait Recorder: Send + Sync {
    /// Record a page in request, for the given identifier and offset.
    fn record(&mut self, node: Arc<dyn FxNode>, offset: u64) -> Result<(), Error>;

    /// Record file opens to gather what files were actually used during the recording.
    fn record_open(&mut self, node: Arc<dyn FxNode>) -> Result<(), Error>;
}
