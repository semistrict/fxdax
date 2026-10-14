//! Fxfs, mounted on an extent-store device: the volume fxdax's images keep their files in, served
//! by object ID, which is the kernel's inode number.
//!
//! The kernel's VFS checks what it can before calling: names exist or not, types match, and
//! permissions allow. Each change is one of Fxfs's transactions, locking the objects it changes.

use crate::device::{Dax, ExtentDevice};
use anyhow::{Error, anyhow, bail, ensure};
use fidl_fuchsia_io as fio;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions, TruncateGuard};
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{MutableAttributesInternal, ReplacedChild, replace_child};
use fxfs::object_store::journal::{DEFAULT_RECLAIM_SIZE, JournalOptions, LazyFlush};
use fxfs::object_store::transaction::{LockKey, LockKeys, Options, Transaction};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, Directory, FileExtent, HandleOptions,
    NewChildStoreOptions,
    MAX_XATTR_NAME_SIZE, MAX_XATTR_VALUE_SIZE, ObjectDescriptor, ObjectKey, ObjectKind,
    ObjectStore, ObjectValue, SetExtendedAttributeMode, StoreObjectHandle, StoreOptions,
    Timestamp,
};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_device::{DeviceHolder, SealedExtent};

/// The volume fxdax's images keep a filesystem's files in (crates/fxdax's `VOLUME_NAME`).
const VOLUME: &str = "default";

/// When the journal is written, but for syncs: metadata operations stay in memory, as ext4's do
/// between its commits.
const LAZY_FLUSH: LazyFlush = LazyFlush { bytes: 128 << 10, delay: Duration::from_secs(5) };

/// The share of memory, and of the device, the journal takes, at most `JOURNAL_MAX`: Fxfs flushes
/// its trees, compacting their layers, as each half of the journal fills, and keeps what the
/// journal holds in their in-memory layers meanwhile. Upstream's (`DEFAULT_RECLAIM_SIZE`, 512 KiB)
/// has it compact after each few thousand metadata operations; ext4's journal is tens of MiB.
const JOURNAL_SHARE: u64 = 32;
const JOURNAL_MAX: u64 = 64 << 20;

/// The share of memory each store's cache of objects' records takes (`object_cache_items`), at
/// about `CACHED_OBJECT_SIZE` bytes each: each use of an inode's attributes not cached looks its
/// record up in the store's tree.
const CACHE_SHARE: u64 = 64;
const CACHED_OBJECT_SIZE: u64 = 256;

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
pub const SET_CTIME: u32 = 1 << 6;

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
    pub ctime_ns: u64,
}

/// What a rename or unlink replaced: `FXFS_REPLACED_*`.
pub const REPLACED_NONE: u32 = 0;
/// A file or symlink, which other names may still link.
pub const REPLACED_OBJECT: u32 = 1;
/// A directory, which is gone.
pub const REPLACED_DIRECTORY: u32 = 2;

/// Where `readdir` starts: at an index into the directory's entries, or after the entry with a
/// name, which removing entries before it doesn't move.
pub enum ReaddirStart<'a> {
    Index(u64),
    After(&'a [u8]),
}

/// The filesystem's usage: `struct fxfs_statfs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatFs {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub files: u64,
}

/// A piece of a file's data, which a DAX window maps: `struct fxfs_dax_extent`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DaxExtent {
    /// Where in the file, and how long.
    pub offset: u64,
    pub len: u64,
    /// The extent, and where in it.
    pub branch: u64,
    pub sequence: u64,
    pub extent_offset: u64,
    /// Whether the extent is open, which only a writable mapping maps.
    pub open: u32,
    pub reserved: u32,
}

impl DaxExtent {
    fn new(extent: &FileExtent, open: bool) -> Self {
        let device_range = extent.device_range();
        Self {
            offset: extent.logical_offset(),
            len: extent.length(),
            branch: device_range.extent.branch,
            sequence: device_range.extent.sequence,
            extent_offset: device_range.range.start,
            open: open.into(),
            reserved: 0,
        }
    }
}

