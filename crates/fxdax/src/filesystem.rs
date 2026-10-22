// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A filesystem: an Fxfs volume, whose nodes are reached through handles to them.
//!
//! [`Filesystem`] is a fuchsia.io client of the volume, as Starnix's remote filesystems and fdio
//! are: each node a [`Node`] handle holds keeps a connection to it once an operation needs one,
//! the one that created it opened by name from its parent directory, or one served from the node
//! itself, with `FLAG_SEND_REPRESENTATION`, whose representation gives the node's protocol and
//! attributes. A listing of a directory reads through a connection of its own, which it closes.
//! File contents are read and written through the file's stream, as futures that the kernel wakes
//! once its pager has supplied or dirtied their pages.

use crate::connection::{self, Connection, RIGHTS};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::executor::assert_not_on_executor;
use crate::image::{Image, ImageShared};
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::node::*;
use crate::storage::Storage;
use crate::volume::{Hold, Volume};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use fxfs::object_store::ObjectDescriptor;
use fxfs_platform::directory::{self as fx_directory, FxDirectory};
use fxfs_platform::file::FxFile;
use fxfs_platform::node::{FxNode, OpenedNode};
use fxfs_platform::symlink::FxSymlink;
use fxfs_platform::volume::FxVolumeAndRoot;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;
use vfs::node::Node as _;
use zircon::user_copy::HostBuffer;

/// Runs `$body` with `$n` bound to `$node`, an `Arc<dyn FxNode>`, as the file, directory or
/// symlink it is, which serve its extended attributes; anything else fails with `NOT_SUPPORTED`.
macro_rules! on_node {
    ($node:expr, |$n:ident| $body:expr) => {
        match $node.into_any().downcast::<FxFile>() {
            Ok($n) => $body,
            Err(any) => match any.downcast::<FxDirectory>() {
                Ok($n) => $body,
                Err(any) => match any.downcast::<FxSymlink>() {
                    Ok($n) => $body,
                    Err(_) => Err(zx::Status::NOT_SUPPORTED),
                },
            },
        }
    };
}

/// The errno of an extended attribute's operation's failure: Fxfs's `NOT_FOUND` is the
/// attribute's, and `FILE_BIG` its name's or value's.
fn xattr_errno(status: zx::Status) -> Errno {
    match status {
        zx::Status::NOT_FOUND => Errno::ENODATA,
        zx::Status::FILE_BIG => Errno::E2BIG,
        status => Errno::of_status(status),
    }
}

/// The name of the volume a filesystem mounted from storage serves.
pub(crate) const VOLUME_NAME: &str = "default";

/// The number of threads of a filesystem's executor, unless [`Options`] say otherwise.
pub(crate) const DEFAULT_EXECUTOR_THREADS: u8 = 1;

/// How a filesystem or image is mounted.
#[derive(Clone, Debug)]
pub struct Options {
    executor_threads: NonZeroU8,
}

impl Options {
    /// The default options.
    pub fn new() -> Self {
        Self { executor_threads: NonZeroU8::new(DEFAULT_EXECUTOR_THREADS).unwrap() }
    }

    /// The number of threads Fxfs's work runs on: its journal, pager and background writeback.
    /// They are separate from the tasks and threads calling the filesystem. The default is 1:
    /// operations mostly wait for one another and the device, and on one thread they wake one
    /// another without waking other threads, which costs a request most of its time otherwise.
    pub fn executor_threads(&mut self, threads: NonZeroU8) -> &mut Self {
        self.executor_threads = threads;
        self
    }

    pub(crate) fn threads(&self) -> u8 {
        self.executor_threads.get()
    }

    /// Mounts the filesystem on `storage`, which [`format`](Self::format) formatted: the volume
    /// it created on an image of its own.
    pub async fn mount(&self, storage: Storage) -> Result<Filesystem, Error> {
        Filesystem::start(self, storage, false).await
    }

