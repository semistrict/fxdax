//! The storage a filesystem lives on: an image file, or memory.

use crate::error::{Error, Result};
use crate::executor::Executor;
use crate::filesystem::{DEFAULT_EXECUTOR_THREADS, VOLUME_NAME};
use fxfs::filesystem::FxFilesystem;
use fxfs::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use fxfs::object_store::StoreOptions;
use fxfs::object_store::volume::root_volume;
use std::fmt;
use std::fs::File;
use std::path::Path;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;
use storage_device::file_backed_device::FileBackedDevice;

/// The storage a filesystem lives on.
///
/// A filesystem takes its storage when it mounts it, and gives it back when it is closed, ready
/// to be mounted or checked again.
pub struct Storage {
    device: DeviceHolder,
    kind: Kind,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    File,
    Memory,
}

impl Storage {
    /// The size of the blocks storage is read and written in. Storage sizes are multiples of it.
    pub const BLOCK_SIZE: u32 = 4096;

    /// The image in `file`, which must be open for reading and writing. Its size, rounded down to
    /// blocks, is the filesystem's.
    pub fn file(file: File) -> Result<Self, Error> {
        let size = file.metadata().map_err(Error::new)?.len();
        if size < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the image is smaller than a block"));
        }
        let device = FileBackedDevice::new(file, Self::BLOCK_SIZE);
        Ok(Self { device: DeviceHolder::new(device), kind: Kind::File })
    }

    /// The image file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let file = File::options().read(true).write(true).open(path).map_err(|error| {
            Error::new(anyhow::Error::new(error).context(format!("opening {}", path.display())))
        })?;
        Self::file(file)
    }

    /// A new image file at `path`, of `size` bytes, which must not exist. It is sparse until the
    /// filesystem writes to it, and must be formatted before it is mounted.
    pub fn create(path: impl AsRef<Path>, size: u64) -> Result<Self, Error> {
        let path = path.as_ref();
        if size < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the image is smaller than a block"));
        }
        let create = || {
            let file = File::options().read(true).write(true).create_new(true).open(path)?;
            file.set_len(size)?;
            std::io::Result::Ok(file)
        };
        let file = create().map_err(|error| {
            Error::new(anyhow::Error::new(error).context(format!("creating {}", path.display())))
        })?;
        Self::file(file)
    }

    /// `size` bytes of memory, which hold a filesystem until they are dropped, such as for a
    /// scratch volume or a test.
    ///
    /// # Panics
    ///
    /// If `size` is not a positive multiple of [`Storage::BLOCK_SIZE`].
    pub fn memory(size: u64) -> Self {
        let block_size = Self::BLOCK_SIZE as u64;
        assert!(size > 0 && size.is_multiple_of(block_size), "{size} is not a multiple of blocks");
        let device = FakeDevice::new(size / block_size, Self::BLOCK_SIZE);
        Self { device: DeviceHolder::new(device), kind: Kind::Memory }
    }

    /// The size in bytes.
    pub fn size(&self) -> u64 {
        self.device.block_count() * self.device.block_size() as u64
    }

    /// Checks the filesystem on the storage, failing on any inconsistency, or anything it warns
    /// about, and gives the storage back.
    pub fn check(self) -> Result<Self, Error> {
        let Self { device, kind } = self;
        let device = Executor::new(DEFAULT_EXECUTOR_THREADS).run(async move {
            let filesystem = FxFilesystem::open(device).await?;
            let options = FsckOptions { fail_on_warning: true, ..Default::default() };
            fsck_with_options(filesystem.clone(), &options).await?;
            let store_id = root_volume(filesystem.clone())
                .await?
                .volume(VOLUME_NAME, StoreOptions::default())
                .await?
                .store_object_id();
            fsck_volume_with_options(filesystem.as_ref(), &options, store_id, None).await?;
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        });
        Ok(Self::closed(device.map_err(Error::new)?, kind))
    }

    pub(crate) fn into_parts(self) -> (DeviceHolder, Kind) {
        (self.device, self.kind)
    }

    /// The storage of `device`, which a filesystem closed.
    pub(crate) fn closed(device: DeviceHolder, kind: Kind) -> Self {
        match kind {
            // Closing a file only syncs it.
            Kind::File => {}
            // Closing memory makes it refuse I/O until it is reopened.
            Kind::Memory => device.reopen(false),
        }
        Self { device, kind }
    }
}

impl fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage").field("kind", &self.kind).field("size", &self.size()).finish()
    }
}
