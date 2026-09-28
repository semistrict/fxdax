// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/lib/vfs/rust/src/directory/connection.rs`: `BaseConnection`'s handling
// of the directory requests that `fio::DirectoryServer` has, copied verbatim except as follows.
//
// - Requests are calls from in-process proxies (see the `fidl` stand-in), which the mutable
//   connection dispatches to these handlers; `handle_open_request` and `handle_rewind` handle the
//   `Open` and `Rewind` requests, which upstream handles in `handle_request`.
// - There are no watchers or extended attributes.

use crate::common::CreationMode;
use crate::directory::entry_container::Directory;
use crate::directory::traversal_position::TraversalPosition;
use crate::directory::{DirectoryOptions, read_dirents};
use crate::execution_scope::{ExecutionScope, yield_to_executor};
use crate::node::OpenNode;
use crate::object_request::Representation;
use crate::path::Path;
use crate::{ObjectRequest, ObjectRequestRef, ProtocolsExt};
use flex_fuchsia_io as fio;
use zx_status::Status;

/// Handles functionality shared between mutable and immutable FIDL connections to a directory.  A
/// single directory may contain multiple connections.  Instances of the `BaseConnection`
/// will also hold any state that is "per-connection".  Currently that would be the access flags
/// and the seek position.
pub(in crate::directory) struct BaseConnection<DirectoryType: Directory> {
    /// Execution scope this connection and any async operations and connections it creates will
    /// use.
    pub(in crate::directory) scope: ExecutionScope,

    pub(in crate::directory) directory: OpenNode<DirectoryType>,

    /// Flags set on this connection when it was opened or cloned.
    pub(in crate::directory) options: DirectoryOptions,

    /// Seek position for this connection to the directory.  We just store the element that was
    /// returned last by ReadDirents for this connection.  Next call will look for the next element
    /// in alphabetical order and resume from there.
    seek: TraversalPosition,
}

impl<DirectoryType: Directory> BaseConnection<DirectoryType> {
    /// Constructs an instance of `BaseConnection` - to be used by derived connections, when they
    /// need to create a nested `BaseConnection` "sub-object".  But when implementing
    /// `create_connection`, derived connections should use the [`create_connection`] call.
    pub(in crate::directory) fn new(
        scope: ExecutionScope,
        directory: OpenNode<DirectoryType>,
        options: DirectoryOptions,
    ) -> Self {
        BaseConnection { scope, directory, options, seek: Default::default() }
    }

    /// Handles `Open`.
    pub(in crate::directory) async fn handle_open_request(
        &self,
        path: String,
        mut flags: fio::Flags,
        options: fio::Options,
        object: flex_client::Channel,
    ) {
        // Remove POSIX flags when the respective rights are not available.
        if !self.options.rights.contains(fio::INHERITED_WRITE_PERMISSIONS) {
            flags &= !fio::Flags::PERM_INHERIT_WRITE;
        }
        if !self.options.rights.contains(fio::Rights::EXECUTE) {
            flags &= !fio::Flags::PERM_INHERIT_EXECUTE;
        }

        ObjectRequest::new(flags, &options, object)
            .handle_async(async |req| self.handle_open(path, flags, req).await)
            .await;
        // Since open typically spawns a task, yield to the executor now to give that task a
        // chance to run before we try and process the next request for this directory.
        yield_to_executor().await;
    }

