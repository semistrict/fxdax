// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/lib/fuchsia-fs/src/node.rs`: the error cloning a connection can have.

use thiserror::Error;

/// An error encountered while cloning a node
#[derive(Debug, Clone, Error)]
#[allow(missing_docs)]
pub enum CloneError {
    #[error("while making a fidl proxy: {0}")]
    CreateProxy(#[source] fidl::Error),

    #[error("while sending clone request: {0}")]
    SendCloneRequest(#[source] fidl::Error),
}
