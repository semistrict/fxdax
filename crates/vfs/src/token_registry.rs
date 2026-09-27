// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A stand-in for `src/storage/lib/vfs/rust/src/token_registry.rs`.
//!
//! Upstream's registry maps FIDL tokens to the directories that minted them, for rename and link.
//! There are no FIDL connections here, so the registry an execution scope holds is empty.

/// The registry of tokens an execution scope holds.
pub struct TokenRegistry(());

impl TokenRegistry {
    pub fn new() -> Self {
        Self(())
    }
}