    /// Formats `storage` with an empty filesystem, and mounts it.
    pub async fn format(&self, storage: Storage) -> Result<Filesystem, Error> {
        Filesystem::start(self, storage, true).await
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// A mounted Fxfs filesystem: a volume of an [`Image`], or of an image of its own, mounted from
/// storage with [`mount`](Filesystem::mount). Its nodes are reached from its
/// [`root`](Self::root) directory. See the crate docs.
///
/// Its methods, and its nodes', may be called from any number of tasks or threads at once (see the
/// crate docs).
pub struct Filesystem {
    volume: Arc<Volume>,
    /// The root directory, which the filesystem holds while it lives.
    root: Node,
    /// What the filesystem's storage is. Locked while unmounting, so that concurrent unmounts wait
    /// for an image of its own to close.
    backing: futures::lock::Mutex<Backing>,
}

/// What a filesystem's storage is.
enum Backing {
    /// A volume of an [`Image`]'s, whose storage is the image's.
    Volume,
    /// An image of its own, mounted from storage, which unmounting closes.
    Own(Image),
    /// An image of its own, closed: its storage, for [`close`](Filesystem::close) to give back.
    Closed(Storage),
    /// An image of its own whose storage is gone: closing it failed, or was given up on, or
    /// [`close`](Filesystem::close) gave the storage back.
    Gone,
}

impl Filesystem {
    /// Mounts the filesystem on `storage` with the default [`Options`].
    pub async fn mount(storage: Storage) -> Result<Self, Error> {
        Options::new().mount(storage).await
    }

    /// Formats `storage` with an empty filesystem, and mounts it with the default [`Options`].
    pub async fn format(storage: Storage) -> Result<Self, Error> {
        Options::new().format(storage).await
    }

    /// Mounts or formats an image of its own on `storage`, and its volume.
    async fn start(options: &Options, storage: Storage, format: bool) -> Result<Self, Error> {
        let image = Image::start(options, storage, format).await?;
        let filesystem = if format {
            image.create_volume(VOLUME_NAME).await
        } else {
            image.open_volume(VOLUME_NAME).await
        };
        match filesystem {
            Ok(mut filesystem) => {
                *filesystem.backing.get_mut() = Backing::Own(image);
                Ok(filesystem)
            }
            Err(error) => {
                if let Err(close_error) = image.close().await {
                    log::error!("fxdax: failed to close the image: {close_error:#}");
                }
                Err(error)
            }
        }
    }

    /// The filesystem of `volume`, the volume `name` of `image`, just mounted, which the image
    /// unmounts when it is closed: serves the volume's root directory, which it holds.
    pub(crate) async fn new(image: Arc<ImageShared>, name: &str, volume: FxVolumeAndRoot) -> Self {
        let (volume, root) = image
            .executor
            .run(async move {
                let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
                volume.root().clone().serve(RIGHTS, server_end);
                (volume, root)
            })
            .await;
        let (volume, root) = Volume::new(image, name, volume, Connection::Directory(root));
        let root = Node(Arc::new(root));
        Self { volume, root, backing: futures::lock::Mutex::new(Backing::Volume) }
    }

    /// The filesystem's volume.
    pub(crate) fn volume(&self) -> &Arc<Volume> {
        &self.volume
    }

    /// The filesystem's root directory.
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty, and gives the
    /// storage of the image it was mounted from back, if it was mounted from storage. A volume
    /// of an [`Image`] fails once it is unmounted, as its storage is the image's, which
    /// [`Image::close`] gives back.
    pub async fn close(mut self) -> Result<Storage, Error> {
        self.unmount().await?;
        match std::mem::replace(self.backing.get_mut(), Backing::Gone) {
            Backing::Closed(storage) => Ok(storage),
            Backing::Volume => Err(Error::msg(
                "the filesystem is a volume of an image, whose storage is the image's",
            )),
            Backing::Gone => {
                Err(Error::msg("closing the filesystem's image failed, or was given up on"))
            }
            Backing::Own(_) => unreachable!("unmounting closes an image of its own"),
        }
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty. A filesystem
    /// mounted from storage then closes its image, keeping the storage for
    /// [`close`](Self::close) to give back. Every operation fails with `ENOTCONN` from then on,
    /// its nodes' too.
    ///
    /// Once unmounting begins, the volume's unmount completes even if the future is dropped.
    /// Concurrent calls wait for the same unmount, and only the first gives its error.
    pub async fn unmount(&self) -> Result<(), Error> {
        let mut backing = self.backing.lock().await;
        self.volume.image.unmount_volume(&self.volume).wait().await?;
        match std::mem::replace(&mut *backing, Backing::Gone) {
            Backing::Own(image) => *backing = Backing::Closed(image.close().await?),
            unchanged => *backing = unchanged,
        }
        Ok(())
    }

    /// Clones the filesystem's image to a new image at `path`, which must not exist, while it is
    /// mounted, and returns the clone's storage (see [`Image::clone_storage_to`]).
    pub async fn clone_storage_to(&self, path: impl AsRef<Path>) -> Result<Storage, Error> {
        // As every operation does, it fails once the filesystem is unmounted.
        self.volume.fx_volume()?;
        self.volume.image.clone_storage_to(path.as_ref()).await
    }

    /// The filesystem's usage.
    pub async fn statfs(&self) -> Result<StatFs> {
        let root = self.root.directory(&self.volume).await?;
        let info = self
            .volume
            .run(async move {
                let (status, info) = root.query_filesystem().await.errno()?;
                zx::Status::ok(status).errno()?;
                info.ok_or(Errno::EIO)
            })
            .await?;
        let block_size = info.block_size.max(1);
        Ok(StatFs {
            block_size,
            blocks: info.total_bytes / block_size as u64,
            blocks_free: info.total_bytes.saturating_sub(info.used_bytes) / block_size as u64,
            name_max: info.max_filename_size,
        })
    }
}

impl Drop for Filesystem {
    /// Unmounts the filesystem if it is still mounted, writing back what is dirty, blocking the
    /// dropping thread until it is done, and closes the image it was mounted from storage with,
    /// if it was. Await `close` or `unmount` rather than dropping a mounted filesystem.
    ///
    /// # Panics
    ///
    /// If a filesystem is dropped on Fxfs's executor while it is mounted, or while the image of
    /// its own it was mounted from storage with is open.
    fn drop(&mut self) {
        let unmount = self.volume.image.unmount_volume(&self.volume);
        if unmount.began() {
            assert_not_on_executor();
            if let Err(error) = self.volume.image.executor.run_blocking(unmount.wait()) {
                log::error!("fxdax: failed to unmount: {error:#}");
            }
        }
        // An image of its own closes as it is dropped. It waits for the volume's unmount under
        // way, which an `unmount` given up on leaves running, only while the volume lives, as it
        // finds the unmount through it: so it is dropped before the volume is.
        drop(std::mem::replace(self.backing.get_mut(), Backing::Gone));
    }
}

impl std::fmt::Debug for Filesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mounted = self.volume.fx_volume().is_ok();
        f.debug_struct("Filesystem").field("mounted", &mounted).finish_non_exhaustive()
    }
}

/// A handle to a node of a [`Filesystem`]: a file, directory, symlink or special file, whose
/// operations are its methods.
///
/// A handle holds its node open, as an open file does: a file unlinked stays, and can be read and
/// written, until the last handle to it drops. Clones of a handle are handles too, and a node may
/// have any number, from separate lookups too. A handle does not keep its filesystem mounted: once
/// it is unmounted, every operation fails with `ENOTCONN`.
#[derive(Clone)]
pub struct Node(Arc<Hold>);

impl Node {
    /// The node's inode number.
    pub fn inode(&self) -> Inode {
        Inode(self.0.object_id())
    }