/// A mounted filesystem.
pub struct Fxfs {
    filesystem: OpenFxFilesystem,
    volume: Arc<ObjectStore>,
    dax: Dax,
    /// The handles of the inodes the kernel has, opened as first needed and closed as the kernel
    /// evicts them (`evict`): one for each object, as Fxfs has handles.
    directories: Mutex<HashMap<u64, Arc<Directory<ObjectStore>>>>,
    files: Mutex<HashMap<u64, Arc<DataObjectHandle<ObjectStore>>>>,
    /// The changes to inodes' extended attributes not yet written back: values set, or `None`,
    /// removed. As the kernel keeps an inode's attributes, and writes them back with them
    /// (`write_inode`), as ext4's journal does.
    xattrs: Mutex<HashMap<u64, BTreeMap<Vec<u8>, Option<Vec<u8>>>>>,
}

impl Fxfs {
    /// Mounts the filesystem on `device`, and its volume, sizing what it keeps in memory by the
    /// kernel's `memory` bytes. A device whose store holds nothing yet is formatted, with an empty
    /// volume, first.
    pub async fn mount(device: ExtentDevice, memory: u64) -> Result<Self, Error> {
        let format = device.is_empty().await?;
        let dax = device.dax();
        let reclaim_size = (memory.min(device.capacity()) / JOURNAL_SHARE)
            .clamp(DEFAULT_RECLAIM_SIZE, JOURNAL_MAX);
        let journal =
            JournalOptions { reclaim_size, lazy_flush: Some(LAZY_FLUSH), ..Default::default() };
        let filesystem = FxFilesystemBuilder::new()
            .format(format)
            .object_cache_items((memory / CACHE_SHARE / CACHED_OBJECT_SIZE) as usize)
            .journal_options(journal)
            .open(DeviceHolder::new(device))
            .await?;
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
            Ok(volume) => Ok(Self {
                filesystem,
                volume,
                dax,
                directories: Mutex::new(HashMap::new()),
                files: Mutex::new(HashMap::new()),
                xattrs: Mutex::new(HashMap::new()),
            }),
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
        let object = self.volume.tree().find(&ObjectKey::object(ino)).await?;
        self.attr_of(ino, object.map(|item| item.value), None).await
    }

    /// `ino`'s attributes as `transaction` leaves them: those it changes, which needn't be read,
    /// and the rest as they are.
    async fn attr_after(&self, transaction: &Transaction<'_>, ino: u64) -> Result<Attr, Error> {
        let store = self.volume.store_object_id();
        let changed = |key| {
            transaction.get_object_mutation(store, key).map(|mutation| mutation.item.value.clone())
        };
        let object = match changed(ObjectKey::object(ino)) {
            Some(object) => Some(object),
            None => self.volume.tree().find(&ObjectKey::object(ino)).await?.map(|item| item.value),
        };
        let size = changed(data_key(ino)).map(|data| data_size(Some(data)));
        self.attr_of(ino, object, size).await
    }

    /// The attributes of `ino`, which `object` describes, and whose data's size is `size`, if
    /// known.
    async fn attr_of(
        &self,
        ino: u64,
        object: Option<ObjectValue>,
        size: Option<u64>,
    ) -> Result<Attr, Error> {
        let Some(ObjectValue::Object { kind, attributes }) = object else {
            bail!(FxfsError::NotFound);
        };
        let (kind_bits, nlink, size) = match kind {
            // Its entry in its parent, its own `.`, and its subdirectories' `..`s.
            ObjectKind::Directory { sub_dirs, .. } => (S_IFDIR, 2 + sub_dirs, 0),
            ObjectKind::File { refs } => {
                let size = match size {
                    Some(size) => size,
                    None => self.size(ino).await?,
                };
                (S_IFREG, refs, size)
            }
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
        Ok(data_size(self.volume.tree().find(&data_key(ino)).await?.map(|item| item.value)))
    }

    /// The attributes of `name` in the directory `dir`.
    pub async fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        // Fxfs's names are UTF-8: no other name is in a directory.
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let (ino, _, _) =
            self.directory(dir).await?.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        self.attr(ino).await
    }

