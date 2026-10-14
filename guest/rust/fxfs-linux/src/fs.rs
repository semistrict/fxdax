//! Fxfs, mounted on an extent-store device: the volume fxdax's images keep their files in, served
//! by object ID, which is the kernel's inode number.
//!
//! The kernel's VFS checks what it can before calling: names exist or not, types match, and
//! permissions allow. Each change is one of Fxfs's transactions, locking the objects it changes.

use crate::device::ExtentDevice;
use anyhow::{Error, anyhow, bail};
use fidl_fuchsia_io as fio;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::lock_keys;
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{ReplacedChild, replace_child};
use fxfs::object_store::transaction::{LockKey, Options};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, Directory, HandleOptions, NewChildStoreOptions,
    ObjectDescriptor, ObjectKey, ObjectKind, ObjectStore, ObjectValue, StoreOptions, Timestamp,
};
use std::sync::Arc;
use storage_device::DeviceHolder;

/// The volume fxdax's images keep a filesystem's files in (crates/fxdax's `VOLUME_NAME`).
const VOLUME: &str = "default";

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// An inode's attributes: `struct fxfs_attr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Attr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime_ns: u64,
    pub mtime_ns: u64,
    pub ctime_ns: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
}

/// Which attributes `setattr` sets: `FXFS_SET_*`.
pub const SET_MODE: u32 = 1 << 0;
pub const SET_UID: u32 = 1 << 1;
pub const SET_GID: u32 = 1 << 2;
pub const SET_SIZE: u32 = 1 << 3;
pub const SET_ATIME: u32 = 1 << 4;
pub const SET_MTIME: u32 = 1 << 5;

/// Attributes to set: `struct fxfs_setattr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SetAttr {
    pub valid: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime_ns: u64,
    pub mtime_ns: u64,
}

/// What a rename or unlink replaced: `FXFS_REPLACED_*`.
pub const REPLACED_NONE: u32 = 0;
/// A file or symlink, which other names may still link.
pub const REPLACED_OBJECT: u32 = 1;
/// A directory, which is gone.
pub const REPLACED_DIRECTORY: u32 = 2;

/// The filesystem's usage: `struct fxfs_statfs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatFs {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub files: u64,
}

/// A mounted filesystem.
pub struct Fxfs {
    filesystem: OpenFxFilesystem,
    volume: Arc<ObjectStore>,
}

impl Fxfs {
    /// Mounts the filesystem on `device`, and its volume. A device whose store holds nothing yet
    /// is formatted, with an empty volume, first.
    pub async fn mount(device: ExtentDevice) -> Result<Self, Error> {
        let format = device.is_empty().await?;
        let filesystem =
            FxFilesystemBuilder::new().format(format).open(DeviceHolder::new(device)).await?;
        let volume = async {
            let root_volume = root_volume(filesystem.clone()).await?;
            if format {
                root_volume.new_volume(VOLUME, NewChildStoreOptions::default()).await
            } else {
                root_volume.volume(VOLUME, StoreOptions::default()).await
            }
        }
        .await;
        match volume {
            Ok(volume) => Ok(Self { filesystem, volume }),
            Err(error) => {
                if let Err(close_error) = filesystem.close().await {
                    log::error!("failed to close the filesystem: {close_error:#}");
                }
                Err(error)
            }
        }
    }

    /// Unmounts the filesystem, writing back what is dirty.
    pub async fn unmount(self) -> Result<(), Error> {
        self.filesystem.close().await
    }

    /// The root directory's attributes.
    pub async fn root(&self) -> Result<Attr, Error> {
        self.attr(self.volume.root_directory_object_id()).await
    }