    /// The node's volume, while its filesystem lives.
    fn volume(&self) -> Result<Arc<Volume>> {
        self.0.volume()
    }

    /// The volume, of which `other` must be a node too: `EXDEV` otherwise.
    fn same_volume(&self, other: &Node) -> Result<Arc<Volume>> {
        if !self.0.same_volume(&other.0) {
            return Err(Errno::EXDEV);
        }
        self.volume()
    }

    /// Fxfs's node.
    fn fx_node(&self, volume: &Volume) -> Result<Arc<dyn FxNode>> {
        volume.with_held(&self.0, |held| held.node.0.clone())
    }

    /// Fxfs's node of this directory: `ENOTDIR` if the node is not one.
    fn fx_directory(&self, volume: &Volume) -> Result<Arc<FxDirectory>> {
        let directory = self.fx_node(volume)?.into_any().downcast::<FxDirectory>();
        directory.map_err(|_| Errno::ENOTDIR)
    }

    /// A new connection to the node.
    async fn connect(&self, volume: &Volume) -> Result<Connection> {
        let node = self.fx_node(volume)?;
        let fx_volume = volume.fx_volume()?;
        volume
            .run(async move { connection::connect(OpenedNode::new(node), &fx_volume).await })
            .await
    }

    /// A connection to the node, which is opened the first time one is needed.
    async fn connection(&self, volume: &Volume) -> Result<Connection> {
        if let Some(connection) = volume.with_held(&self.0, |held| held.connection.clone())? {
            return Ok(connection);
        }
        let connection = self.connect(volume).await?;
        // Another operation may have connected meanwhile, whose connection is kept.
        volume.with_held(&self.0, |held| held.connection.get_or_insert(connection).clone())
    }

