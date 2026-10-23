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
use crate::volume::{Hold, OpenGuarded, Volume};
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
    /// Operations under way as it begins complete before it writes the volume back, as it waits for
    /// the work they run on Fxfs's executor, and for the reads and writes copying to and from
    /// files' pages through their streams, on the callers' threads, which hold their files open
    /// until they complete or are dropped: what they write is written back, and the pages they
    /// need are read in, or let be dirtied, meanwhile. A read or write makes progress only as its
    /// caller polls it, so one its caller has stopped polling holds the unmount off until it is
    /// dropped: awaited in the task that would poll one, the unmount never completes. Complete or
    /// drop the reads and writes under way first.
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
    /// if it was. The unmount waits for the reads and writes under way, which make progress only
    /// as their callers poll them (see [`unmount`](Self::unmount)): dropping the filesystem on the
    /// thread that would poll one never returns, so complete or drop them first. Await `close` or
    /// `unmount` rather than dropping a mounted filesystem.
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

    /// A connection to the node, which is opened the first time one is needed.
    async fn connection(&self, volume: &Volume) -> Result<Connection> {
        if let Some(connection) = volume.with_held(&self.0, |held| held.connection.clone())? {
            return Ok(connection);
        }
        let node = self.fx_node(volume)?;
        let fx_volume = volume.fx_volume()?;
        let connection = volume
            .run(async move { connection::connect(OpenedNode::new(node), &fx_volume).await })
            .await?;
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

    /// This file's stream, which its reads and writes copy through, and the file opened under a
    /// guard on the volume's scope, which they hold while they copy (see [`OpenGuarded`]). The
    /// file's connection, which gives the stream, is found as the file is opened, under one hold
    /// of the volume's lock, and opened the first time one is needed.
    async fn stream(&self) -> Result<(Arc<zx::Stream>, OpenGuarded)> {
        let volume = self.volume()?;
        let (connection, open) = volume.open_guarded(&self.0, |held| held.connection.clone())?;
        let connection = match connection {
            Some(connection) => connection,
            None => self.connection(&volume).await?,
        };
        Ok((connection.file()?.1.clone(), open))
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
        let directory = self.fx_directory(&volume)?;
        let entries = volume.run(connection::read_entries(directory)).await?;
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
    /// future before it completes leaves the buffers with what was read so far. It holds the
    /// filesystem's unmount off until it completes or is dropped (see [`Filesystem::unmount`]).
    pub async fn read_vectored(
        &self,
        offset: u64,
        bufs: &mut [VolatileSliceMut<'_>],
    ) -> Result<usize> {
        let (stream, _open) = self.stream().await?;
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
    /// future before it completes leaves the file with what was written so far. It holds the
    /// filesystem's unmount off until it completes or is dropped (see [`Filesystem::unmount`]).
    pub async fn write_vectored(&self, offset: u64, bufs: &[VolatileSlice<'_>]) -> Result<usize> {
        let (stream, _open) = self.stream().await?;
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
            .run(async move {
                on_node!(node, |n| n.get_extended_attribute(name).await).map_err(xattr_errno)
            })
            .await
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
            .run(async move {
                on_node!(node, |n| n.set_extended_attribute(name, value, mode).await)
                    .map_err(xattr_errno)
            })
            .await
    }

    /// The names of the node's extended attributes.
    pub async fn listxattr(&self) -> Result<Vec<Vec<u8>>> {
        let volume = self.volume()?;
        let node = self.fx_node(&volume)?;
        volume
            .run(async move {
                on_node!(node, |n| n.list_extended_attributes().await).map_err(xattr_errno)
            })
            .await
    }

    /// Removes the node's extended attribute `name`. Fails with `ENODATA` if it has none of that
    /// name.
    pub async fn removexattr(&self, name: &[u8]) -> Result<()> {
        let volume = self.volume()?;
        let (node, name) = (self.fx_node(&volume)?, name.to_vec());
        volume
            .run(async move {
                on_node!(node, |n| n.remove_extended_attribute(name).await).map_err(xattr_errno)
            })
            .await
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
    use crate::kernel::page_size;
    use fuchsia_async::ScopeHandle;
    use futures::FutureExt as _;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use futures::task::ArcWake;
    use fxfs::object_store::transaction::{LockKey, lock_keys};
    use fxfs_platform::volume::{FxVolume, READ_AHEAD_SIZE};
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::task::{Context, Poll};

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

    /// Holds the lock of Fxfs's that a transaction changing the node `object_id` of `volume` takes,
    /// until the sender returned is sent on or dropped.
    async fn hold_lock(volume: &Volume, object_id: u64) -> oneshot::Sender<()> {
        let store = volume.fx_volume().unwrap().store().clone();
        let (locked, held) = oneshot::channel();
        let (release, released) = oneshot::channel::<()>();
        volume.image.executor.spawn(async move {
            let keys = lock_keys![LockKey::object(store.store_object_id(), object_id)];
            let transaction = store.new_transaction(keys, Default::default()).await.unwrap();
            let _ = locked.send(());
            let _ = released.await;
            drop(transaction);
        });
        held.await.unwrap();
        release
    }

    /// A handle to the scope of `volume`'s Fxfs volume, which has been given a task that ends only
    /// once the scope's shutdown wakes it: the handle's `on_no_tasks` completes once unmounting has
    /// shut the scope down, and its other tasks have ended. Fxfs's `ExecutionScope` has no
    /// `on_no_tasks`, as it waits for its tasks and guards both, and gives the handle only through
    /// a guard.
    async fn watch_shutdown(volume: &Volume) -> ScopeHandle {
        let scope = volume.fx_volume().unwrap().scope().try_active_guard().unwrap().to_handle();
        let (spawned, polled) = oneshot::channel();
        let mut spawned = Some(spawned);
        let task = std::future::poll_fn(move |_| match spawned.take() {
            Some(spawned) => {
                let _ = spawned.send(());
                Poll::Pending
            }
            None => Poll::Ready(()),
        });
        let spawner = scope.clone();
        volume.image.executor.run(async move { drop(spawner.spawn(task)) }).await;
        polled.await.unwrap();
        scope
    }

    /// Leaves `file`, of `volume`, dirty but with no connection, whose close would write it back:
    /// closes the file's connection, which writes the file back, then writes `data` at its start
    /// through the stream the connection gave, as a write under way as it closed would. Returns
    /// the stream, as such a write has it still.
    async fn dirty_unconnected(volume: &Volume, file: &Node, data: &[u8]) -> Arc<zx::Stream> {
        let connection = volume.with_held(&file.0, |held| held.connection.take()).unwrap();
        let (proxy, stream) = connection.as_ref().unwrap().file().unwrap();
        let (proxy, stream) = (Clone::clone(proxy), stream.clone());
        volume.image.executor.run(async move { proxy.close().await }).await.unwrap().unwrap();
        assert_eq!(write_stream(&stream, 0, data).await, Ok(data.len()));
        stream
    }

    /// Writes `data` at `offset` through `stream`, a file's, as the file's writes do.
    async fn write_stream(
        stream: &zx::Stream,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, zx::Status> {
        let bufs = [VolatileSlice::from(data).host_buffer()];
        zircon::syscalls::stream_writev_at_async(stream.raw_handle(), offset, &bufs).await
    }

    /// Polls `future`, a read's or a write's, once, which leaves it waiting on a page request, and
    /// waits until it is woken: until Fxfs's pager has answered the request, supplying the pages
    /// it asked for, or letting it dirty them. It has copied nothing to or from them yet.
    async fn woken(future: &mut (impl Future + Unpin)) {
        struct Woken(std::sync::Mutex<Option<oneshot::Sender<()>>>);
        impl ArcWake for Woken {
            fn wake_by_ref(woken: &Arc<Self>) {
                if let Some(sender) = woken.0.lock().unwrap().take() {
                    let _ = sender.send(());
                }
            }
        }
        let (sender, receiver) = oneshot::channel();
        let waker = futures::task::waker(Arc::new(Woken(std::sync::Mutex::new(Some(sender)))));
        let polled = future.poll_unpin(&mut Context::from_waker(&waker));
        assert!(polled.is_pending(), "it needed no page request");
        receiver.await.unwrap();
    }

    /// Whether the scope of `volume`, Fxfs's, has finished: shut down with no guard left, which
    /// unmounting waits for to write the volume back. Its `ExecutionScope` gives a guard under the
    /// lock it shuts the scope down under, so a shutdown under way is done once this asks: a scope
    /// that the shutdown found guarded, and has a guard still, has not finished.
    fn scope_finished(volume: &FxVolume) -> bool {
        volume.scope().try_active_guard().is_none()
    }

    /// Unmounting waits for an operation under way, as for the volume's fuchsia.io requests: the
    /// unmount shuts the scope of Fxfs's volume down, then waits for the scope to finish, with its
    /// guards gone, before it writes the volume back, and the operation holds one. An rmdir held
    /// by the lock of Fxfs's that it takes keeps the scope, shut down and with no tasks left, from
    /// finishing; let go, it removes the directory, and the unmount completes. Meanwhile, the
    /// image's volumes directory is free, as work under a guard may need it: dirtying a page under
    /// critical memory pressure does (`VolumesDirectory::report_pager_dirty`).
    #[test]
    fn unmounting_waits_for_an_operation_under_way() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, fs.root().inode().0).await;
            let mut rmdir = Box::pin(fs.root().rmdir(b"dir"));
            assert!(futures::poll!(&mut rmdir).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with an rmdir under way");
            // So the unmount waits for it still, with the volumes directory free.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under an rmdir");
            let volumes_directory = volume.image.volumes_directory().unwrap();
            assert!(
                volumes_directory.lock().now_or_never().is_some(),
                "the volumes directory is locked while the unmount waits for the scope"
            );

            release.send(()).unwrap();
            assert_eq!(rmdir.await, Ok(()));
            unmounted.await.expect("unmounting failed");
            drop((scope, fx_volume, volumes_directory, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            assert_eq!(fs.root().lookup(b"dir").await.err(), Some(Errno::ENOENT));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// An operation that found the volume mounted, but finds its scope finished as it runs its
    /// work, or as it begins to copy, a read or write, as it does if the whole unmount comes
    /// between the two, fails with `ENOTCONN`, as one does once unmounting begins, and changes
    /// nothing. The test shuts the scope down itself, as the unmount does, leaving the volume
    /// mounted. The file's page is dirty, so that a write to it, as a read of it, needs no pager.
    #[test]
    fn operations_refuse_a_volume_whose_scope_is_shut_down() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"written").await, Ok(7));
            // As the unmount shuts it down.
            let scope = fs.volume().fx_volume().unwrap().scope().clone();
            scope.shutdown();
            scope.wait().await;
            assert_eq!(fs.root().rmdir(b"dir").await, Err(Errno::ENOTCONN));
            assert_eq!(file.write(0, b"changed").await, Err(Errno::ENOTCONN));
            let mut contents = [0; 7];
            assert_eq!(file.read(0, &mut contents).await, Err(Errno::ENOTCONN));

            drop((scope, file));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            assert_eq!(fs.root().lookup(b"dir").await.unwrap().1.file_type, FileType::Directory);
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(&contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for writes under way, which hold their file open under a guard on the
    /// volume's scope while they copy (see [`Node::write_vectored`]), and writes back what they
    /// wrote: an overwrite of a file's page written back already, and a write extending another
    /// file at its end. Fxfs's pager let each dirty its page before the unmount began, and each
    /// copies to it only once the unmount is under way. The extending write was lost otherwise: it
    /// extends its file over its page only once it has copied, and the volume's last write-back
    /// leaves a dirty page past the file's end for a later one, which never comes. The writes are
    /// to files of their own, as the file's stream holds off a write extending it until those
    /// before it complete.
    #[test]
    fn unmounting_waits_for_writes_under_way() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (overwritten, _) = fs.root().create(b"overwritten", 0o644, OWNER).await.unwrap();
            let (extended, _) = fs.root().create(b"extended", 0o644, OWNER).await.unwrap();
            for file in [&overwritten, &extended] {
                assert_eq!(file.write(0, &vec![1; page]).await, Ok(page));
                file.fsync().await.unwrap();
            }
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let data = vec![2; page];
            let mut overwrite = Box::pin(overwritten.write(0, &data));
            woken(&mut overwrite).await;
            let mut extend = Box::pin(extended.write(page as u64, &data));
            woken(&mut extend).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with writes under way");
            // So the unmount waits for them still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under writes");

            assert_eq!(overwrite.await, Ok(page));
            assert_eq!(extend.await, Ok(page));
            unmounted.await.expect("unmounting failed");
            drop((overwritten, extended, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let written = [
                (&b"overwritten"[..], data.clone()),
                (b"extended", [vec![1; page], data].concat()),
            ];
            for (name, written) in written {
                let (file, attr) = fs.root().lookup(name).await.unwrap();
                assert_eq!(attr.size, written.len() as u64);
                let mut contents = vec![0; written.len()];
                assert_eq!(file.read(0, &mut contents).await, Ok(written.len()));
                assert_eq!(contents, written);
            }
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// A write under way holds the unmount off only until it is dropped, which is what lets the
    /// unmount complete once the write's caller has stopped polling it (see
    /// [`Filesystem::unmount`]): the file it holds open, and its guard, go with it, and the
    /// unmount completes, leaving the file as it was, as the write copied nothing. Fxfs's pager
    /// let the write dirty its page, written back already, before the unmount began.
    #[test]
    fn unmounting_completes_once_a_write_under_way_is_dropped() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, &vec![1; page]).await, Ok(page));
            file.fsync().await.unwrap();
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let data = vec![2; page];
            let mut write = Box::pin(file.write(0, &data));
            woken(&mut write).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a write under way");
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a write");

            drop(write);
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(attr.size, page as u64);
            let mut contents = vec![0; page];
            assert_eq!(file.read(0, &mut contents).await, Ok(page));
            assert_eq!(contents, vec![1; page]);
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// A write through a file's stream that holds no guard on the volume's scope, unlike the
    /// filesystem's own (see [`Node::write_vectored`]), and copies once the scope has finished,
    /// before the volume's last write-back reaches its file, is written back if its page is dirty
    /// already, and fails otherwise: what a write copies to a dirty page it copies under the lock
    /// of the file's VMO that the write-back takes to begin cleaning it, and to dirty a page it
    /// needs Fxfs's pager, which refuses once the scope has finished. The file is dirty and has no
    /// connection, whose close as the unmount begins would write it back, and the writes go
    /// through the stream its connection gave; the lock of Fxfs's that writing the file back
    /// takes holds the write-back off until they are done.
    #[test]
    fn writes_copying_once_the_scope_finishes_are_written_back_or_fail() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, &vec![1; 2 * page]).await, Ok(2 * page));
            let volume = fs.volume().clone();
            let stream = dirty_unconnected(&volume, &file, &vec![2; page]).await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(scope_finished(&fx_volume), "the scope is held");
            // The first page is dirty, the second clean.
            let data = vec![3; page];
            assert_eq!(write_stream(&stream, 0, &data).await, Ok(page));
            assert_eq!(write_stream(&stream, page as u64, &data).await, Err(zx::Status::BAD_STATE));
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under the lock");

            release.send(()).unwrap();
            unmounted.await.expect("unmounting failed");
            drop((file, stream, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; 2 * page];
            assert_eq!(file.read(0, &mut contents).await, Ok(2 * page));
            assert_eq!(contents, [vec![3; page], vec![1; page]].concat());
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a read under way, which holds its file open under a guard on the
    /// volume's scope while it copies (see [`Node::read_vectored`]), and Fxfs's pager supplies
    /// the pages it asks for meanwhile: the file is open though the unmount has closed it, and the
    /// scope's shutdown waits for the guard. Fxfs's pager reads in the whole window of
    /// `READ_AHEAD_SIZE` that a page it is asked for is in; the file's second window is in memory,
    /// its first and third not, so the read asks for the first before the unmount begins, and,
    /// once it has copied the second, for the third, as it alone holds the file open.
    #[test]
    fn unmounting_waits_for_a_read_under_way() {
        const WINDOW: usize = READ_AHEAD_SIZE as usize;
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let data: Vec<u8> = (0..3 * WINDOW).map(|at| (at / page) as u8).collect();
            assert_eq!(file.write(0, &data).await, Ok(data.len()));
            drop(file);
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            let mut byte = [0];
            assert_eq!(file.read(WINDOW as u64, &mut byte).await, Ok(1));
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let mut contents = vec![0; data.len()];
            let mut read = Box::pin(file.read(0, &mut contents));
            woken(&mut read).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a read under way");
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a read");

            assert_eq!(read.await, Ok(data.len()));
            assert_eq!(contents, data);
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a clone of the image writing the volume back, as for an operation:
    /// the clone writes each volume mounted as it begins back under a guard on its scope. One held
    /// by the lock of Fxfs's that writing back a dirty file takes keeps the scope from finishing;
    /// let go, it clones the image, file and all, and the unmount completes. The file has no
    /// connection, whose close as the unmount begins would wait for the lock too.
    #[test]
    fn unmounting_waits_for_a_clone_writing_the_volume_back() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let storage = Storage::create(dir.path().join("image"), 64 << 20).unwrap();
            let fs = Filesystem::format(storage).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            dirty_unconnected(&volume, &file, b"written").await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut clone = Box::pin(fs.clone_storage_to(dir.path().join("clone")));
            assert!(futures::poll!(&mut clone).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a clone writing it back");
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a clone");

            release.send(()).unwrap();
            let clone = clone.await.expect("cloning failed");
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            fs.close().await.unwrap().check().await.unwrap();
            let fs = Filesystem::mount(clone.check().await.unwrap()).await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a snapshot of the volume writing it back, as for an operation: the
    /// snapshot writes its source back, if it is mounted as the snapshot begins, under a guard on
    /// its scope. One held by the lock of Fxfs's that writing back a dirty file takes keeps the
    /// scope from finishing; let go, it snapshots the volume, file and all, once the unmount, which
    /// the snapshot waits for, completes. The file has no connection, whose close as the unmount
    /// begins would wait for the lock too.
    #[test]
    fn unmounting_waits_for_a_snapshot_writing_its_source_back() {
        block_on(async {
            let image = Image::format(Storage::memory(64 << 20)).await.unwrap();
            let fs = image.create_volume("source").await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            dirty_unconnected(&volume, &file, b"written").await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut snapshot = Box::pin(image.snapshot_volume("source", "snapshot"));
            assert!(futures::poll!(&mut snapshot).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(
                !scope_finished(&fx_volume),
                "the scope finished with a snapshot writing it back"
            );
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a snapshot");

            release.send(()).unwrap();
            unmounted.await.expect("unmounting failed");
            snapshot.await.expect("snapshotting failed");
            drop((file, scope, fx_volume, volume, fs));
            let fs = image.open_volume("snapshot").await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.unmount().await.unwrap();
            image.close().await.unwrap().check().await.unwrap();
        })
    }
}
