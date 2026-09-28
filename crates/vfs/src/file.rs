// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module holding different kinds of files and their building blocks.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/file.rs`: the `File` trait, the options it takes
//! and stream connections (served in-process, see `connection`), without fsverity or other kinds
//! of connections.

use crate::node::Node;
use fidl_fuchsia_io as fio;
use std::future::{Future, ready};
use zx_status::Status;

pub mod connection;

pub use connection::{GetVmo, StreamIoConnection};

/// FileOptions include options that are relevant after the file has been opened. Flags like
/// `TRUNCATE`, which only applies during open time, are not included.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct FileOptions {
    pub rights: fio::Operations,
    pub is_append: bool,
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub is_linkable: bool,
}

impl FileOptions {
    /// Converts to `StreamOptions`.
    pub fn to_stream_options(&self) -> zx::StreamOptions {
        let mut options = zx::StreamOptions::empty();
        if self.rights.contains(fio::Operations::READ_BYTES) {
            options |= zx::StreamOptions::MODE_READ;
        }
        if self.rights.contains(fio::Operations::WRITE_BYTES) {
            options |= zx::StreamOptions::MODE_WRITE;
        }
        if self.is_append {
            options |= zx::StreamOptions::MODE_APPEND;
        }
        options
    }
}

#[derive(Default, PartialEq)]
pub enum SyncMode {
    /// Used when the Sync fuchsia.io method is used.
    #[default]
    Normal,

    /// Used when the connection is about to be closed. Typically this will involve flushing data
    /// from caches, but performance is a consideration, so it should only perform what might be
    /// necessary for closing the file. If anything *must* happen when a file is closed, it must be
    /// implemented in the `Node::close` function, not here; a call to sync with this mode is not
    /// guaranteed and not implementing/supporting it should have no effect on correctness. If
    /// `Node::close` needs to flush data in an async context, it has to spawn a task.  Supporting
    /// this mode means that in most cases there's no need to spawn a task because there should be
    /// nothing that needs to be flushed (but it must check). This will only be called if the
    /// connection has write permissions; a connection that only has read permissions should not
    /// have made any changes that need flushing.
    PreClose,
}

/// Trait used for all files.
pub trait File: Node {
    /// Capabilities:
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        false
    }
    fn executable(&self) -> bool {
        false
    }

    /// Called when the file is going to be accessed, typically by a new connection.
    /// Flags is the same as the flags passed to `flex_fuchsia_io.Node/Open`.
    /// The following flags are handled by the connection and do not need to be handled inside
    /// open():
    /// * OPEN_FLAG_TRUNCATE - A call to truncate() will be made immediately after open().
    /// * OPEN_FLAG_DESCRIBE - The OnOpen event is sent before any other requests are received from
    /// the file's client.
    fn open_file(&self, options: &FileOptions) -> impl Future<Output = Result<(), Status>> + Send;

    /// Truncate the file to `length`.
    /// If there are pending attributes to update (see `update_attributes`), they should also be
    /// flushed at this time. Otherwise, no attributes should be updated, other than size as needed.
    fn truncate(&self, length: u64) -> impl Future<Output = Result<(), Status>> + Send;

    /// Get the size of this file.
    /// This is used to calculate seek offset relative to the end.
    fn get_size(&self) -> impl Future<Output = Result<u64, Status>> + Send;

    /// Set the mutable attributes of this file based on the values in `attributes`. If the file
    /// does not support updating *all* of the specified attributes, implementations should fail
    /// with `ZX_ERR_NOT_SUPPORTED`.
    fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> impl Future<Output = Result<(), Status>> + Send;

    /// Get a VMO representing this file.
    /// If not supported by the underlying filesystem, should return Err(NOT_SUPPORTED).
    fn get_backing_memory(
        &self,
        _flags: fio::VmoFlags,
    ) -> impl Future<Output = Result<zx::Vmo, Status>> + Send {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Preallocate disk space for this range.
    fn allocate(
        &self,
        _offset: u64,
        _length: u64,
        _mode: fio::AllocateMode,
    ) -> impl Future<Output = Result<(), Status>> + Send {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Sync this file's contents to the storage medium (probably disk).
    /// This does not necessarily guarantee that the file will be completely written to disk once
    /// the call returns. It merely guarantees that any changes to the file have been propagated
    /// to the next layer in the storage stack.
    fn sync(&self, mode: SyncMode) -> impl Future<Output = Result<(), Status>> + Send;
}
