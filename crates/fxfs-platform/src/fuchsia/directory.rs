// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/directory.rs`: FxDirectory as the node of a
// directory, looking up and creating its children, and opening files in it for fuchsia.io
// connections, which are served in-process (see the `fidl` stand-in).
//
// Deviations:
// - `lookup` looks up a single name rather than a path, and takes the attributes to create it with
//   rather than an object request. There is no dirent cache, and no unnamed temporary files.
// - `open` opens files only, as there are no directory or symlink connections.
// - No watchers, links, unlinks, renames, symlinks, extended attributes or reading entries.

use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::file::FxFile;
use crate::fuchsia::node::{FxNode, GetResult, OpenedNode};
use crate::fuchsia::volume::{FxVolume, RootDir};
use anyhow::{Error, bail};
use either::{Left, Right};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use futures::future::BoxFuture;
use fxfs::errors::FxfsError;
use fxfs::object_store::transaction::{LockKey, Options, Transaction, lock_keys};
use fxfs::object_store::{self, Directory, ObjectDescriptor, ObjectStore};
use fxfs_macros::ToWeakNode;
use std::sync::Arc;
use vfs::ProtocolsExt;
use vfs::execution_scope::ExecutionScope;

#[derive(ToWeakNode)]
pub struct FxDirectory {
    // The root directory is the only directory which has no parent, and its parent can never
    // change, hence the Option can go on the outside.
    parent: Option<Mutex<Arc<FxDirectory>>>,
    directory: object_store::Directory<FxVolume>,
}

impl RootDir for FxDirectory {
    fn serve(self: Arc<Self>, _flags: fio::Flags, server_end: ServerEnd<fio::DirectoryMarker>) {
        let scope = self.volume().scope().clone();
        server_end.serve(Arc::new(DirectoryConnection { directory: self, scope }));
    }

    fn as_node(self: Arc<Self>) -> Arc<dyn FxNode> {
        self as Arc<dyn FxNode>
    }
}

impl FxDirectory {
    pub(super) fn new(
        parent: Option<Arc<FxDirectory>>,
        directory: object_store::Directory<FxVolume>,
    ) -> Self {
        Self { parent: parent.map(|p| Mutex::new(p)), directory }
    }

    pub fn directory(&self) -> &object_store::Directory<FxVolume> {
        &self.directory
    }

    pub fn volume(&self) -> &Arc<FxVolume> {
        self.directory.owner()
    }

    pub fn store(&self) -> &ObjectStore {
        self.directory.store()
    }

    pub fn is_deleted(&self) -> bool {
        self.directory.is_deleted()
    }

    pub fn set_deleted(&self) {
        self.directory.set_deleted();
    }

    /// Looks up the child called `name`, creating it if `protocols` ask for that, and opens it.
    pub async fn lookup(
        self: &Arc<Self>,
        protocols: &dyn ProtocolsExt,
        name: &str,
        create_attributes: Option<&fio::MutableNodeAttributes>,
    ) -> Result<OpenedNode<dyn FxNode>, Error> {
        let store = self.store();
        let fs = store.filesystem();
        let current_dir = self;

        // Create the transaction here if we might need to create the object so that we have a
        // lock in place.
        let keys =
            lock_keys![LockKey::object(store.store_object_id(), current_dir.directory.object_id())];
        let create_object = matches!(
            protocols.creation_mode(),
            vfs::CreationMode::AllowExisting | vfs::CreationMode::Always
        );
        let transaction_or_guard = if create_object {
            Left(store.new_transaction(keys, Options::default()).await?)
        } else {
            // When child objects are created, the object is created along with the
            // directory entry in the same transaction, and so we need to hold a read lock
            // over the lookup and open calls.
            Right(fs.lock_manager().read_lock(keys).await)
        };

        let child_descriptor = if let Some((object_id, object_descriptor, _locked)) =
            current_dir.directory.lookup(name).await?
        {
            let child_node = self
                .volume()
                .get_or_load_node(object_id, object_descriptor.clone(), Some(current_dir.clone()))
                .await?;
            Some((child_node, object_descriptor))
        } else {
            None
        };

        match child_descriptor {
            Some((child_node, object_descriptor)) => {
                if transaction_or_guard.is_left()
                    && protocols.creation_mode() == vfs::CreationMode::Always
                {
                    bail!(FxfsError::AlreadyExists);
                }
                match object_descriptor {
                    ObjectDescriptor::Directory => {
                        if !protocols.is_node() && !protocols.is_dir_allowed() {
                            if protocols.is_file_allowed() {
                                bail!(FxfsError::NotFile)
                            } else {
                                bail!(FxfsError::WrongType)
                            }
                        }
                    }
                    ObjectDescriptor::File => {
                        if !protocols.is_node() && !protocols.is_file_allowed() {
                            if protocols.is_dir_allowed() {
                                bail!(FxfsError::NotDir)
                            } else {
                                bail!(FxfsError::WrongType)
                            }
                        }
                    }
                    ObjectDescriptor::Symlink => {
                        if !protocols.is_node() && !protocols.is_symlink_allowed() {
                            bail!(FxfsError::WrongType)
                        }
                    }
                    ObjectDescriptor::Volume => bail!(FxfsError::Inconsistent),
                }
                // We must make sure to take an open-count whilst we are holding a read
                // lock.
                Ok(OpenedNode::new(child_node))
            }
            None => {
                if let Left(mut transaction) = transaction_or_guard {
                    let new_node = current_dir
                        .create_child(
                            &mut transaction,
                            name,
                            protocols.create_directory(),
                            create_attributes,
                        )
                        .await?;
                    if let GetResult::Placeholder(p) =
                        self.volume().cache().get_or_reserve(new_node.object_id()).await
                    {
                        transaction
                            .commit_with_callback(|_| {
                                p.commit(&new_node);
                                // NOTE: We don't take the open count until here in case the
                                // transaction fails.
                                OpenedNode::new(new_node)
                            })
                            .await
                    } else {
                        // We created a node, but the object ID was already used in the cache,
                        // which suggests a object ID was reused (which would either be a bug or
                        // corruption).
                        bail!(FxfsError::Inconsistent);
                    }
                } else {
                    bail!(FxfsError::NotFound);
                }
            }
        }
    }

