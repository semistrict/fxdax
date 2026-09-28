// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common trait for all the directory entry objects.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/directory/entry.rs`: the entry information nodes
//! give directories, without the `DirectoryEntry` trait and the open requests it serves.

use fidl_fuchsia_io as fio;

/// Information about a directory entry, used to populate ReadDirents() output.
/// The first element is the inode number, or INO_UNKNOWN (from fuchsia.io) if not set, and the second
/// element is one of the DIRENT_TYPE_* constants defined in the fuchsia.io.
#[derive(PartialEq, Eq, Clone)]
pub struct EntryInfo(u64, fio::DirentType);

impl EntryInfo {
    /// Constructs a new directory entry information object.
    pub fn new(inode: u64, type_: fio::DirentType) -> Self {
        Self(inode, type_)
    }

    /// Retrives the `inode` argument of the [`EntryInfo::new()`] constructor.
    pub fn inode(&self) -> u64 {
        let Self(inode, _type) = self;
        *inode
    }

    /// Retrieves the `type_` argument of the [`EntryInfo::new()`] constructor.
    pub fn type_(&self) -> fio::DirentType {
        let Self(_inode, type_) = self;
        *type_
    }
}

/// Give useful information about the entry, for example, the directory entry type.
pub trait GetEntryInfo {
    /// This method is used to populate ReadDirents() output.
    fn entry_info(&self) -> EntryInfo;
}
