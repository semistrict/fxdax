//! Fxfs, mounted on an extent-store device: the volume fxdax's images keep their files in, served
//! by object ID, which is the kernel's inode number.

use crate::device::ExtentDevice;
use anyhow::{Error, anyhow};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::object_handle::ReadObjectHandle;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{Directory, HandleOptions, ObjectDescriptor, ObjectStore, StoreOptions};
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
}

/// A mounted filesystem.
pub struct Fxfs {
    filesystem: OpenFxFilesystem,
    volume: Arc<ObjectStore>,
}

impl Fxfs {
    /// Mounts the filesystem on `device`, and its volume.
    pub async fn mount(device: ExtentDevice) -> Result<Self, Error> {
        let filesystem = FxFilesystemBuilder::new().open(DeviceHolder::new(device)).await?;
        let volume = async {
            root_volume(filesystem.clone()).await?.volume(VOLUME, StoreOptions::default()).await
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
        let root = self.volume.root_directory_object_id();
        self.attr(root, &ObjectDescriptor::Directory).await
    }

    /// The attributes of `name` in the directory `dir`.
    pub async fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        // Fxfs's names are UTF-8: no other name is in a directory.
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let (ino, descriptor, _) =
            self.directory(dir).await?.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        self.attr(ino, &descriptor).await
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
        let file =
            ObjectStore::open_object(&self.volume, ino, HandleOptions::default(), None).await?;
        let end = file.get_size().min(offset.saturating_add(buf.len() as u64));
        if offset >= end {
            return Ok(0);
        }
        let data = file.read_bytes(offset..end).await?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    async fn directory(&self, ino: u64) -> Result<Directory<ObjectStore>, Error> {
        Directory::open(&self.volume, ino).await
    }

    /// The attributes of the object `ino`, which `descriptor` describes.
    async fn attr(&self, ino: u64, descriptor: &ObjectDescriptor) -> Result<Attr, Error> {
        let properties = match descriptor {
            ObjectDescriptor::Directory => self.directory(ino).await?.get_properties().await?,
            ObjectDescriptor::File => {
                ObjectStore::open_object(&self.volume, ino, HandleOptions::default(), None)
                    .await?
                    .get_properties()
                    .await?
            }
            _ => return Err(anyhow!(FxfsError::NotSupported).context("not a file or directory")),
        };
        let kind = file_type(descriptor);
        let (permissions, uid, gid) = match properties.posix_attributes {
            Some(posix) => (posix.mode & !S_IFMT, posix.uid, posix.gid),
            None => (if kind == S_IFDIR { 0o755 } else { 0o644 }, 0, 0),
        };
        let nlink = match kind {
            // Its entry in its parent, its own `.`, and its subdirectories' `..`s.
            S_IFDIR => 2 + properties.sub_dirs,
            _ => properties.refs,
        };
        Ok(Attr {
            ino,
            size: properties.data_attribute_size,
            blocks: properties.allocated_size / 512,
            atime_ns: properties.access_time.as_nanos(),
            mtime_ns: properties.modification_time.as_nanos(),
            ctime_ns: properties.change_time.as_nanos(),
            mode: kind | permissions,
            nlink: nlink.try_into().unwrap_or(u32::MAX),
            uid,
            gid,
        })
    }
}

/// The `S_IFMT` bits of objects `descriptor` describes.
fn file_type(descriptor: &ObjectDescriptor) -> u32 {
    match descriptor {
        ObjectDescriptor::Directory => S_IFDIR,
        ObjectDescriptor::Symlink => S_IFLNK,
        _ => S_IFREG,
    }
}
