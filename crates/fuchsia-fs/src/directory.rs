// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/lib/fuchsia-fs/src/directory.rs`: cloning directory connections, reading
// their entries and parsing `ReadDirents` buffers, copied verbatim. There are no namespaces,
// watchers, opening helpers or recursive operations.

use crate::node::CloneError;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use std::str::Utf8Error;
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, KnownLayout, Ref, Unaligned};

/// Error returned by readdir.
#[derive(Debug, Clone, Error)]
pub enum EnumerateError {
    #[error("a directory entry could not be decoded: {:?}", _0)]
    DecodeDirent(DecodeDirentError),

    #[error("fidl error during {}: {:?}", _0, _1)]
    Fidl(&'static str, fidl::Error),

    #[error("`read_dirents` failed with status {:?}", _0)]
    ReadDirents(zx_status::Status),

    #[error("`rewind` failed with status {:?}", _0)]
    Rewind(zx_status::Status),
}

/// An error encountered while decoding a single directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeDirentError {
    #[error("an entry extended past the end of the buffer")]
    BufferOverrun,

    #[error("name is not valid utf-8: {}", _0)]
    InvalidUtf8(Utf8Error),
}

/// Opens a new connection to the given `directory`. The cloned connection has the same permissions.
pub fn clone(dir: &fio::DirectoryProxy) -> Result<fio::DirectoryProxy, CloneError> {
    let (client_end, server_end) = create_proxy::<fio::DirectoryMarker>();
    dir.clone(server_end.into_channel().into()).map_err(CloneError::SendCloneRequest)?;
    Ok(client_end)
}

pub use fio::DirentType as DirentKind;

/// A directory entry.
#[derive(Clone, Eq, Ord, PartialOrd, PartialEq, Debug)]
pub struct DirEntry {
    /// The name of this node.
    pub name: String,

    /// The type of this node, or [`DirentKind::Unknown`] if not known.
    pub kind: DirentKind,
}

async fn readdir_inner(
    dir: &fio::DirectoryProxy,
    include_dot: bool,
) -> Result<Vec<DirEntry>, EnumerateError> {
    let status = dir.rewind().await.map_err(|e| EnumerateError::Fidl("rewind", e))?;
    zx_status::Status::ok(status).map_err(EnumerateError::Rewind)?;

    let mut entries = vec![];

    loop {
        let (status, buf) = dir
            .read_dirents(fio::MAX_BUF)
            .await
            .map_err(|e| EnumerateError::Fidl("read_dirents", e))?;
        zx_status::Status::ok(status).map_err(EnumerateError::ReadDirents)?;

        if buf.is_empty() {
            break;
        }

        for entry in parse_dir_entries(&buf) {
            let entry = entry.map_err(EnumerateError::DecodeDirent)?;
            if include_dot || entry.name != "." {
                entries.push(entry);
            }
        }
    }

    entries.sort_unstable();

    Ok(entries)
}

/// Returns a sorted Vec of directory entries contained directly in the given directory proxy. The
/// returned entries will not include "." or nodes from any subdirectories.
pub async fn readdir(dir: &fio::DirectoryProxy) -> Result<Vec<DirEntry>, EnumerateError> {
    readdir_inner(dir, /*include_dot=*/ false).await
}

/// Parses the buffer returned by a read_dirents FIDL call.
///
/// Returns either an error or a parsed entry for each entry in the supplied buffer (see
/// read_dirents for the format of this buffer).
pub fn parse_dir_entries(mut buf: &[u8]) -> Vec<Result<DirEntry, DecodeDirentError>> {
    #[derive(KnownLayout, FromBytes, Immutable, Unaligned)]
    #[repr(C, packed)]
    struct Dirent {
        /// The inode number of the entry.
        _ino: u64,
        /// The length of the filename located after this entry.
        size: u8,
        /// The type of the entry. One of the `fio::DIRENT_TYPE_*` constants.
        kind: u8,
        // The unterminated name of the entry.  Length is the `size` field above.
        // char name[0],
    }

    let mut entries = vec![];

    while !buf.is_empty() {
        let Ok((dirent, rest)) = Ref::<_, Dirent>::from_prefix(buf) else {
            entries.push(Err(DecodeDirentError::BufferOverrun));
            return entries;
        };

        let entry = {
            // Don't read past the end of the buffer.
            let size = usize::from(dirent.size);
            if size > rest.len() {
                entries.push(Err(DecodeDirentError::BufferOverrun));
                return entries;
            }

            // Advance to the next entry.
            buf = &rest[size..];
            match String::from_utf8(rest[..size].to_vec()) {
                Ok(name) => Ok(DirEntry {
                    name,
                    kind: DirentKind::from_primitive(dirent.kind).unwrap_or(DirentKind::Unknown),
                }),
                Err(err) => Err(DecodeDirentError::InvalidUtf8(err.utf8_error())),
            }
        };

        entries.push(entry);
    }

    entries
}