    async fn create_child(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        name: &str,
        create_dir: bool, // If false, creates a file.
        create_attributes: Option<&fio::MutableNodeAttributes>,
    ) -> Result<Arc<dyn FxNode>, Error> {
        if create_dir {
            let dir = Arc::new(FxDirectory::new(
                Some(self.clone()),
                self.directory.create_child_dir(transaction, name).await?,
            ));
            if let Some(attrs) = create_attributes {
                dir.directory().handle().update_attributes(transaction, Some(&attrs), None).await?;
            }
            Ok(dir as Arc<dyn FxNode>)
        } else {
            let file = FxFile::new(self.directory.create_child_file(transaction, name).await?);
            if let Some(attrs) = create_attributes {
                file.handle()
                    .uncached_handle()
                    .update_attributes(transaction, Some(&attrs), None)
                    .await?;
            }
            Ok(file as Arc<dyn FxNode>)
        }
    }

    /// Opens the file called `name` for a new connection, as `open_async` does for files.
    pub async fn open_file(
        self: Arc<Self>,
        scope: ExecutionScope,
        name: &str,
        flags: fio::Flags,
        options: &fio::Options,
    ) -> Result<fio::FileProxy, zx::Status> {
        let node = self
            .lookup(&flags, name, options.create_attributes.as_ref())
            .await
            .map_err(map_to_status)?;
        if node.is::<FxFile>() {
            let file = node.downcast::<FxFile>().unwrap_or_else(|_| unreachable!());
            FxFile::create_connection_async(file, scope, flags).await
        } else {
            Err(zx::Status::NOT_SUPPORTED)
        }
    }
}

impl Drop for FxDirectory {
    fn drop(&mut self) {
        self.volume().cache().remove(self);
    }
}

impl FxNode for FxDirectory {
    fn object_id(&self) -> u64 {
        self.directory.object_id()
    }

    fn parent(&self) -> Option<Arc<FxDirectory>> {
        self.parent.as_ref().map(|p| p.lock().clone())
    }

    fn set_parent(&self, parent: Arc<FxDirectory>) {
        match &self.parent {
            Some(p) => *p.lock() = parent,
            None => panic!("Called set_parent on root node"),
        }
    }

    // If these ever do anything, BlobDirectory might need to be fixed.
    fn open_count_add_one(&self) {}
    fn open_count_sub_one(self: Arc<Self>) {}

    fn object_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor::Directory
    }
}

impl From<Directory<FxVolume>> for FxDirectory {
    fn from(dir: Directory<FxVolume>) -> Self {
        Self::new(None, dir)
    }
}

/// A `fuchsia.io/Directory` connection to a directory, served in-process.
struct DirectoryConnection {
    directory: Arc<FxDirectory>,
    scope: ExecutionScope,
}

impl fio::DirectoryServer for DirectoryConnection {
    fn open<'a>(
        &'a self,
        path: &'a str,
        flags: fio::Flags,
        options: &'a fio::Options,
    ) -> BoxFuture<'a, Result<fio::FileProxy, zx::Status>> {
        async move {
            let Some(_guard) = self.scope.try_active_guard() else {
                return Err(zx::Status::PEER_CLOSED);
            };
            self.directory.clone().open_file(self.scope.clone(), path, flags, options).await
        }
        .boxed()
    }

    fn close(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        async { Ok(()) }.boxed()
    }
}
