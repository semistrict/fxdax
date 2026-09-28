// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/lib/vfs/rust/src/protocols.rs`: `ProtocolsExt` and `ToFileOptions`
// for `fio::Flags`, without the directory, symlink, service and node options, and without the
// io1 `fio::OpenFlags`.

use crate::common::CreationMode;
use crate::file::FileOptions;
use fidl_fuchsia_io as fio;
use zx_status::Status;

/// Extends fio::Flags and fio::OpenFlags
pub trait ProtocolsExt: ToFileOptions + Send + Sync + 'static {
    /// True if the directory protocol is allowed.
    fn is_dir_allowed(&self) -> bool;

    /// True if the file protocol is allowed.
    fn is_file_allowed(&self) -> bool;

    /// True if the symlink protocol is allowed.
    fn is_symlink_allowed(&self) -> bool;

    /// The creation mode for the connection.
    fn creation_mode(&self) -> CreationMode;

    /// The rights for the connection.  If None, it means the connection is not for a node based
    /// protocol.  If the connection is supposed to use the same rights as the parent connection,
    /// the rights should have been populated.
    fn rights(&self) -> Option<fio::Operations>;

    /// True if the file should be in append mode.
    fn is_append(&self) -> bool;

    /// True if the file should be truncated.
    fn is_truncate(&self) -> bool;

    /// If creating an object, Whether to create a directory.
    fn create_directory(&self) -> bool;

    /// True if the protocol should be a limited node connection.
    fn is_node(&self) -> bool;

    /// True if creating as an unnamed temporary object.
    fn create_unnamed_temporary_in_directory_path(&self) -> bool;
}

impl ProtocolsExt for fio::Flags {
    fn is_dir_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_DIRECTORY)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn is_file_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_FILE)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn is_symlink_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_SYMLINK)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn creation_mode(&self) -> CreationMode {
        if self.contains(fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY) {
            if self.contains(fio::Flags::FLAG_MUST_CREATE) {
                return CreationMode::UnlinkableUnnamedTemporary;
            }
            return CreationMode::UnnamedTemporary;
        }
        if self.contains(fio::Flags::FLAG_MUST_CREATE) {
            CreationMode::Always
        } else if self.contains(fio::Flags::FLAG_MAYBE_CREATE) {
            CreationMode::AllowExisting
        } else {
            CreationMode::Never
        }
    }

    fn rights(&self) -> Option<fio::Operations> {
        Some(flags_to_rights(self))
    }

    fn is_append(&self) -> bool {
        self.contains(fio::Flags::FILE_APPEND)
    }

    fn is_truncate(&self) -> bool {
        self.contains(fio::Flags::FILE_TRUNCATE)
    }

    fn create_directory(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_DIRECTORY)
    }

    fn is_node(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_NODE)
    }

    fn create_unnamed_temporary_in_directory_path(&self) -> bool {
        self.creation_mode() == CreationMode::UnnamedTemporary
            || self.creation_mode() == CreationMode::UnlinkableUnnamedTemporary
    }
}

pub trait ToFileOptions: Send + 'static {
    fn to_file_options(&self) -> Result<FileOptions, Status>;
}

impl ToFileOptions for fio::Flags {
    fn to_file_options(&self) -> Result<FileOptions, Status> {
        // Verify protocols.
        if !self.is_file_allowed() {
            if self.is_dir_allowed() && !self.is_symlink_allowed() {
                return Err(Status::NOT_DIR);
            } else {
                return Err(Status::WRONG_TYPE);
            }
        }

        // Verify prohibited flags and disallow invalid flag combinations.
        if self.contains(fio::Flags::FILE_TRUNCATE) && !self.contains(fio::Flags::PERM_WRITE_BYTES)
        {
            return Err(Status::INVALID_ARGS);
        }

        // Used to remove any non-file flags.
        const ALLOWED_RIGHTS: fio::Operations = fio::Operations::empty()
            .union(fio::Operations::GET_ATTRIBUTES)
            .union(fio::Operations::READ_BYTES)
            .union(fio::Operations::WRITE_BYTES)
            .union(fio::Operations::UPDATE_ATTRIBUTES)
            .union(fio::Operations::EXECUTE);

        Ok(FileOptions {
            rights: flags_to_rights(self).intersection(ALLOWED_RIGHTS),
            is_append: self.contains(fio::Flags::FILE_APPEND),
        })
    }
}

impl ToFileOptions for FileOptions {
    fn to_file_options(&self) -> Result<FileOptions, Status> {
        Ok(*self)
    }
}

fn flags_to_rights(flags: &fio::Flags) -> fio::Rights {
    fio::Rights::from_bits_truncate(flags.bits())
}