    async fn directory(&self, volume: &Volume) -> Result<fio::DirectoryProxy> {
        // `Clone::clone`, as the proxy's `clone` is fuchsia.io's `Clone`.
        Ok(Clone::clone(self.connection(volume).await?.directory()?))
    }

    async fn file(&self, volume: &Volume) -> Result<fio::FileProxy> {
        Ok(Clone::clone(self.connection(volume).await?.file()?.0))
    }

    async fn stream(&self) -> Result<Arc<zx::Stream>> {
        let volume = self.volume()?;
        Ok(self.connection(&volume).await?.file()?.1.clone())
    }

    /// The child `name` of this directory, and its attributes, if it has one.
    async fn child(
        &self,
        volume: &Volume,
        name: &[u8],
    ) -> Result<Option<(Arc<dyn FxNode>, fio::Attributes)>> {
        let directory = self.fx_directory(volume)?;
        let name = connection::name(name)?;
        volume
            .run(async move {
                let Some(node) = directory.lookup_child(&name).await.errno()? else {
                    return Ok(None);
                };
                let attributes = attributes(&node).await?;
                Ok(Some((node, attributes)))
            })
            .await
    }

    /// Looks up `name` in this directory, and returns a handle to the node it names and its
    /// attributes.
    pub async fn lookup(&self, name: &[u8]) -> Result<(Node, Attr)> {
        let volume = self.volume()?;
        let (node, attributes) = self.child(&volume, name).await?.ok_or(Errno::ENOENT)?;
        handle(&volume, node, None, attributes)
    }

    /// The node's attributes.
    pub async fn getattr(&self) -> Result<Attr> {
        let volume = self.volume()?;
        let node = self.fx_node(&volume)?;
        let node_type = file_type(&*node);
        let attributes = volume.run(async move { attributes(&node).await }).await?;
        attr(volume.block_size(), node_type, attributes)
    }

    /// Changes the node's attributes, and returns them.
    pub async fn setattr(&self, set: &SetAttr) -> Result<Attr> {
        let volume = self.volume()?;
        let connection = self.connection(&volume).await?;
        if let Some(size) = set.size {
            let proxy = Clone::clone(connection.file()?.0);
            volume.run(async move { proxy.resize(size).await.errno()?.errno() }).await?;
        }
        let mut update = fio::MutableNodeAttributes {
            uid: set.uid,
            gid: set.gid,
            access_time: set.atime.map(|time| nanos_from_time(time.resolve())).transpose()?,
            modification_time: set.mtime.map(|time| nanos_from_time(time.resolve())).transpose()?,
            ..Default::default()
        };
        if set.perm.is_some() || set.kill_suid_sgid {
            let attr = self.getattr().await?;
            let mut perm = attr.perm as u32;
            if set.kill_suid_sgid {
                perm &= !S_ISUID;
                if perm & S_IXGRP != 0 {
                    perm &= !S_ISGID;
                }
            }
            if let Some(new_perm) = set.perm {
                perm = new_perm as u32 & PERM_MASK;
            }
            update.mode = Some(attr.file_type.mode_bits() | perm);
        }
        if update != fio::MutableNodeAttributes::default() {
            self.update_attributes(&volume, connection, update).await?;
        }
        self.getattr().await
    }

    /// Updates the node's attributes through `connection`, a connection to it, or, for a symlink,
    /// whose connections don't update attributes, its node.
    async fn update_attributes(
        &self,
        volume: &Volume,
        connection: Connection,
        mut update: fio::MutableNodeAttributes,
    ) -> Result<()> {
        if let Connection::Symlink(_) = connection {
            let symlink = self.fx_node(volume)?.into_any().downcast::<FxSymlink>();
            let symlink = symlink.map_err(|_| Errno::EIO)?;
            // Fxfs keeps a node's owner with its mode, which a symlink fuchsia.io made has
            // neither of: a symlink's mode is always the same.
            if update.mode.is_some() || update.uid.is_some() || update.gid.is_some() {
                update.mode = Some(FileType::Symlink.mode_bits() | 0o777);
            }
            return volume
                .run(async move { symlink.update_attributes(&update).await.errno() })
                .await;
        }
        volume.run(async move { connection.update_attributes(&update).await }).await
    }

    /// The target of this symlink.
    pub async fn readlink(&self) -> Result<Vec<u8>> {
        let volume = self.volume()?;
        let Connection::Symlink(proxy) = self.connection(&volume).await? else {
            return Err(Errno::EINVAL);
        };
        volume.run(async move { proxy.describe().await.errno()?.target.ok_or(Errno::EIO) }).await
    }