    pub(in crate::directory) async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.directory.get_attributes(query).await
    }

    pub(in crate::directory) fn handle_clone(&mut self, object: flex_client::Channel) {
        let flags = fio::Flags::from(&self.options);
        ObjectRequest::new(flags, &Default::default(), object)
            .handle(|req| self.directory.clone().open(self.scope.clone(), Path::dot(), flags, req));
    }

    async fn handle_open(
        &self,
        path: String,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let path = Path::validate_and_split(path)?;

        if !path.is_dot() && !self.options.rights.contains(fio::Operations::TRAVERSE) {
            return Err(Status::ACCESS_DENIED);
        }

        // Child connection must have stricter or same rights as the parent connection.
        if let Some(rights) = flags.rights() {
            if rights.intersects(!self.options.rights) {
                return Err(Status::ACCESS_DENIED);
            }
        }

        // If the target negotiated to a service, ensure this directory connection permits
        // connecting.
        if flags.intersects(fio::Flags::PROTOCOL_SERVICE)
            && !self.options.rights.contains(fio::Operations::CONNECT)
        {
            return Err(Status::ACCESS_DENIED);
        }

        // If requesting attributes, check permission.
        if !object_request.attributes().is_empty()
            && !self.options.rights.contains(fio::Operations::GET_ATTRIBUTES)
        {
            return Err(Status::ACCESS_DENIED);
        }

        match flags.creation_mode() {
            CreationMode::Never => {
                if object_request.create_attributes().is_some() {
                    return Err(Status::INVALID_ARGS);
                }
            }
            CreationMode::UnnamedTemporary | CreationMode::UnlinkableUnnamedTemporary => {
                // We only support creating unnamed temporary files.
                if !flags.intersects(fio::Flags::PROTOCOL_FILE) {
                    return Err(Status::NOT_SUPPORTED);
                }
                // The parent connection must be able to modify directories if creating an object.
                if !self.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
                    return Err(Status::ACCESS_DENIED);
                }
                // The ability to create an unnamed temporary file is dependent on the filesystem.
                // We won't know if the directory the path eventually leads to supports the creation
                // of unnamed temporary files until we have fully traversed the path. The way that
                // Rust VFS is set up is such that the filesystem is responsible for traversing the
                // path, so it is the filesystem's responsibility to report if it does not support
                // this feature.
            }
            CreationMode::AllowExisting | CreationMode::Always => {
                // The parent connection must be able to modify directories if creating an object.
                if !self.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
                    return Err(Status::ACCESS_DENIED);
                }

                let protocol_flags = flags & fio::MASK_KNOWN_PROTOCOLS;
                // If creating an object, exactly one protocol must be specified (the flags must be
                // a power of two and non-zero).
                if protocol_flags.is_empty()
                    || (protocol_flags.bits() & (protocol_flags.bits() - 1)) != 0
                {
                    return Err(Status::INVALID_ARGS);
                }
                // Only a directory or file object can be created.
                if !protocol_flags
                    .intersects(fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PROTOCOL_FILE)
                {
                    return Err(Status::NOT_SUPPORTED);
                }
            }
        }

        if path.is_dot() && flags.creation_mode() == CreationMode::Always {
            return Err(Status::ALREADY_EXISTS);
        }

        self.directory.clone().open_async(self.scope.clone(), path, flags, object_request).await
    }

    pub(in crate::directory) async fn handle_read_dirents(
        &mut self,
        max_bytes: u64,
    ) -> (Result<(), Status>, Vec<u8>) {
        if !self.options.rights.intersects(fio::Operations::ENUMERATE) {
            return (Err(Status::ACCESS_DENIED), Vec::new());
        }
        let (new_pos, sealed) =
            match self.directory.read_dirents(&self.seek, read_dirents::Sink::new(max_bytes)).await
            {
                Ok(res) => res,
                Err(e) => return (Err(e), Vec::new()),
            };
        self.seek = new_pos;
        match sealed.open().downcast::<read_dirents::Done>() {
            Ok(done) => (done.status, done.buf),
            Err(_) => {
                debug_assert!(
                    false,
                    "`read_dirents()` returned a `dirents_sink::Sealed` \
                    instance that is not an instance of the \
                    `read_dirents::Done`. This is a bug in the \
                    `read_dirents()` implementation."
                );
                (Err(Status::NOT_SUPPORTED), Vec::new())
            }
        }
    }

    /// Handles `Rewind`.
    pub(in crate::directory) fn handle_rewind(&mut self) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::ENUMERATE) {
            Err(Status::ACCESS_DENIED)
        } else {
            self.seek = Default::default();
            Ok(())
        }
    }

    pub(in crate::directory) async fn handle_link(
        &self,
        source_name: &str,
        target_parent_token: flex_client::NullableHandle,
        target_name: String,
    ) -> Result<(), Status> {
        if source_name.contains('/') || target_name.contains('/') {
            return Err(Status::INVALID_ARGS);
        }

        // To avoid rights escalation, we must make sure that the connection to the source directory
        // has the maximal set of file rights.  We do not check for EXECUTE because mutable
        // filesystems that support link don't currently support EXECUTE rights.
        if !self.options.rights.contains(fio::RW_STAR_DIR) {
            return Err(Status::BAD_HANDLE);
        }

        let (target_parent, target_rights) = self
            .scope
            .token_registry()
            .get_owner_and_rights(target_parent_token)?
            .ok_or(Status::NOT_FOUND)?;

        if !target_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }

        target_parent.link(target_name, self.directory.clone().into_any(), source_name).await
    }
}

impl<DirectoryType: Directory> Representation for BaseConnection<DirectoryType> {
    type Protocol = fio::DirectoryMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::Directory(fio::DirectoryInfo {
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.directory.get_attributes(requested_attributes).await?)
            },
            ..Default::default()
        }))
    }
}
