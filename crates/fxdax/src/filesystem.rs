// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A filesystem: an Fxfs volume, whose nodes are reached through handles to them.
//!
//! [`Filesystem`] serves the volume through Fxfs's POSIX layer (crates/fxfs-posix), the guest
//! kernel's too, as a VFS does: a [`Node`] handle holds its node as an inode is held, and its
//! operations check what the kernel's VFS checks before it calls the layer: that names are valid,
//! exist or not, and name nodes of the types the operation takes. A change of a directory's names
//! checks and changes them under the volume's lock of its names (`Volume::namespace`), as the VFS
//! locks a directory's. Times are kept as Linux keeps them: a write changes its file's
//! modification time, and a change of a directory's names its own.
//!
//! Every operation runs on Fxfs's executor, where it completes whoever waits for it. Reads and
//! writes copy through buffers of their own, as the memory they are given may be another's that
//! changes meanwhile ([`VolatileSlice`]).

use crate::error::{Errno, Error, Result};
use crate::executor::assert_not_on_executor;
use crate::image::{Compactions, Image, ImageShared};
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::node::*;
use crate::storage::Storage;
use crate::volume::{Hold, HostFxfs, Volume};
use fxfs::errors::FxfsError;
use fxfs::object_store::{ObjectStore, SetExtendedAttributeMode};
use fxfs_posix::ReaddirStart;
use std::future::Future;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;

/// The errno of `error`, an Fxfs operation's.
fn errno(error: anyhow::Error) -> Errno {
    Errno::of_fxfs(&error)
}

/// The errno of `error`, an extended attribute's operation's.
fn xattr_errno(error: anyhow::Error) -> Errno {
    Errno::of_xattr(&error)
}

/// The longest name a directory holds, as Linux's `NAME_MAX`.
const NAME_MAX: usize = 255;

/// How much of a write Fxfs writes in a transaction at most: a write longer is written a piece at
/// a time.
const WRITE_CHUNK: usize = 1 << 20;

/// `name`, a name of a directory's: valid, which `.`, `..`, and those holding `/` or NUL are not,
/// and in UTF-8, as Fxfs keeps names.
fn valid_name(name: &[u8]) -> Result<String> {
    if name.len() > NAME_MAX {
        return Err(Errno::ENAMETOOLONG);
    }
    if matches!(name, b"" | b"." | b"..") || name.iter().any(|&byte| byte == b'/' || byte == 0) {
        return Err(Errno::EINVAL);
    }
    Ok(std::str::from_utf8(name).map_err(|_| Errno::EINVAL)?.to_owned())
}

/// The attributes of the node `name` names in the directory `dir`, if it names one.
async fn find(fxfs: &HostFxfs, dir: u64, name: &str) -> Result<Option<fxfs_posix::Attr>> {
    match fxfs.lookup(dir, name.as_bytes()).await {
        Ok(attr) => Ok(Some(attr)),
        Err(error) if FxfsError::NotFound.matches(&error) => Ok(None),
        Err(error) => Err(errno(error)),
    }
}

/// The type of the node whose attributes are `attr`.
fn type_of(attr: &fxfs_posix::Attr) -> Result<FileType> {
    FileType::from_mode(attr.mode).ok_or(Errno::EIO)
}

/// The attributes of a node, from Fxfs's, on an image of `block_size` blocks.
fn attr(block_size: u32, attr: &fxfs_posix::Attr) -> Result<Attr> {
    Ok(Attr {
        inode: Inode(attr.ino),
        size: attr.size,
        blocks: attr.blocks,
        atime: time_from_nanos(attr.atime_ns),
        mtime: time_from_nanos(attr.mtime_ns),
        ctime: time_from_nanos(attr.ctime_ns),
        file_type: type_of(attr)?,
        perm: (attr.mode & PERM_MASK) as u16,
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        rdev: attr.rdev as u32,
        blksize: block_size,
    })
}