    /// Calls `emit` for the entries of the directory `dir` from `start` on, with each one's name,
    /// inode and type (as `S_IFMT` bits of a mode), until it returns false.
    pub async fn readdir(
        &self,
        dir: u64,
        start: ReaddirStart<'_>,
        mut emit: impl FnMut(&str, u64, u32) -> bool,
    ) -> Result<(), Error> {
        let dir = self.directory(dir).await?;
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let (mut entries, skip, after) = match start {
            ReaddirStart::Index(index) => (dir.iter(&mut merger).await?, index, None),
            // The entries in order after `name`, which may since have been removed.
            ReaddirStart::After(name) => {
                let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::InvalidArgs))?;
                (dir.iter_from(&mut merger, name).await?, 0, Some(name))
            }
        };
        let mut at = 0;
        while let Some((name, ino, descriptor)) = entries.get() {
            if at >= skip && Some(name) != after && !emit(name, ino, file_type(descriptor)) {
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

    /// Writes `data` to the file `ino` at `offset`, growing the file if it ends before the data
    /// does. The kernel keeps the file's times (`setattr`).
    pub async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
        let file = self.file(ino).await?;
        let mut buf = file.allocate_buffer(data.len()).await;
        buf.subslice_mut(..data.len()).copy_from_slice(data);
        file.write_or_append(Some(offset), buf.subslice(..data.len())).await?;
        Ok(())
    }

    /// The device, as C has it, for DAX (`struct fxfs_device`).
    pub fn device(&self) -> *mut core::ffi::c_void {
        self.dax.device()
    }

    /// The pieces of the file `ino`'s data in `range`, in order, which a DAX window maps it from.
    pub async fn dax_extents(&self, ino: u64, range: Range<u64>) -> Result<Vec<DaxExtent>, Error> {
        let file = self.file(ino).await?;
        let extents = file.extents_in(range).await?;
        Ok(extents.iter().map(|(extent, open)| DaxExtent::new(extent, *open)).collect())
    }

    /// Makes room to write `range` of the file `ino` through a DAX window, without writing it, and
    /// makes the file `size` long if it is shorter, in one transaction: returns the pieces holding
    /// the range's blocks, which are open, for the caller to store the data in before the next
    /// seal (`DataObjectHandle::txn_map_write`, as `keep` says).
    pub async fn dax_map_write(
        &self,
        ino: u64,
        range: Range<u64>,
        size: u64,
        keep: bool,
    ) -> Result<Vec<DaxExtent>, Error> {
        loop {
            match self.try_dax_map_write(ino, range.clone(), size, keep).await {
                // A seal came between the writes copying the first and last blocks' surroundings
                // to an extent: the transaction is dropped, and its extents with it.
                Err(error) if error.chain().any(|e| e.downcast_ref::<SealedExtent>().is_some()) => {
                }
                result => return result,
            }
        }
    }

    async fn try_dax_map_write(
        &self,
        ino: u64,
        range: Range<u64>,
        size: u64,
        keep: bool,
    ) -> Result<Vec<DaxExtent>, Error> {
        let file = self.file(ino).await?;
        let mut transaction = file.new_transaction().await?;
        let extents = file.txn_map_write(&mut transaction, range, keep).await?;
        // The transaction locks the file: its size is as it was.
        if size > file.get_size() {
            file.txn_update_size(&mut transaction, size, None).await?;
        }
        transaction.commit().await?;
        for extent in &extents {
            self.dax.opened(extent.device_range().extent);
        }
        Ok(extents.iter().map(|extent| DaxExtent::new(extent, true)).collect())
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
    ) -> Result<[Attr; 2], Error> {
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
        self.commit_new(transaction, ino, dir.object_id()).await
    }

    /// Creates the symlink `name` in the directory `dir`, to `target`, owned by `uid` and `gid`.
    pub async fn symlink(
        &self,
        dir: u64,
        name: &[u8],
        target: &[u8],
        uid: u32,
        gid: u32,
    ) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
        let ino = dir.create_symlink(&mut transaction, target, name).await?;
        let owner = fio::MutableNodeAttributes {
            mode: Some(S_IFLNK | 0o777),
            uid: Some(uid),
            gid: Some(gid),
            ..Default::default()
        };
        self.handle(ino).update_attributes(&mut transaction, Some(&owner), None).await?;
        self.commit_new(transaction, ino, dir.object_id()).await
    }

    /// Commits `transaction`, which made `ino` in directory `dir`: their attributes.
    async fn commit_new(
        &self,
        transaction: Transaction<'_>,
        ino: u64,
        dir: u64,
    ) -> Result<[Attr; 2], Error> {
        let attrs =
            [self.attr_after(&transaction, ino).await?, self.attr_after(&transaction, dir).await?];
        transaction.commit().await?;
        Ok(attrs)
    }

    /// The target of the symlink `ino`.
    pub async fn readlink(&self, ino: u64) -> Result<Vec<u8>, Error> {
        self.volume.read_symlink(ino).await
    }

    /// Links the file or symlink `ino` into the directory `dir` as `name`.
    pub async fn link(&self, ino: u64, dir: u64, name: &[u8]) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        let dir = self.directory(dir).await?;
        let object = self.volume.tree().find(&ObjectKey::object(ino)).await?;
        let descriptor = match object.map(|item| item.value) {
            Some(ObjectValue::Object {
                kind: ObjectKind::Symlink { .. } | ObjectKind::EncryptedSymlink { .. },
                ..
            }) => ObjectDescriptor::Symlink,
            Some(ObjectValue::Object { kind: ObjectKind::File { .. }, .. }) => {
                ObjectDescriptor::File
            }
            Some(ObjectValue::Object { kind: ObjectKind::Directory { .. }, .. }) => {
                bail!(FxfsError::NotFile)
            }
            _ => bail!(FxfsError::NotFound),
        };
        let mut transaction = self.transaction(&[dir.object_id(), ino]).await?;
        dir.insert_child(&mut transaction, name, ino, descriptor).await?;
        self.volume.adjust_refs(&mut transaction, ino, 1).await?;
        // A new link is a change to the object's status.
        let now = Some(Timestamp::now());
        self.handle(ino).update_attributes(&mut transaction, None, now).await?;
        let attrs = [
            self.attr_after(&transaction, ino).await?,
            self.attr_after(&transaction, dir.object_id()).await?,
        ];
        transaction.commit().await?;
        Ok(attrs)
    }

    /// Removes `name`, which links `ino`, from the directory `dir`, and returns what it was
    /// (`REPLACED_*`). A file's last name gone, it waits in the graveyard until `evict`; a
    /// directory, which must be empty, is gone.
    pub async fn unlink(&self, dir: u64, name: &[u8], ino: u64) -> Result<(u32, Attr), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let dir = self.directory(dir).await?;
        // Removing a name is what lets a full filesystem free space.
        let options = Options { borrow_metadata_space: true, ..Default::default() };
        let (_guard, mut transaction) =
            self.replace_transaction(&[dir.object_id()], Some(ino), options).await?;
        let replaced = replace_child(&mut transaction, None, (&dir, name)).await?;
        check_replaced(&replaced, Some(ino))?;
        let dir_attr = self.attr_after(&transaction, dir.object_id()).await?;
        transaction.commit().await?;
        Ok((self.replaced(replaced), dir_attr))
    }

    /// Moves `name`, which links `ino`, in the directory `dir` to `new_name` in `new_dir`,
    /// replacing what is there, `new_ino` if anything, and returns what it replaced
    /// (`REPLACED_*`).
    pub async fn rename(
        &self,
        dir: u64,
        name: &[u8],
        ino: u64,
        new_dir: u64,
        new_name: &[u8],
        new_ino: Option<u64>,
    ) -> Result<(u32, [Attr; 3]), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let new_name = utf8_name(new_name)?;
        let src = self.directory(dir).await?;
        let dst = self.directory(new_dir).await?;
        let (_guard, mut transaction) =
            self.replace_transaction(&[dir, new_dir, ino], new_ino, Options::default()).await?;
        let replaced =
            replace_child(&mut transaction, Some((&src, name)), (&dst, new_name)).await?;
        check_replaced(&replaced, new_ino)?;
        let attrs = [
            self.attr_after(&transaction, ino).await?,
            self.attr_after(&transaction, dir).await?,
            self.attr_after(&transaction, new_dir).await?,
        ];
        transaction.commit().await?;
        Ok((self.replaced(replaced), attrs))
    }

    /// Exchanges `name` in the directory `dir` and `new_name` in `new_dir`, which both exist
    /// (RENAME_EXCHANGE): each name then links what the other did, in one transaction.
    pub async fn exchange(
        &self,
        dir: u64,
        name: &[u8],
        new_dir: u64,
        new_name: &[u8],
    ) -> Result<[Attr; 4], Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let new_name = str::from_utf8(new_name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let (src, dst) = (self.directory(dir).await?, self.directory(new_dir).await?);
        loop {
            let (a, a_kind, _) = src.lookup(name).await?.ok_or(FxfsError::NotFound)?;
            let (b, b_kind, _) = dst.lookup(new_name).await?.ok_or(FxfsError::NotFound)?;
            let mut transaction = self.transaction(&[dir, new_dir, a, b]).await?;
            // The names may have changed before the locks were taken: look again under them.
            let still = |found: Option<(u64, ObjectDescriptor, bool)>, ino| {
                found.is_some_and(|(found, _, _)| found == ino)
            };
            if !still(src.lookup(name).await?, a) || !still(dst.lookup(new_name).await?, b) {
                continue;
            }
            // insert_child counts a directory linked in as a new subdirectory, of a directory
            // that loses one here.
            src.insert_child(&mut transaction, name, b, b_kind.clone()).await?;
            dst.insert_child(&mut transaction, new_name, a, a_kind.clone()).await?;
            for (directory, kind) in [(&src, &a_kind), (&dst, &b_kind)] {
                if *kind == ObjectDescriptor::Directory {
                    let less = MutableAttributesInternal::new(-1, None, None, None);
                    directory
                        .update_dir_attributes_internal(
                            &mut transaction,
                            directory.object_id(),
                            less,
                        )
                        .await?;
                }
            }
            let now = Some(Timestamp::now());
            for ino in [a, b] {
                self.handle(ino).update_attributes(&mut transaction, None, now).await?;
            }
            let mut attrs = [Attr::default(); 4];
            for (attr, ino) in attrs.iter_mut().zip([a, b, dir, new_dir]) {
                *attr = self.attr_after(&transaction, ino).await?;
            }
            transaction.commit().await?;
            return Ok(attrs);
        }
    }

    /// The value of the extended attribute `name` of the object `ino`.
    pub async fn get_xattr(&self, ino: u64, name: &[u8]) -> Result<Vec<u8>, Error> {
        match self.pending_xattr(ino, name) {
            Some(Some(value)) => Ok(value),
            Some(None) => bail!(FxfsError::NotFound),
            None => self.handle(ino).get_extended_attribute(name.to_vec()).await,
        }
    }

    /// The change to `ino`'s extended attribute `name` not yet written back, if any.
    fn pending_xattr(&self, ino: u64, name: &[u8]) -> Option<Option<Vec<u8>>> {
        self.xattrs.lock().unwrap().get(&ino)?.get(name).cloned()
    }

    /// Whether `ino` has the extended attribute `name`, as the kernel has it.
    async fn has_xattr(&self, ino: u64, name: &[u8]) -> Result<bool, Error> {
        if let Some(pending) = self.pending_xattr(ino, name) {
            return Ok(pending.is_some());
        }
        let key = ObjectKey::extended_attribute(ino, name.to_vec());
        Ok(self.volume.tree().find(&key).await?.is_some())
    }

    /// Records a change to `ino`'s extended attribute `name`, which `write_inode` writes back.
    fn change_xattr(&self, ino: u64, name: &[u8], value: Option<Vec<u8>>) {
        self.xattrs.lock().unwrap().entry(ino).or_default().insert(name.to_vec(), value);
    }

    /// Writes back the changes to `ino`'s extended attributes.
    async fn write_xattrs(&self, ino: u64) -> Result<(), Error> {
        let Some(changes) = self.xattrs.lock().unwrap().remove(&ino) else {
            return Ok(());
        };
        let handle = self.handle(ino);
        let mut changes = changes.into_iter();
        while let Some((name, value)) = changes.next() {
            let written = match value.clone() {
                Some(value) => {
                    let mode = SetExtendedAttributeMode::Set;
                    handle.set_extended_attribute(name.clone(), value, mode).await
                }
                None => match handle.remove_extended_attribute(name.clone()).await {
                    // Set and removed before either was written back.
                    Err(error) if FxfsError::NotFound.matches(&error) => Ok(()),
                    removed => removed,
                },
            };
            if let Err(error) = written {
                // What is left is written back next time, unless changed again meanwhile.
                let mut xattrs = self.xattrs.lock().unwrap();
                let pending = xattrs.entry(ino).or_default();
                for (name, value) in std::iter::once((name, value)).chain(changes) {
                    pending.entry(name).or_insert(value);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Sets the extended attribute `name` of the object `ino` to `value`: creating it, replacing
    /// it, or either, as `mode` says. Written back with the inode (`write_inode`).
    pub async fn set_xattr(
        &self,
        ino: u64,
        name: &[u8],
        value: &[u8],
        mode: SetExtendedAttributeMode,
    ) -> Result<(), Error> {
        // What Fxfs would refuse, refused now.
        ensure!(name.len() <= MAX_XATTR_NAME_SIZE, FxfsError::TooBig);
        ensure!(value.len() <= MAX_XATTR_VALUE_SIZE, FxfsError::TooBig);
        match mode {
            SetExtendedAttributeMode::Create if self.has_xattr(ino, name).await? => {
                bail!(FxfsError::AlreadyExists)
            }
            SetExtendedAttributeMode::Replace if !self.has_xattr(ino, name).await? => {
                bail!(FxfsError::NotFound)
            }
            _ => {}
        }
        self.change_xattr(ino, name, Some(value.to_vec()));
        Ok(())
    }

    /// The names of the object `ino`'s extended attributes.
    pub async fn list_xattrs(&self, ino: u64) -> Result<Vec<Vec<u8>>, Error> {
        let mut names = self.handle(ino).list_extended_attributes().await?;
        if let Some(changes) = self.xattrs.lock().unwrap().get(&ino) {
            names.retain(|name| !changes.contains_key(name));
            let set = changes.iter().filter(|(_, value)| value.is_some());
            names.extend(set.map(|(name, _)| name.clone()));
        }
        Ok(names)
    }

    /// Removes the extended attribute `name` of the object `ino`. Written back with the inode
    /// (`write_inode`).
    pub async fn remove_xattr(&self, ino: u64, name: &[u8]) -> Result<(), Error> {
        ensure!(self.has_xattr(ino, name).await?, FxfsError::NotFound);
        self.change_xattr(ino, name, None);
        Ok(())
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

    /// Closes the inode `ino`, which the kernel evicts, and frees its object if no name links it.
    pub fn evict(&self, ino: u64, unlinked: bool) {
        self.directories.lock().unwrap().remove(&ino);
        self.files.lock().unwrap().remove(&ino);
        // An inode is written back before it is evicted, unless no name links it.
        let changes = self.xattrs.lock().unwrap().remove(&ino);
        if changes.is_some() && !unlinked {
            log::error!("inode {ino} evicted with extended attributes not written back");
        }
        if unlinked {
            self.filesystem.graveyard().queue_tombstone_object(self.volume.store_object_id(), ino);
        }
    }

    /// Sets the attributes of the object `ino` that `set` says.
    /// Writes back what the kernel keeps of the inode `ino`: the attributes `set` says, and the
    /// changes to its extended attributes.
    pub async fn write_inode(&self, ino: u64, set: &SetAttr) -> Result<(), Error> {
        self.setattr(ino, set).await?;
        self.write_xattrs(ino).await
    }

    pub async fn setattr(&self, ino: u64, set: &SetAttr) -> Result<(), Error> {
        if set.valid & SET_SIZE != 0 {
            if self.attr(ino).await?.mode & S_IFMT != S_IFREG {
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
        if attributes != fio::MutableNodeAttributes::default() || flag(SET_CTIME) {
            let mut transaction = self.transaction(&[ino]).await?;
            let ctime = if flag(SET_CTIME) {
                Timestamp::from_nanos(set.ctime_ns)
            } else {
                Timestamp::now()
            };
            let handle = self.handle(ino);
            handle.update_attributes(&mut transaction, Some(&attributes), Some(ctime)).await?;
            transaction.commit().await?;
        }
        Ok(())
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

    /// The directory `ino`'s handle: the inode's, opened if it isn't yet.
    async fn directory(&self, ino: u64) -> Result<Arc<Directory<ObjectStore>>, Error> {
        if let Some(directory) = self.directories.lock().unwrap().get(&ino) {
            return Ok(directory.clone());
        }
        let directory = Arc::new(Directory::open(&self.volume, ino).await?);
        // Another call may have opened it meanwhile: the first's stays.
        Ok(self.directories.lock().unwrap().entry(ino).or_insert(directory).clone())
    }

    /// A handle to the object `ino`, of whatever kind, for its attributes.
    fn handle(&self, ino: u64) -> StoreObjectHandle<ObjectStore> {
        StoreObjectHandle::new(self.volume.clone(), ino, false, HandleOptions::default(), false)
    }

    /// The file `ino`'s handle: the inode's, opened if it isn't yet.
    async fn file(&self, ino: u64) -> Result<Arc<DataObjectHandle<ObjectStore>>, Error> {
        if let Some(file) = self.files.lock().unwrap().get(&ino) {
            return Ok(file.clone());
        }
        let file = ObjectStore::open_object(&self.volume, ino, HandleOptions::default(), None);
        let file = Arc::new(file.await?);
        // Another call may have opened it meanwhile: the first's stays.
        Ok(self.files.lock().unwrap().entry(ino).or_insert(file).clone())
    }

    /// A transaction locking the objects `inos`.
    async fn transaction(&self, inos: &[u64]) -> Result<Transaction<'_>, Error> {
        self.transaction_with(inos, Options::default()).await
    }

    async fn transaction_with<'a>(
        &'a self,
        inos: &[u64],
        options: Options<'a>,
    ) -> Result<Transaction<'a>, Error> {
        let store = self.volume.store_object_id();
        let mut keys = LockKeys::with_capacity(inos.len());
        for &ino in inos {
            keys.push(LockKey::object(store, ino));
        }
        self.volume.new_transaction(keys, options).await
    }

    /// A transaction that replaces a name, locking `inos` and what it replaces, `replaced` if
    /// anything, which can't be freed meanwhile: as Fxfs's `acquire_context_for_replace` has it,
    /// but for the kernel knowing which inodes the names link, which its locks keep.
    async fn replace_transaction<'a>(
        &'a self,
        inos: &[u64],
        replaced: Option<u64>,
        options: Options<'a>,
    ) -> Result<(Option<TruncateGuard<'a>>, Transaction<'a>), Error> {
        let store = self.volume.store_object_id();
        let guard = match replaced {
            Some(ino) => Some(self.filesystem.truncate_guard(store, ino).await),
            None => None,
        };
        let inos: Vec<u64> = inos.iter().copied().chain(replaced).collect();
        Ok((guard, self.transaction_with(&inos, options).await?))
    }
}

/// Fails unless `replaced` is what the kernel said a name linked, `expected`.
fn check_replaced(replaced: &ReplacedChild, expected: Option<u64>) -> Result<(), Error> {
    let found = match *replaced {
        ReplacedChild::None => None,
        ReplacedChild::Object(ino)
        | ReplacedChild::ObjectWithRemainingLinks(ino)
        | ReplacedChild::Directory(ino)
        | ReplacedChild::Purged(ino) => Some(ino),
    };
    if found != expected {
        bail!(
            anyhow!(FxfsError::Inconsistent)
                .context(format!("replaced {found:?}, where the kernel had {expected:?}"))
        );
    }
    Ok(())
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

/// The key of the record of object `ino`'s data.
fn data_key(ino: u64) -> ObjectKey {
    ObjectKey::attribute(ino, AttributeId::DATA, AttributeKey::Attribute)
}

/// The size of the data whose record is `data`: none if there is none.
fn data_size(data: Option<ObjectValue>) -> u64 {
    match data {
        Some(ObjectValue::Attribute { size, .. } | ObjectValue::VerifiedAttribute { size, .. }) => {
            size
        }
        _ => 0,
    }
}