    /// The attributes of the object `ino`.
    pub async fn attr(&self, ino: u64) -> Result<Attr, Error> {
        let item = self.volume.tree().find(&ObjectKey::object(ino)).await?;
        let Some(ObjectValue::Object { kind, attributes }) = item.map(|item| item.value) else {
            bail!(FxfsError::NotFound);
        };
        let (kind_bits, nlink, size) = match kind {
            // Its entry in its parent, its own `.`, and its subdirectories' `..`s.
            ObjectKind::Directory { sub_dirs, .. } => (S_IFDIR, 2 + sub_dirs, 0),
            ObjectKind::File { refs } => (S_IFREG, refs, self.size(ino).await?),
            ObjectKind::Symlink { refs, link } | ObjectKind::EncryptedSymlink { refs, link } => {
                (S_IFLNK, refs, link.len() as u64)
            }
            ObjectKind::Graveyard => bail!(FxfsError::NotFound),
        };
        let (mode, uid, gid, rdev) = match attributes.posix_attributes {
            // A file's mode may say it is a special file: a FIFO, socket or device.
            Some(posix) if kind_bits == S_IFREG && posix.mode & S_IFMT != 0 => {
                (posix.mode, posix.uid, posix.gid, posix.rdev)
            }
            Some(posix) => (kind_bits | posix.mode & !S_IFMT, posix.uid, posix.gid, posix.rdev),
            None if kind_bits == S_IFDIR => (S_IFDIR | 0o755, 0, 0, 0),
            None if kind_bits == S_IFLNK => (S_IFLNK | 0o777, 0, 0, 0),
            None => (S_IFREG | 0o644, 0, 0, 0),
        };
        Ok(Attr {
            ino,
            size,
            blocks: attributes.allocated_size / 512,
            atime_ns: attributes.access_time.as_nanos(),
            mtime_ns: attributes.modification_time.as_nanos(),
            ctime_ns: attributes.change_time.as_nanos(),
            mode,
            nlink: nlink.try_into().unwrap_or(u32::MAX),
            uid,
            gid,
            rdev,
        })
    }

    /// The size of the file `ino`'s data.
    async fn size(&self, ino: u64) -> Result<u64, Error> {
        let key = ObjectKey::attribute(ino, AttributeId::DATA, AttributeKey::Attribute);
        match self.volume.tree().find(&key).await?.map(|item| item.value) {
            Some(
                ObjectValue::Attribute { size, .. } | ObjectValue::VerifiedAttribute { size, .. },
            ) => Ok(size),
            _ => Ok(0),
        }
    }