    /// Creates `name` in this directory as a `protocol` node with `mode`, owned by `owner`.
    async fn create_node(
        &self,
        name: &[u8],
        protocol: fio::Flags,
        mode: u32,
        rdev: u32,
        owner: Owner,
    ) -> Result<(Node, Attr)> {
        let volume = self.volume()?;
        let directory = self.directory(&volume).await?;
        let name = connection::name(name)?;
        let flags = protocol | fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FLAG_MUST_CREATE;
        let create_attributes = fio::MutableNodeAttributes {
            mode: Some(mode),
            uid: Some(owner.uid),
            gid: Some(owner.gid),
            rdev: Some(rdev as u64),
            ..Default::default()
        };
        let (connection, attributes) = volume
            .run(async move {
                connection::open(&directory, &name, flags, Some(create_attributes)).await
            })
            .await?;
        handle_connected(&volume, connection, attributes)
    }

    /// Creates the regular file `name` in this directory with `perm`, owned by `owner`, and
    /// returns a handle to it and its attributes.
    pub async fn create(&self, name: &[u8], perm: u16, owner: Owner) -> Result<(Node, Attr)> {
        let mode = FileType::File.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(name, fio::Flags::PROTOCOL_FILE, mode, 0, owner).await
    }

    /// Creates the directory `name` in this directory with `perm`, owned by `owner`, and returns a
    /// handle to it and its attributes.
    pub async fn mkdir(&self, name: &[u8], perm: u16, owner: Owner) -> Result<(Node, Attr)> {
        let mode = FileType::Directory.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(name, fio::Flags::PROTOCOL_DIRECTORY, mode, 0, owner).await
    }

    /// Creates the node `name` in this directory of `file_type` with `perm`, owned by `owner`, and
    /// returns a handle to it and its attributes. A device node stands for device `rdev`.
    ///
    /// It may be a regular file, or a special file: a FIFO, socket, or character or block device,
    /// which is an Fxfs file whose mode says what it is, as Starnix makes them. Directories and
    /// symlinks fail with `EINVAL`.
    pub async fn mknod(
        &self,
        name: &[u8],
        file_type: FileType,
        perm: u16,
        rdev: u32,
        owner: Owner,
    ) -> Result<(Node, Attr)> {
        if matches!(file_type, FileType::Directory | FileType::Symlink) {
            return Err(Errno::EINVAL);
        }
        let mode = file_type.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(name, fio::Flags::PROTOCOL_FILE, mode, rdev, owner).await
    }

    /// Creates the symlink `name` in this directory to `target`, owned by `owner`, and returns a
    /// handle to it and its attributes.
    pub async fn symlink(&self, name: &[u8], target: &[u8], owner: Owner) -> Result<(Node, Attr)> {
        let volume = self.volume()?;
        let directory = self.directory(&volume).await?;
        let link_name = connection::name(name)?;
        let target = target.to_vec();
        let (proxy, attributes) = volume
            .run(async move {
                let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
                directory
                    .create_symlink(&link_name, &target, Some(server_end))
                    .await
                    .errno()?
                    .errno()?;
                let connection = Connection::Symlink(proxy);
                let attributes = connection.get_attributes().await?;
                Ok::<_, Errno>((connection, attributes))
            })
            .await?;
        let (node, _) = handle_connected(&volume, proxy.clone(), attributes)?;
        let owner = fio::MutableNodeAttributes {
            uid: Some(owner.uid),
            gid: Some(owner.gid),
            ..Default::default()
        };
        node.update_attributes(&volume, proxy, owner).await?;
        let attr = node.getattr().await?;
        Ok((node, attr))
    }

    /// Links the node into directory `dir` as `name`, and returns its attributes. Directories
    /// fail with `EPERM`, and a directory of another filesystem with `EXDEV`.
    pub async fn link(&self, dir: &Node, name: &[u8]) -> Result<Attr> {
        let volume = self.same_volume(dir)?;
        let connection = self.connection(&volume).await?;
        let directory = dir.directory(&volume).await?;
        let name = connection::name(name)?;
        let link_connection = connection.clone();
        let attributes = volume
            .run(async move {
                let token = connection::token(&directory).await?;
                match &link_connection {
                    Connection::File(proxy, _) => proxy.link_into(token, &name).await,
                    Connection::Symlink(proxy) => proxy.link_into(token, &name).await,
                    Connection::Directory(_) => return Err(Errno::EPERM),
                }
                .errno()?
                .errno()?;
                link_connection.get_attributes().await
            })
            .await?;
        attr(volume.block_size(), connection.file_type(), attributes)
    }