/// The times of a change at `now`: its modification time, and its status change time.
fn changed_at(now: u64) -> fxfs_posix::SetAttr {
    fxfs_posix::SetAttr {
        valid: fxfs_posix::SET_MTIME | fxfs_posix::SET_CTIME,
        mtime_ns: now,
        ctime_ns: now,
        ..Default::default()
    }
}

/// Now, in nanoseconds since the Unix epoch.
fn now() -> Result<u64> {
    nanos_from_time(std::time::SystemTime::now())
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

    /// The filesystem of the volume of `image` whose store is `store`, just mounted, which the
    /// image unmounts when it is closed: serves the volume's root directory, which it holds.
    pub(crate) fn new(image: Arc<ImageShared>, store: Arc<ObjectStore>) -> Self {
        let (volume, root) = Volume::new(image, store);
        let root = Node(Arc::new(root));
        Self { volume, root, backing: futures::lock::Mutex::new(Backing::Volume) }
    }

    /// Fails with `ENOTCONN` once the filesystem is unmounted, as every operation does.
    fn mounted(&self) -> Result<()> {
        match self.volume.is_mounted() {
            true => Ok(()),
            false => Err(Errno::ENOTCONN),
        }
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
        self.mounted()?;
        self.volume.image.clone_storage_to(path.as_ref()).await
    }

    /// The filesystem's usage.
    pub async fn statfs(&self) -> Result<StatFs> {
        let statfs =
            self.volume.run(move |fxfs| async move { fxfs.statfs().await.map_err(errno) }).await?;
        Ok(StatFs {
            block_size: statfs.block_size as u32,
            blocks: statfs.blocks,
            blocks_free: statfs.free_blocks,
            name_max: NAME_MAX as u32,
        })
    }

    /// What compacting the trees of the filesystem's image has cost since the image was mounted,
    /// for benchmarks to compare: the trees the image's volumes share, the allocator's among them.
    pub fn compactions(&self) -> Result<Compactions> {
        // As every operation does, it fails once the filesystem is unmounted.
        self.mounted()?;
        self.volume.image.compactions()
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
        let mounted = self.volume.is_mounted();
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

    /// The node's inode number, as Fxfs has it: its object ID.
    fn ino(&self) -> u64 {
        self.0.object_id()
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

    /// The node's type, which never changes, and its volume.
    fn file_type(&self) -> Result<(FileType, Arc<Volume>)> {
        let volume = self.volume()?;
        let file_type = volume.with_held(&self.0, |held| held.file_type)?;
        Ok((file_type, volume))
    }

    /// The volume of this directory: `ENOTDIR` if the node is not one.
    fn directory(&self) -> Result<Arc<Volume>> {
        match self.file_type()? {
            (FileType::Directory, volume) => Ok(volume),
            _ => Err(Errno::ENOTDIR),
        }
    }

    /// The volume of this file, whose contents are read and written: `EISDIR` if the node is a
    /// directory, and `EINVAL` if a symlink.
    fn file(&self) -> Result<Arc<Volume>> {
        match self.file_type()? {
            (FileType::Directory, _) => Err(Errno::EISDIR),
            (FileType::Symlink, _) => Err(Errno::EINVAL),
            (_, volume) => Ok(volume),
        }
    }

    /// Fails with `ENOENT` if the directory `dir` of `volume`, which is held, is removed: nothing
    /// is made in it any more.
    fn exists(volume: &Volume, dir: &Node) -> Result<()> {
        match volume.is_unlinked(&dir.0)? {
            true => Err(Errno::ENOENT),
            false => Ok(()),
        }
    }

    /// Looks up `name` in this directory, and returns a handle to the node it names and its
    /// attributes.
    pub async fn lookup(&self, name: &[u8]) -> Result<(Node, Attr)> {
        let volume = self.directory()?;
        let (dir, name) = (self.ino(), valid_name(name)?);
        let held = volume.clone();
        volume
            .run(move |fxfs| async move {
                // Held before the name can go (`Volume::namespace`).
                let _names = held.namespace.read().await;
                let found = find(&fxfs, dir, &name).await?.ok_or(Errno::ENOENT)?;
                handle(&held, &found, dir)
            })
            .await
    }

    /// The node's attributes.
    pub async fn getattr(&self) -> Result<Attr> {
        let volume = self.volume()?;
        let (ino, block_size) = (self.ino(), volume.block_size());
        let attr = volume
            .run(move |fxfs| async move { attr(block_size, &fxfs.attr(ino).await.map_err(errno)?) })
            .await?;
        self.linked(&volume, attr)
    }

    /// `attr`, the node's attributes, as Fxfs gives them, with no link if no name links it any
    /// more: Fxfs counts one for an object in its graveyard, which a VFS counts as none.
    fn linked(&self, volume: &Volume, mut attr: Attr) -> Result<Attr> {
        if volume.is_unlinked(&self.0)? {
            attr.nlink = 0;
        }
        Ok(attr)
    }

    /// Changes the node's attributes, and returns them.
    pub async fn setattr(&self, set: &SetAttr) -> Result<Attr> {
        let (file_type, volume) = self.file_type()?;
        let mut change = fxfs_posix::SetAttr::default();
        let now = now()?;
        if let Some(size) = set.size {
            match file_type {
                FileType::Directory => return Err(Errno::EISDIR),
                FileType::Symlink => return Err(Errno::EINVAL),
                _ => {}
            }
            // A truncation changes the file's contents, as a write does.
            change = changed_at(now);
            change.valid |= fxfs_posix::SET_SIZE;
            change.size = size;
        }
        if let Some(uid) = set.uid {
            change.valid |= fxfs_posix::SET_UID;
            change.uid = uid;
        }
        if let Some(gid) = set.gid {
            change.valid |= fxfs_posix::SET_GID;
            change.gid = gid;
        }
        if let Some(atime) = set.atime {
            change.valid |= fxfs_posix::SET_ATIME;
            change.atime_ns = nanos_from_time(atime.resolve())?;
        }
        if let Some(mtime) = set.mtime {
            change.valid |= fxfs_posix::SET_MTIME;
            change.mtime_ns = nanos_from_time(mtime.resolve())?;
        }
        let (perm, kill_suid_sgid) = (set.perm, set.kill_suid_sgid);
        let (ino, block_size) = (self.ino(), volume.block_size());
        let attr = volume
            .run(move |fxfs| async move {
                if perm.is_some() || kill_suid_sgid {
                    let mode = match file_type {
                        // A symlink's permissions are always all of them.
                        FileType::Symlink => 0o777,
                        _ => {
                            let mut mode = fxfs.attr(ino).await.map_err(errno)?.mode & PERM_MASK;
                            if kill_suid_sgid {
                                mode &= !S_ISUID;
                                if mode & S_IXGRP != 0 {
                                    mode &= !S_ISGID;
                                }
                            }
                            perm.map_or(mode, |perm| perm as u32 & PERM_MASK)
                        }
                    };
                    change.valid |= fxfs_posix::SET_MODE;
                    change.mode = file_type.mode_bits() | mode;
                }
                if change.valid != 0 {
                    fxfs.setattr(ino, &change).await.map_err(errno)?;
                }
                attr(block_size, &fxfs.attr(ino).await.map_err(errno)?)
            })
            .await?;
        self.linked(&volume, attr)
    }

    /// The target of this symlink.
    pub async fn readlink(&self) -> Result<Vec<u8>> {
        let volume = match self.file_type()? {
            (FileType::Symlink, volume) => volume,
            _ => return Err(Errno::EINVAL),
        };
        let ino = self.ino();
        volume.run(move |fxfs| async move { fxfs.readlink(ino).await.map_err(errno) }).await
    }

    /// Makes `name` in this directory with `make`, unless it names something already (`EEXIST`),
    /// and returns a handle to what it made, and its attributes.
    async fn make<F>(
        &self,
        name: &[u8],
        make: impl FnOnce(Arc<HostFxfs>, u64, String) -> F + Send + 'static,
    ) -> Result<(Node, Attr)>
    where
        F: Future<Output = anyhow::Result<fxfs_posix::Attr>> + Send + 'static,
    {
        let volume = self.directory()?;
        let (dir, name) = (self.ino(), valid_name(name)?);
        let (held, this) = (volume.clone(), self.clone());
        volume
            .run(move |fxfs| async move {
                let _names = held.namespace.write().await;
                Self::exists(&held, &this)?;
                if find(&fxfs, dir, &name).await?.is_some() {
                    return Err(Errno::EEXIST);
                }
                let made = make(fxfs, dir, name).await.map_err(errno)?;
                handle(&held, &made, dir)
            })
            .await
    }

    /// Creates `name` in this directory with `mode`, which says what it is, owned by `owner`.
    async fn create_node(
        &self,
        name: &[u8],
        mode: u32,
        rdev: u32,
        owner: Owner,
    ) -> Result<(Node, Attr)> {
        self.make(name, move |fxfs, dir, name| async move {
            let [made, _] = fxfs
                .create(dir, name.as_bytes(), mode, rdev.into(), owner.uid, owner.gid, &[])
                .await?;
            anyhow::Ok(made)
        })
        .await
    }

    /// Creates the regular file `name` in this directory with `perm`, owned by `owner`, and
    /// returns a handle to it and its attributes.
    pub async fn create(&self, name: &[u8], perm: u16, owner: Owner) -> Result<(Node, Attr)> {
        let mode = FileType::File.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(name, mode, 0, owner).await
    }

    /// Creates the directory `name` in this directory with `perm`, owned by `owner`, and returns a
    /// handle to it and its attributes.
    pub async fn mkdir(&self, name: &[u8], perm: u16, owner: Owner) -> Result<(Node, Attr)> {
        let mode = FileType::Directory.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(name, mode, 0, owner).await
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
        self.create_node(name, mode, rdev, owner).await
    }

    /// Creates the symlink `name` in this directory to `target`, owned by `owner`, and returns a
    /// handle to it and its attributes.
    pub async fn symlink(&self, name: &[u8], target: &[u8], owner: Owner) -> Result<(Node, Attr)> {
        let target = target.to_vec();
        self.make(name, move |fxfs, dir, name| async move {
            let [made, _] =
                fxfs.symlink(dir, name.as_bytes(), &target, owner.uid, owner.gid).await?;
            anyhow::Ok(made)
        })
        .await
    }

    /// Links the node into directory `dir` as `name`, and returns its attributes. Directories
    /// fail with `EPERM`, and a directory of another filesystem with `EXDEV`.
    pub async fn link(&self, dir: &Node, name: &[u8]) -> Result<Attr> {
        let volume = self.same_volume(dir)?;
        if self.file_type()?.0 == FileType::Directory {
            return Err(Errno::EPERM);
        }
        dir.directory()?;
        let (ino, dir_ino, name) = (self.ino(), dir.ino(), valid_name(name)?);
        let (held, this, dir) = (volume.clone(), self.clone(), dir.clone());
        let block_size = volume.block_size();
        volume
            .run(move |fxfs| async move {
                let _names = held.namespace.write().await;
                Self::exists(&held, &dir)?;
                // A node no name links any more is freed once it is let go of.
                Self::exists(&held, &this)?;
                if find(&fxfs, dir_ino, &name).await?.is_some() {
                    return Err(Errno::EEXIST);
                }
                let [linked, _] =
                    fxfs.link(ino, dir_ino, name.as_bytes(), false).await.map_err(errno)?;
                attr(block_size, &linked)
            })
            .await
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

    /// Removes `name`, as `rmdir` does if `directory`, and as `unlink` does otherwise.
    async fn remove(&self, name: &[u8], directory: bool) -> Result<()> {
        let volume = self.directory()?;
        let (dir, name) = (self.ino(), valid_name(name)?);
        let held = volume.clone();
        volume
            .run(move |fxfs| async move {
                let _names = held.namespace.write().await;
                let found = find(&fxfs, dir, &name).await?.ok_or(Errno::ENOENT)?;
                match (type_of(&found)? == FileType::Directory, directory) {
                    (true, false) => return Err(Errno::EISDIR),
                    (false, true) => return Err(Errno::ENOTDIR),
                    _ => {}
                }
                let (replaced, _) =
                    fxfs.unlink(dir, name.as_bytes(), found.ino).await.map_err(errno)?;
                held.unlinked(&fxfs, found.ino, replaced).await
            })
            .await
    }

    /// Renames `name` in this directory to `new_name` in directory `new_dir`, replacing what is
    /// there unless `flags` say not to, as rename(2) does: a directory only with an empty
    /// directory, and anything else only with something else, but a directory into itself never.
    /// A directory of another filesystem fails with `EXDEV`.
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
        self.directory()?;
        new_dir.directory()?;
        let (dir, name) = (self.ino(), valid_name(name)?);
        let (new_dir_ino, new_name) = (new_dir.ino(), valid_name(new_name)?);
        let no_replace = flags.contains(RenameFlags::NOREPLACE);
        let (held, new_dir) = (volume.clone(), new_dir.clone());
        volume
            .run(move |fxfs| async move {
                let _names = held.namespace.write().await;
                let moved = find(&fxfs, dir, &name).await?.ok_or(Errno::ENOENT)?;
                let replaced = find(&fxfs, new_dir_ino, &new_name).await?;
                // As Linux's renameat2 does, a rename that must not replace fails once the source
                // is found if the new name holds anything, the source itself too.
                if no_replace && replaced.is_some() {
                    return Err(Errno::EEXIST);
                }
                Self::exists(&held, &new_dir)?;
                let moves_directory = type_of(&moved)? == FileType::Directory;
                if let Some(replaced) = &replaced {
                    match (moves_directory, type_of(replaced)? == FileType::Directory) {
                        (true, false) => return Err(Errno::ENOTDIR),
                        (false, true) => return Err(Errno::EISDIR),
                        _ => {}
                    }
                    // Names of the same node: nothing to do.
                    if replaced.ino == moved.ino {
                        return Ok(());
                    }
                }
                if moves_directory && held.is_within(new_dir_ino, moved.ino)? {
                    return Err(Errno::EINVAL);
                }
                let replaced_ino = replaced.as_ref().map(|replaced| replaced.ino);
                let (replaced_kind, _) = fxfs
                    .rename(
                        dir,
                        name.as_bytes(),
                        moved.ino,
                        new_dir_ino,
                        new_name.as_bytes(),
                        replaced_ino,
                    )
                    .await
                    .map_err(errno)?;
                if moves_directory {
                    held.moved(moved.ino, new_dir_ino)?;
                }
                match replaced_ino {
                    Some(replaced) => held.unlinked(&fxfs, replaced, replaced_kind).await,
                    None => Ok(()),
                }
            })
            .await
    }

    /// The entries of this directory, but `.` and `..`, as it holds them now.
    pub async fn entries(&self) -> Result<Vec<DirEntry>> {
        let volume = self.directory()?;
        let dir = self.ino();
        volume
            .run(move |fxfs| async move {
                let mut entries = Vec::new();
                let listed = fxfs.readdir(dir, ReaddirStart::Index(0), |name, ino, kind| {
                    let file_type = match FileType::from_mode(kind) {
                        Some(FileType::Directory) => FileType::Directory,
                        Some(FileType::Symlink) => FileType::Symlink,
                        _ => FileType::File,
                    };
                    entries.push(DirEntry { inode: Inode(ino), file_type, name: name.into() });
                    true
                });
                listed.await.map_err(errno)?;
                Ok(entries)
            })
            .await
    }

    /// Reads from this file at `offset` into `buf`, and returns how much was read: less than asked
    /// only at the end of the file.
    pub async fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_vectored(offset, &mut [VolatileSliceMut::from(buf)]).await
    }

    /// Reads from this file at `offset` into `bufs` in turn, memory that may change meanwhile, and
    /// returns how much was read: less than asked only at the end of the file.
    ///
    /// The file's contents are read into a buffer of the read's own, which is copied into `bufs`
    /// once the read is done. Dropping the future before it completes leaves the buffers as they
    /// were.
    pub async fn read_vectored(
        &self,
        offset: u64,
        bufs: &mut [VolatileSliceMut<'_>],
    ) -> Result<usize> {
        let volume = self.file()?;
        let len = bufs.iter().map(VolatileSliceMut::len).sum::<usize>();
        let ino = self.ino();
        let data = volume
            .run(move |fxfs| async move {
                let mut data = vec![0; len];
                let read = fxfs.read_into(ino, offset, &mut [&mut data[..]]).await;
                data.truncate(read.map_err(errno)?);
                Ok(data)
            })
            .await?;
        let mut rest = &data[..];
        for buf in bufs {
            rest = &rest[buf.copy_from(rest)..];
        }
        Ok(data.len())
    }

    /// Writes `data` to this file at `offset`, extending it if that is past its end, and returns
    /// how much was written.
    pub async fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        self.write_vectored(offset, &[VolatileSlice::from(data)]).await
    }

    /// Writes `bufs` in turn to this file at `offset`, memory that may change meanwhile, and
    /// returns how much was written, changing the file's modification time.
    ///
    /// What `bufs` hold is copied, then written, a MiB at a time, in a transaction each.
    /// Dropping the future once the write has begun leaves it to complete.
    pub async fn write_vectored(&self, offset: u64, bufs: &[VolatileSlice<'_>]) -> Result<usize> {
        let volume = self.file()?;
        let mut data = vec![0; bufs.iter().map(VolatileSlice::len).sum::<usize>()];
        let mut at = 0;
        for buf in bufs {
            at += buf.copy_to(&mut data[at..]);
        }
        if data.is_empty() {
            return Ok(0);
        }
        offset.checked_add(data.len() as u64).ok_or(Errno::EFBIG)?;
        let (ino, now) = (self.ino(), now()?);
        volume
            .run(move |fxfs| async move {
                for (chunk, written) in data.chunks(WRITE_CHUNK).zip((0..).step_by(WRITE_CHUNK)) {
                    fxfs.write(ino, offset + written as u64, chunk).await.map_err(errno)?;
                }
                fxfs.setattr(ino, &changed_at(now)).await.map_err(errno)?;
                Ok(data.len())
            })
            .await
    }

    /// Allocates `len` bytes of this file at `offset`, extending it if they are past its end.
    pub async fn allocate(&self, offset: u64, len: u64) -> Result<()> {
        let volume = self.file()?;
        let end = offset.checked_add(len).filter(|_| len > 0).ok_or(Errno::EINVAL)?;
        let ino = self.ino();
        volume
            .run(move |fxfs| async move { fxfs.allocate(ino, offset..end).await.map_err(errno) })
            .await
    }

    /// Writes back the node's data and metadata, and for a directory, the filesystem's.
    pub async fn fsync(&self) -> Result<()> {
        let (file_type, volume) = self.file_type()?;
        volume
            .run(move |fxfs| async move {
                match file_type {
                    FileType::Directory => fxfs.sync().await,
                    FileType::Symlink => Ok(()),
                    _ => fxfs.fsync().await,
                }
                .map_err(errno)
            })
            .await
    }

    /// The value of the node's extended attribute `name`. Fails with `ENODATA` if it has none of
    /// that name.
    pub async fn getxattr(&self, name: &[u8]) -> Result<Vec<u8>> {
        let volume = self.volume()?;
        let (ino, name) = (self.ino(), name.to_vec());
        volume
            .run(move |fxfs| async move { fxfs.get_xattr(ino, &name).await.map_err(xattr_errno) })
            .await
    }

    /// Sets the node's extended attribute `name` to `value`, as `mode` says.
    pub async fn setxattr(&self, name: &[u8], value: &[u8], mode: XattrMode) -> Result<()> {
        let volume = self.volume()?;
        let (ino, name, value) = (self.ino(), name.to_vec(), value.to_vec());
        let mode = match mode {
            XattrMode::Set => SetExtendedAttributeMode::Set,
            XattrMode::Create => SetExtendedAttributeMode::Create,
            XattrMode::Replace => SetExtendedAttributeMode::Replace,
        };
        volume
            .run(move |fxfs| async move {
                fxfs.set_xattr(ino, &name, &value, mode).await.map_err(xattr_errno)?;
                // Written back at once, as the host's VFS keeps no inode to write it back with.
                fxfs.write_inode(ino, &Default::default()).await.map_err(xattr_errno)
            })
            .await
    }

    /// The names of the node's extended attributes.
    pub async fn listxattr(&self) -> Result<Vec<Vec<u8>>> {
        let volume = self.volume()?;
        let ino = self.ino();
        volume
            .run(move |fxfs| async move { fxfs.list_xattrs(ino).await.map_err(xattr_errno) })
            .await
    }

    /// Removes the node's extended attribute `name`. Fails with `ENODATA` if it has none of that
    /// name.
    pub async fn removexattr(&self, name: &[u8]) -> Result<()> {
        let volume = self.volume()?;
        let (ino, name) = (self.ino(), name.to_vec());
        volume
            .run(move |fxfs| async move {
                fxfs.remove_xattr(ino, &name).await.map_err(xattr_errno)?;
                fxfs.write_inode(ino, &Default::default()).await.map_err(xattr_errno)
            })
            .await
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").field("inode", &self.inode()).finish_non_exhaustive()
    }
}

/// A new handle to the node `found`, of `volume`, found in the directory `dir`, and its
/// attributes.
fn handle(volume: &Arc<Volume>, found: &fxfs_posix::Attr, dir: u64) -> Result<(Node, Attr)> {
    let attr = attr(volume.block_size(), found)?;
    let hold = volume.hold(found.ino, attr.file_type, Some(dir))?;
    Ok((Node(Arc::new(hold)), attr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use fxfs::object_store::transaction::{LockKey, lock_keys};
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

    /// A directory held holds the directory it is in, which a handle to it alone keeps, until the
    /// last handle to it, or to a directory in it, drops: a rename moves the hold.
    #[test]
    fn held_directories_hold_their_parents() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let root = fs.root().inode().0;
            let held = || fs.volume().held();
            let (outer, _) = fs.root().mkdir(b"outer", 0o755, OWNER).await.unwrap();
            let (inner, _) = outer.mkdir(b"inner", 0o755, OWNER).await.unwrap();
            let (other, _) = fs.root().mkdir(b"other", 0o755, OWNER).await.unwrap();
            let ids = (outer.inode().0, inner.inode().0, other.inode().0);
            drop(outer);
            // The inner directory holds the outer one, which no handle holds.
            assert_eq!(held(), BTreeMap::from([(root, 1), (ids.0, 0), (ids.1, 1), (ids.2, 1)]));
            let (outer, _) = fs.root().lookup(b"outer").await.unwrap();
            outer.rename(b"inner", &other, b"inner", RenameFlags::empty()).await.unwrap();
            drop(outer);
            assert_eq!(held(), BTreeMap::from([(root, 1), (ids.1, 1), (ids.2, 1)]));
            drop(other);
            assert_eq!(held(), BTreeMap::from([(root, 1), (ids.1, 1), (ids.2, 0)]));
            drop(inner);
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
        let fxfs = volume.fxfs().unwrap();
        let store = fxfs.filesystem().object_manager().store(fxfs.volume_id()).unwrap();
        drop(fxfs);
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

    /// Unmounting waits for an operation under way, which runs on Fxfs's executor: an rmdir held
    /// by the lock of Fxfs's that it takes keeps the unmount waiting, with the volume refusing
    /// new operations; let go, it removes the directory, and the unmount completes.
    #[test]
    fn unmounting_waits_for_an_operation_under_way() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
            let volume = fs.volume().clone();

            let release = hold_lock(&volume, fs.root().inode().0).await;
            let mut rmdir = Box::pin(fs.root().rmdir(b"dir"));
            assert!(futures::poll!(&mut rmdir).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under an rmdir");
            assert_eq!(fs.root().getattr().await, Err(Errno::ENOTCONN));

            release.send(()).unwrap();
            assert_eq!(rmdir.await, Ok(()));
            unmounted.await.expect("unmounting failed");
            drop(volume);
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            assert_eq!(fs.root().lookup(b"dir").await.err(), Some(Errno::ENOENT));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for writes under way, and writes back what they wrote: an overwrite of a
    /// file's block, and a write extending another file at its end. Each is held by the lock of
    /// Fxfs's on its file that its transaction takes, and completes once let go of.
    #[test]
    fn unmounting_waits_for_writes_under_way() {
        const BLOCK: usize = Storage::BLOCK_SIZE as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (overwritten, _) = fs.root().create(b"overwritten", 0o644, OWNER).await.unwrap();
            let (extended, _) = fs.root().create(b"extended", 0o644, OWNER).await.unwrap();
            for file in [&overwritten, &extended] {
                assert_eq!(file.write(0, &[1; BLOCK]).await, Ok(BLOCK));
            }
            let volume = fs.volume().clone();

            let data = [2; BLOCK];
            let releases = [
                hold_lock(&volume, overwritten.inode().0).await,
                hold_lock(&volume, extended.inode().0).await,
            ];
            let mut overwrite = Box::pin(overwritten.write(0, &data));
            assert!(futures::poll!(&mut overwrite).is_pending());
            let mut extend = Box::pin(extended.write(BLOCK as u64, &data));
            assert!(futures::poll!(&mut extend).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under writes");

            for release in releases {
                release.send(()).unwrap();
            }
            assert_eq!(overwrite.await, Ok(BLOCK));
            assert_eq!(extend.await, Ok(BLOCK));
            unmounted.await.expect("unmounting failed");
            drop((overwritten, extended, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let written =
                [(&b"overwritten"[..], data.to_vec()), (b"extended", [[1; BLOCK], data].concat())];
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

    /// A write given up on, its future dropped once it has begun, completes on Fxfs's executor
    /// regardless, before the unmount writes the volume back: what it wrote is kept.
    #[test]
    fn a_write_given_up_on_completes_before_the_unmount() {
        const BLOCK: usize = Storage::BLOCK_SIZE as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, &[1; BLOCK]).await, Ok(BLOCK));
            let volume = fs.volume().clone();

            let release = hold_lock(&volume, file.inode().0).await;
            let data = [2; BLOCK];
            let mut write = Box::pin(file.write(0, &data));
            assert!(futures::poll!(&mut write).is_pending());
            drop(write);
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a write");

            release.send(()).unwrap();
            unmounted.await.expect("unmounting failed");
            drop((file, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = [0; BLOCK];
            assert_eq!(file.read(0, &mut contents).await, Ok(BLOCK));
            assert_eq!(contents, data);
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Names are refused as a VFS refuses them: too long, or `.`, `..`, empty, or holding `/` or
    /// NUL, or not in UTF-8, which Fxfs keeps names in.
    #[test]
    fn invalid_names_are_refused() {
        assert_eq!(valid_name(&[b'a'; 256]), Err(Errno::ENAMETOOLONG));
        assert_eq!(valid_name(&[b'a'; 255]), Ok("a".repeat(255)));
        for name in [&b""[..], b".", b"..", b"a/b", b"a\0b", b"\xff"] {
            assert_eq!(valid_name(name), Err(Errno::EINVAL), "{name:?}");
        }
    }
}