    /// The attributes of `name` in the directory `dir`.
    pub async fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        // Fxfs's names are UTF-8: no other name is in a directory.
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let (ino, _, _) =
            self.directory(dir).await?.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        self.attr(ino).await
    }

    /// Calls `emit` for the entries of the directory `dir` from the `index`th on, with each one's
    /// name, inode and type (as `S_IFMT` bits of a mode), until it returns false.
    pub async fn readdir(
        &self,
        dir: u64,
        index: u64,
        mut emit: impl FnMut(&str, u64, u32) -> bool,
    ) -> Result<(), Error> {
        let dir = self.directory(dir).await?;
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut entries = dir.iter(&mut merger).await?;
        let mut at = 0;
        while let Some((name, ino, descriptor)) = entries.get() {
            if at >= index && !emit(name, ino, file_type(descriptor)) {
                break;
            }
            at += 1;
            entries.advance().await?;
        }
        Ok(())
    }

    /// Reads up to `buf.len()` bytes of the file `ino` at `offset`, returning how many it read:
    /// fewer only at the end of the file.
    pub async fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let file = self.file(ino).await?;
        let end = file.get_size().min(offset.saturating_add(buf.len() as u64));
        if offset >= end {
            return Ok(0);
        }
        let data = file.read_bytes(offset..end).await?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    /// Writes `data` to the file `ino` at `offset`, and returns the file's attributes. The write
    /// sets the file's modification and change times, in the same transaction.
    pub async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr, Error> {
        let file = self.file(ino).await?;
        let mut buf = file.allocate_buffer(data.len()).await;
        buf.subslice_mut(..data.len()).copy_from_slice(data);
        let mut transaction = file.new_transaction().await?;
        file.txn_write(&mut transaction, offset, buf.subslice(..data.len())).await?;
        let now = Timestamp::now();
        let times = fio::MutableNodeAttributes {
            modification_time: Some(now.as_nanos()),
            ..Default::default()
        };
        file.update_attributes(&mut transaction, Some(&times), Some(now)).await?;
        transaction.commit().await?;
        self.attr(ino).await
    }

    /// Creates `name` in the directory `dir`, as `mode` says: a directory, or a file, which may be
    /// a special file (as Starnix and fxdax make them), owned by `uid` and `gid`.
    pub async fn create(
        &self,
        dir: u64,
        name: &[u8],
        mode: u32,
        rdev: u64,
        uid: u32,
        gid: u32,
    ) -> Result<Attr, Error> {
        let name = utf8_name(name)?;
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
        let attributes = fio::MutableNodeAttributes {
            mode: Some(mode),
            uid: Some(uid),
            gid: Some(gid),
            rdev: Some(rdev),
            ..Default::default()
        };
        let ino = if mode & S_IFMT == S_IFDIR {
            let child = dir.create_child_dir(&mut transaction, name).await?;
            child.handle().update_attributes(&mut transaction, Some(&attributes), None).await?;
            child.object_id()
        } else {
            let child = dir.create_child_file(&mut transaction, name).await?;
            child.update_attributes(&mut transaction, Some(&attributes), None).await?;
            child.object_id()
        };
        transaction.commit().await?;
        self.attr(ino).await
    }

    /// Creates the symlink `name` in the directory `dir`, to `target`.
    pub async fn symlink(&self, dir: u64, name: &[u8], target: &[u8]) -> Result<Attr, Error> {
        let name = utf8_name(name)?;
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
        let ino = dir.create_symlink(&mut transaction, target, name).await?;
        transaction.commit().await?;
        self.attr(ino).await
    }

    /// The target of the symlink `ino`.
    pub async fn readlink(&self, ino: u64) -> Result<Vec<u8>, Error> {
        self.volume.read_symlink(ino).await
    }

    /// Links the file or symlink `ino` into the directory `dir` as `name`.
    pub async fn link(&self, ino: u64, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        let name = utf8_name(name)?;
        let dir = self.directory(dir).await?;
        let descriptor = match self.attr(ino).await?.mode & S_IFMT {
            S_IFLNK => ObjectDescriptor::Symlink,
            S_IFDIR => bail!(FxfsError::NotFile),
            _ => ObjectDescriptor::File,
        };
        let mut transaction = self.transaction(&[dir.object_id(), ino]).await?;
        dir.insert_child(&mut transaction, name, ino, descriptor).await?;
        self.volume.adjust_refs(&mut transaction, ino, 1).await?;
        transaction.commit().await?;
        self.attr(ino).await
    }

    /// Removes `name` from the directory `dir`, and returns what it was (`REPLACED_*`). A file's
    /// last name gone, it waits in the graveyard until `evict`; a directory, which must be empty,
    /// is gone.
    pub async fn unlink(&self, dir: u64, name: &[u8]) -> Result<u32, Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let dir = self.directory(dir).await?;
        let context = dir.acquire_context_for_replace(None, name, true).await?;
        let mut transaction = context.transaction;
        if context.dst_id_and_descriptor.is_none() {
            bail!(FxfsError::NotFound);
        }
        let replaced = replace_child(&mut transaction, None, (&dir, name)).await?;
        transaction.commit().await?;
        Ok(self.replaced(replaced))
    }

    /// Moves `name` in the directory `dir` to `new_name` in `new_dir`, replacing what is there,
    /// and returns what it replaced (`REPLACED_*`).
    pub async fn rename(
        &self,
        dir: u64,
        name: &[u8],
        new_dir: u64,
        new_name: &[u8],
    ) -> Result<u32, Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let new_name = utf8_name(new_name)?;
        let src = self.directory(dir).await?;
        let dst = self.directory(new_dir).await?;
        let context = dst.acquire_context_for_replace(Some((&src, name)), new_name, false).await?;
        let mut transaction = context.transaction;
        if context.src_id_and_descriptor.is_none() {
            bail!(FxfsError::NotFound);
        }
        let replaced =
            replace_child(&mut transaction, Some((&src, name)), (&dst, new_name)).await?;
        transaction.commit().await?;
        Ok(self.replaced(replaced))
    }

    /// What `replaced` was, tombstoning a directory, which nothing holds once it is replaced.
    fn replaced(&self, replaced: ReplacedChild) -> u32 {
        match replaced {
            ReplacedChild::None => REPLACED_NONE,
            ReplacedChild::Object(_)
            | ReplacedChild::ObjectWithRemainingLinks(_)
            | ReplacedChild::Purged(_) => REPLACED_OBJECT,
            ReplacedChild::Directory(id) => {
                self.filesystem
                    .graveyard()
                    .queue_tombstone_object(self.volume.store_object_id(), id);
                REPLACED_DIRECTORY
            }
        }
    }

    /// Frees the object `ino`, which no name links any more, once the kernel is done with it.
    pub fn evict(&self, ino: u64) {
        self.filesystem.graveyard().queue_tombstone_object(self.volume.store_object_id(), ino);
    }

    /// Sets the attributes of the object `ino` that `set` says.
    pub async fn setattr(&self, ino: u64, set: &SetAttr) -> Result<Attr, Error> {
        let is_dir = self.attr(ino).await?.mode & S_IFMT == S_IFDIR;
        if set.valid & SET_SIZE != 0 {
            if is_dir {
                bail!(FxfsError::NotFile);
            }
            self.file(ino).await?.truncate(set.size).await?;
        }
        let flag = |bit: u32| set.valid & bit != 0;
        let attributes = fio::MutableNodeAttributes {
            mode: flag(SET_MODE).then_some(set.mode),
            uid: flag(SET_UID).then_some(set.uid),
            gid: flag(SET_GID).then_some(set.gid),
            access_time: flag(SET_ATIME).then_some(set.atime_ns),
            modification_time: flag(SET_MTIME).then_some(set.mtime_ns),
            ..Default::default()
        };
        if attributes != fio::MutableNodeAttributes::default() {
            let mut transaction = self.transaction(&[ino]).await?;
            let now = Some(Timestamp::now());
            if is_dir {
                let dir = self.directory(ino).await?;
                dir.handle().update_attributes(&mut transaction, Some(&attributes), now).await?;
            } else {
                let file = self.file(ino).await?;
                file.update_attributes(&mut transaction, Some(&attributes), now).await?;
            }
            transaction.commit().await?;
        }
        self.attr(ino).await
    }

    /// Makes what the filesystem holds durable.
    pub async fn sync(&self) -> Result<(), Error> {
        self.filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
    }

    /// The filesystem's usage.
    pub async fn statfs(&self) -> Result<StatFs, Error> {
        let block_size = self.filesystem.block_size().get();
        let blocks = self.filesystem.device().block_count();
        let used = self.filesystem.allocator().get_used_bytes().0 / block_size;
        Ok(StatFs {
            block_size,
            blocks,
            free_blocks: blocks.saturating_sub(used),
            files: self.volume.object_count(),
        })
    }

    async fn directory(&self, ino: u64) -> Result<Directory<ObjectStore>, Error> {
        Directory::open(&self.volume, ino).await
    }

    async fn file(&self, ino: u64) -> Result<DataObjectHandle<ObjectStore>, Error> {
        ObjectStore::open_object(&self.volume, ino, HandleOptions::default(), None).await
    }

    /// A transaction locking the objects `inos`.
    async fn transaction(
        &self,
        inos: &[u64],
    ) -> Result<fxfs::object_store::transaction::Transaction<'_>, Error> {
        let store = self.volume.store_object_id();
        let keys = match inos {
            [ino] => lock_keys![LockKey::object(store, *ino)],
            [a, b] => lock_keys![LockKey::object(store, *a), LockKey::object(store, *b)],
            _ => unreachable!("a transaction locks one or two objects"),
        };
        self.volume.new_transaction(keys, Options::default()).await
    }
}

/// `name`, a new entry's: Fxfs's names are UTF-8.
fn utf8_name(name: &[u8]) -> Result<&str, Error> {
    str::from_utf8(name)
        .map_err(|_| anyhow!(FxfsError::InvalidArgs).context("Fxfs's names are UTF-8"))
}

/// The `S_IFMT` bits of objects `descriptor` describes.
fn file_type(descriptor: &ObjectDescriptor) -> u32 {
    match descriptor {
        ObjectDescriptor::Directory => S_IFDIR,
        ObjectDescriptor::Symlink => S_IFLNK,
        _ => S_IFREG,
    }
}
