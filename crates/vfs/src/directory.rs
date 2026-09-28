// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module holding different kinds of pseudo directories and their building blocks.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/directory.rs`, with the building blocks of mutable
//! directories, their connections and `serve_on`, and without pseudo directories. `common`, `dirents_sink`,
//! `read_dirents`, `traversal_position` and `entry_container` (built without watchers by
//! scripts/kernel-cfg.py) are vendored; `connection`, `entry` and `mutable` are excerpts. There
//! are no watchers, whose connections are channels directories write events to.

use crate::ToObjectRequest;
use crate::directory::entry_container::Directory;
use crate::execution_scope::ExecutionScope;
use crate::path::Path;
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use std::sync::Arc;

#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/directory/common.rs"]
pub(crate) mod common;
mod connection;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/directory/dirents_sink.rs"]
pub mod dirents_sink;
pub mod entry;
#[path = "../../../vendor/fuchsia-kernel/src/storage/lib/vfs/rust/src/directory/entry_container.rs"]
pub mod entry_container;
pub mod mutable;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/directory/read_dirents.rs"]
pub mod read_dirents;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/directory/traversal_position.rs"]
pub mod traversal_position;

/// A directory can be open either as a directory or a node.
#[derive(Clone)]
pub struct DirectoryOptions {
    pub(crate) rights: fio::Operations,
}

impl From<&DirectoryOptions> for fio::Flags {
    fn from(options: &DirectoryOptions) -> Self {
        // There is 1:1 mapping between `fio::Operations` and `fio::Flags`.
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::from_bits_truncate(options.rights.bits())
    }
}

impl Default for DirectoryOptions {
    fn default() -> Self {
        DirectoryOptions { rights: fio::R_STAR_DIR }
    }
}

/// Helper function to serve a connection to `directory` on `server_end` with specified `flags` and
/// `scope`. Errors will be communicated via epitaph on `server_end`.
pub fn serve_on<D: Directory + ?Sized>(
    directory: Arc<D>,
    flags: fio::Flags,
    scope: ExecutionScope,
    server_end: ServerEnd<fio::DirectoryMarker>,
) {
    let request = flags.to_object_request(server_end);
    request.handle(|request| directory.open(scope, Path::dot(), flags, request));
}
