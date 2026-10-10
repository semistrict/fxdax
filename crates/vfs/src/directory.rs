// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module holding different kinds of pseudo directories and their building blocks.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/directory.rs`, with the building blocks of mutable
//! directories, their connections and `serve_on`, and without pseudo directories. `common`,
//! `dirents_sink`, `read_dirents`, `traversal_position` and `entry_container` (without watchers)
//! are copied; `connection`, `entry` and `mutable` are excerpts. There are no watchers, whose
//! connections are channels directories write events to.

use crate::ToObjectRequest;
use crate::directory::entry_container::Directory;
use crate::execution_scope::ExecutionScope;
use crate::path::Path;
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use std::sync::Arc;

pub(crate) mod common;
mod connection;
pub mod dirents_sink;
pub mod entry;
pub mod entry_container;
pub mod mutable;
pub mod read_dirents;
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
