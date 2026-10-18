// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The storage a filesystem lives on: an image, or memory.

mod gate;
mod image;
mod io;

use crate::error::{Error, Result};
use crate::executor::Executor;
use crate::filesystem::DEFAULT_EXECUTOR_THREADS;
use extent_store::{Description, HostDir, Store};
use fxfs::filesystem::FxFilesystem;
use fxfs::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use fxfs::object_store::volume::{list_volumes, root_volume};
use gate::WriteGate;
use image::ImageDevice;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

/// The storage a filesystem lives on.
///
/// A filesystem takes its storage when it mounts it, and gives it back when it is closed, ready
/// to be mounted or checked again.
pub struct Storage {
    device: DeviceHolder,
    kind: Kind,
}

#[derive(Clone)]
pub(crate) enum Kind {
    /// An image: its directory, whether the device's I/O bypasses the page cache, the device's
    /// writes and frees, which cloning the image pauses, and its store, which a clone forks.
    Image {
        dir: PathBuf,
        direct: bool,
        writes: Arc<WriteGate>,
        store: Arc<Store<HostDir>>,
    },
    Memory,
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image { dir, direct, .. } => {
                f.debug_struct("Image").field("dir", dir).field("direct", direct).finish()
            }
            Self::Memory => write!(f, "Memory"),
        }
    }
}

impl Storage {
    /// The size of the blocks storage is read and written in. Storage sizes are multiples of it.
    pub const BLOCK_SIZE: u32 = 4096;

    /// The image at `path`: a directory holding an extent store (see `image`).
    ///
    /// Its I/O bypasses the host's page cache where the host filesystem allows, as the in-process
    /// kernel caches files' pages already, and runs on threads of its own (io_uring on Linux).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = path.as_ref().to_owned();
        let writes = Arc::new(WriteGate::default());
        let device = ImageDevice::open(&dir, writes.clone()).map_err(Error::new)?;
        let (direct, store) = (device.is_direct(), device.store());
        let kind = Kind::Image { dir, direct, writes, store };
        Ok(Self { device: DeviceHolder::new(device), kind })
    }

    /// A new image at `path`, which must not exist, holding at most `size` bytes. It holds nothing
    /// until the filesystem writes to it, and must be formatted before it is mounted.
    pub fn create(path: impl AsRef<Path>, size: u64) -> Result<Self, Error> {
        let path = path.as_ref();
        if size < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the image is smaller than a block"));
        }
        let description =
            Description { block_size: Self::BLOCK_SIZE, capacity: size, branch: new_branch() };
        let writes = Arc::new(WriteGate::default());
        let device = ImageDevice::create(path, description, writes.clone()).map_err(Error::new)?;
        let (dir, direct, store) = (path.to_owned(), device.is_direct(), device.store());
        let kind = Kind::Image { dir, direct, writes, store };
        Ok(Self { device: DeviceHolder::new(device), kind })
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

    /// Whether the storage is an image whose I/O bypasses the host's page cache.
    pub fn bypasses_page_cache(&self) -> bool {
        matches!(self.kind, Kind::Image { direct: true, .. })
    }

    /// Clones the image to a new image at `path`, which must not exist, and returns its storage.
    /// The clone is a fork of the image's extent store: it takes the same time and space whatever
    /// the image holds, sharing the image's data, whose files each keeps while it holds them.
    /// `path` is the fork's handle, naming the image's directory, which must stay where it is for
    /// as long as the clone does. Memory cannot be cloned.
    ///
    /// The storage must not be mounted, which it is not while it is the caller's. To clone a
    /// mounted filesystem's, see [`Filesystem::clone_storage_to`](crate::Filesystem::clone_storage_to).
    pub async fn clone_to(&self, path: impl AsRef<Path>) -> Result<Storage, Error> {
        let Kind::Image { store, writes, .. } = &self.kind else {
            return Err(Error::msg("memory cannot be cloned to an image"));
        };
        clone_image(store.clone(), writes.clone(), path.as_ref()).await
    }

    /// Deletes the image at `path`, which nothing in this process has open: a clone, or an image,
    /// whose data goes but for what other clones hold. An image whose clones live keeps its
    /// directory, holding what they share of it, which no longer opens, and which goes with the
    /// last of them (TASK-39.4). An image's directory must not be removed otherwise while clones of
    /// it live: they hold their data there.
    pub fn delete(path: impl AsRef<Path>) -> Result<(), Error> {
        let options = extent_store::Options::default();
        extent_store::delete_host(path.as_ref(), options).map_err(Error::new)
    }

    /// The most bytes the storage holds.
    pub fn size(&self) -> u64 {
        self.device.block_count() * self.device.block_size() as u64
    }

    /// Checks the image on the storage and every volume in it, failing on any inconsistency, or
    /// anything it warns about, and gives the storage back.
    pub async fn check(self) -> Result<Self, Error> {
        let Self { device, kind } = self;
        let executor = Executor::new(DEFAULT_EXECUTOR_THREADS);
        let device = executor.run(async move {
            let filesystem = FxFilesystem::open(device).await?;
            let options = FsckOptions { fail_on_warning: true, ..Default::default() };
            fsck_with_options(filesystem.clone(), &options).await?;
            let store_ids =
                list_volumes(root_volume(filesystem.clone()).await?.volume_directory()).await?;
            for store_id in store_ids {
                fsck_volume_with_options(filesystem.as_ref(), &options, store_id, None).await?;
            }
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        });
        Ok(Self::closed(device.await.map_err(Error::new)?, kind))
    }

    pub(crate) fn into_parts(self) -> (DeviceHolder, Kind) {
        (self.device, self.kind)
    }

    /// The storage of `device`, which a filesystem closed.
    pub(crate) fn closed(device: DeviceHolder, kind: Kind) -> Self {
        match kind {
            // Closing an image only flushes it.
            Kind::Image { .. } => {}
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

/// A new image's branch: random, so that images made apart, and the clones of one, mint extent
/// IDs of their own (backlog doc-4).
pub(crate) fn new_branch() -> u64 {
    rand::random::<u64>().max(1)
}

/// Clones the image whose store is `store` to a new image at `path`, a fork of the store with a
/// branch of its own, on a thread of its own, as forking commits the store durably. The clone
/// pauses `writes`, the device's writes and frees, while it forks, so that it is the image as it
/// was at one time.
pub(crate) async fn clone_image(
    store: Arc<Store<HostDir>>,
    writes: Arc<WriteGate>,
    path: &Path,
) -> Result<Storage, Error> {
    let path = path.to_owned();
    let (done, cloned) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("fxdax clone".into())
        .spawn(move || {
            let _paused = writes.pause();
            let cloned = extent_store::fork_host(&store, &path, new_branch()).map_err(|error| {
                Error::new(error.context(format!("cloning to {}", path.display())))
            });
            let _ = done.send(cloned.map(|()| path));
        })
        .map_err(Error::new)?;
    let path = cloned.await.map_err(Error::new)??;
    Storage::open(path)
}