    /// Removes the non-directory `name` from this directory. A directory fails with `EISDIR`.
    pub async fn unlink(&self, name: &[u8]) -> Result<()> {
        self.remove(name, false).await
    }

    /// Removes the empty directory `name` from this directory. A non-directory fails with
    /// `ENOTDIR`.
    pub async fn rmdir(&self, name: &[u8]) -> Result<()> {
        self.remove(name, true).await
    }

    /// Removes `name`, as `rmdir` does if `directory`, and as `unlink` does otherwise, through
    /// Fxfs's directory: it checks what `name` holds in the transaction that removes it, and
    /// fuchsia.io's unlink can't require that it is not a directory.
    async fn remove(&self, name: &[u8], directory: bool) -> Result<()> {
        let volume = self.volume()?;
        let parent = self.fx_directory(&volume)?;
        let name = connection::name(name)?;
        volume.run(async move { parent.remove_child(&name, directory).await.errno() }).await
    }

    /// Renames `name` in this directory to `new_name` in directory `new_dir`, replacing what is
    /// there unless `flags` say not to. A directory of another filesystem fails with `EXDEV`.
    ///
    /// It renames through Fxfs's directory, which checks that `new_name` holds nothing, if it
    /// must not, in the transaction that renames: fuchsia.io's rename can't say not to replace.
    pub async fn rename(
        &self,
        name: &[u8],
        new_dir: &Node,
        new_name: &[u8],
        flags: RenameFlags,
    ) -> Result<()> {
        if flags.contains(RenameFlags::EXCHANGE) || !RenameFlags::all().contains(flags) {
            return Err(Errno::EINVAL);
        }
        let volume = self.same_volume(new_dir)?;
        let old_directory = self.fx_directory(&volume)?;
        let new_directory = new_dir.fx_directory(&volume)?;
        let old = connection::name(name)?;
        let new = connection::name(new_name)?;
        let no_replace = flags.contains(RenameFlags::NOREPLACE);
        volume
            .run(async move {
                new_directory.rename_child(&old_directory, &old, &new, no_replace).await.errno()
            })
            .await
    }

    /// The entries of this directory, but `.` and `..`, as it holds them now.
    pub async fn entries(&self) -> Result<Vec<DirEntry>> {
        let volume = self.volume()?;
        // Each listing reads through a connection of its own: a connection has one position in
        // its directory, which listings sharing it would move under one another.
        let connection = self.connect(&volume).await?;
        let entries = volume
            .run(async move { connection::read_entries(connection.directory()?).await })
            .await?;
        // Fxfs lists a directory's `.`, and no `..`.
        Ok(entries
            .into_iter()
            .filter(|(_, _, name)| name != b".")
            .map(|(object_id, file_type, name)| DirEntry {
                inode: Inode(object_id),
                file_type,
                name,
            })
            .collect())
    }

    /// Reads from this file at `offset` into `buf`, and returns how much was read: less than asked
    /// only at the end of the file.
    pub async fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_vectored(offset, &mut [VolatileSliceMut::from(buf)]).await
    }

    /// Reads from this file at `offset` into `bufs` in turn, memory that may change meanwhile, and
    /// returns how much was read: less than asked only at the end of the file.
    ///
    /// The file's pages are copied straight into the buffers. Waiting for pages not yet in memory
    /// holds no thread: the future is woken once Fxfs's pager has supplied them. Dropping the
    /// future before it completes leaves the buffers with what was read so far.
    pub async fn read_vectored(
        &self,
        offset: u64,
        bufs: &mut [VolatileSliceMut<'_>],
    ) -> Result<usize> {
        let stream = self.stream().await?;
        let bufs: Vec<HostBuffer> = bufs.iter().map(VolatileSliceMut::host_buffer).collect();
        zircon::syscalls::stream_readv_at_async(stream.raw_handle(), offset, &bufs).await.errno()
    }

    /// Writes `data` to this file at `offset`, extending it if that is past its end, and returns
    /// how much was written.
    pub async fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        self.write_vectored(offset, &[VolatileSlice::from(data)]).await
    }

    /// Writes `bufs` in turn to this file at `offset`, memory that may change meanwhile, and
    /// returns how much was written.
    ///
    /// Waiting for Fxfs's pager, to read pages in or mark them dirty, holds no thread. Dropping the
    /// future before it completes leaves the file with what was written so far.
    pub async fn write_vectored(&self, offset: u64, bufs: &[VolatileSlice<'_>]) -> Result<usize> {
        let stream = self.stream().await?;
        let bufs: Vec<HostBuffer> = bufs.iter().map(VolatileSlice::host_buffer).collect();
        zircon::syscalls::stream_writev_at_async(stream.raw_handle(), offset, &bufs).await.errno()
    }

    /// Allocates `len` bytes of this file at `offset`, extending it if they are past its end.
    pub async fn allocate(&self, offset: u64, len: u64) -> Result<()> {
        let volume = self.volume()?;
        let proxy = self.file(&volume).await?;
        volume
            .run(async move {
                proxy.allocate(offset, len, fio::AllocateMode::empty()).await.errno()?.errno()
            })
            .await
    }

    /// Writes back the node's data and metadata, and for a directory, the filesystem's.
    pub async fn fsync(&self) -> Result<()> {
        let volume = self.volume()?;
        let connection = self.connection(&volume).await?;
        volume
            .run(async move {
                match &connection {
                    Connection::Directory(proxy) => proxy.sync().await.errno()?,
                    Connection::File(proxy, _) => proxy.sync().await.errno()?,
                    Connection::Symlink(_) => Ok(()),
                }
                .errno()
            })
            .await
    }

    /// The value of the node's extended attribute `name`. Fails with `ENODATA` if it has none of
    /// that name.
    pub async fn getxattr(&self, name: &[u8]) -> Result<Vec<u8>> {
        let volume = self.volume()?;
        let (node, name) = (self.fx_node(&volume)?, name.to_vec());
        volume
            .run(async move { on_node!(node, |n| n.get_extended_attribute(name).await) })
            .await
            .map_err(xattr_errno)
    }

    /// Sets the node's extended attribute `name` to `value`, as `mode` says.
    pub async fn setxattr(&self, name: &[u8], value: &[u8], mode: XattrMode) -> Result<()> {
        let volume = self.volume()?;
        let (node, name, value) = (self.fx_node(&volume)?, name.to_vec(), value.to_vec());
        let mode = match mode {
            XattrMode::Set => fio::SetExtendedAttributeMode::Set,
            XattrMode::Create => fio::SetExtendedAttributeMode::Create,
            XattrMode::Replace => fio::SetExtendedAttributeMode::Replace,
        };
        volume
            .run(async move { on_node!(node, |n| n.set_extended_attribute(name, value, mode).await) })
            .await
            .map_err(xattr_errno)
    }

    /// The names of the node's extended attributes.
    pub async fn listxattr(&self) -> Result<Vec<Vec<u8>>> {
        let volume = self.volume()?;
        let node = self.fx_node(&volume)?;
        volume
            .run(async move { on_node!(node, |n| n.list_extended_attributes().await) })
            .await
            .map_err(xattr_errno)
    }

    /// Removes the node's extended attribute `name`. Fails with `ENODATA` if it has none of that
    /// name.
    pub async fn removexattr(&self, name: &[u8]) -> Result<()> {
        let volume = self.volume()?;
        let (node, name) = (self.fx_node(&volume)?, name.to_vec());
        volume
            .run(async move { on_node!(node, |n| n.remove_extended_attribute(name).await) })
            .await
            .map_err(xattr_errno)
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").field("inode", &self.inode()).finish_non_exhaustive()
    }
}

/// A new handle to `node`, of `volume`, which `connection` connects to if given and it has no
/// connection yet, and the node's attributes, from `attributes`.
fn handle(
    volume: &Arc<Volume>,
    node: Arc<dyn FxNode>,
    connection: Option<Connection>,
    attributes: fio::Attributes,
) -> Result<(Node, Attr)> {
    let attr = attr(volume.block_size(), file_type(&*node), attributes)?;
    let hold = volume.hold(node, connection)?;
    Ok((Node(Arc::new(hold)), attr))
}

/// A new handle to the node a new `connection` connects to, as [`handle`] gives it.
fn handle_connected(
    volume: &Arc<Volume>,
    connection: Connection,
    attributes: fio::Attributes,
) -> Result<(Node, Attr)> {
    let object_id = attributes.1.id.ok_or(Errno::EIO)?;
    // The connection holds the node open, so it is in the volume's cache.
    let node = volume.fx_volume()?.cache().get(object_id).ok_or(Errno::EIO)?;
    handle(volume, node, Some(connection), attributes)
}

/// The attributes of a node of `node_type`, unless its mode says otherwise, from its fuchsia.io
/// attributes, on an image of `block_size` blocks.
fn attr(
    block_size: u32,
    node_type: FileType,
    (mutable, immutable): fio::Attributes,
) -> Result<Attr> {
    // Special files are Fxfs files whose mode says what they are, as Starnix makes them.
    let file_type = mutable.mode.and_then(FileType::from_mode).unwrap_or(node_type);
    let perm = mutable.mode.map_or(file_type.default_perm(), |mode| (mode & PERM_MASK) as u16);
    let object_id = immutable.id.ok_or(Errno::EIO)?;
    Ok(Attr {
        inode: Inode(object_id),
        size: immutable.content_size.unwrap_or(0),
        blocks: immutable.storage_size.unwrap_or(0).div_ceil(512),
        atime: time_from_nanos(mutable.access_time.unwrap_or(0)),
        mtime: time_from_nanos(mutable.modification_time.unwrap_or(0)),
        ctime: time_from_nanos(immutable.change_time.unwrap_or(0)),
        file_type,
        perm,
        nlink: immutable.link_count.unwrap_or(1) as u32,
        uid: mutable.uid.unwrap_or(0),
        gid: mutable.gid.unwrap_or(0),
        rdev: mutable.rdev.unwrap_or(0) as u32,
        blksize: block_size,
    })
}

/// The type of `node`, unless its mode says otherwise.
fn file_type(node: &dyn FxNode) -> FileType {
    match node.object_descriptor() {
        ObjectDescriptor::Directory => FileType::Directory,
        ObjectDescriptor::Symlink => FileType::Symlink,
        _ => FileType::File,
    }
}

/// The attributes of `node`, as fxdax makes nodes' attributes from.
async fn attributes(node: &Arc<dyn FxNode>) -> Result<fio::Attributes> {
    let attributes = fx_directory::get_attributes(node, connection::ATTRIBUTES).await.errno()?;
    Ok((attributes.mutable_attributes, attributes.immutable_attributes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use futures::executor::block_on;
    use std::collections::BTreeMap;

    const OWNER: Owner = Owner { uid: 1000, gid: 100 };

    /// A node is held while a handle to it lives: one entry, however many handles, which goes
    /// with the last of them. Clones are the same handle.
    #[test]
    fn handles_hold_nodes_until_the_last_drops() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let root = fs.root().inode().0;
            let held = || fs.volume().held();
            assert_eq!(held(), BTreeMap::from([(root, 1)]));

            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"contents").await, Ok(8));
            let id = file.inode().0;
            let clone = file.clone();
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 1)]));
            let (again, _) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(again.inode().0, id);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 2)]));

            fs.root().unlink(b"file").await.unwrap();
            drop(file);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 2)]));
            drop(clone);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 1)]));
            let mut buf = [0; 16];
            assert_eq!(again.read(0, &mut buf).await, Ok(8));
            assert_eq!(&buf[..8], b"contents");
            drop(again);
            assert_eq!(held(), BTreeMap::from([(root, 1)]));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// An unmounted filesystem dropped on Fxfs's executor neither blocks it nor panics: only a
    /// mounted one's drop unmounts it.
    #[test]
    fn dropping_an_unmounted_filesystem_on_the_executor_neither_blocks_nor_panics() {
        let fs = block_on(Filesystem::format(Storage::memory(8 << 20))).unwrap();
        block_on(fs.unmount()).unwrap();
        let image = fs.volume().image.clone();
        let dropped = image.executor.run_blocking(async move {
            drop(fs);
            true
        });
        assert!(dropped);
    }

    /// A filesystem of an image of its own, dropped while an unmount given up on is under way,
    /// closes its image once that unmount is done, not under it: the volume is unmounted when the
    /// drop returns, and the image is whole, with what was written.
    #[test]
    fn dropping_a_filesystem_mid_unmount_closes_its_image_after_the_unmount() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("image");
            let fs = Filesystem::format(Storage::create(&path, 64 << 20).unwrap()).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"written").await, Ok(7));
            drop(file);
            // Polled once, the unmount begins the volume's, and is given up on while it waits.
            assert!(fs.unmount().now_or_never().is_none(), "the volume unmounted at once");
            let unmounted = fs.volume.image.unmount_volume(&fs.volume);
            assert!(!unmounted.began());
            drop(fs);
            assert!(unmounted.wait().now_or_never().is_some(), "the image closed mid-unmount");

            let fs = Filesystem::mount(Storage::open(&path).unwrap().check().await.unwrap())
                .await
                .unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }
}
