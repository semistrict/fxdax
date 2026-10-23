// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An image: a mounted Fxfs filesystem whose volumes are each a [`Filesystem`].
//!
//! Fxfs keeps any number of volumes on one device, which share its journal, its allocator and the
//! memory they cache. An [`Image`] mounts the device and creates, lists, opens and deletes its
//! volumes; each open volume is a [`Filesystem`], which unmounts on its own. Closing the image
//! unmounts the volumes still open, and gives the storage back.

use crate::error::{Errno, Error};
use crate::executor::{Executor, assert_not_on_executor};
use crate::filesystem::{Filesystem, Options};
use crate::storage::{Kind, Storage, clone_image};
use crate::volume::{Unmount, Volume};
use fidl_fuchsia_fs_startup::CreateOptions;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::object_store::volume::{RootVolume, root_volume};
use fxfs_platform::file::FlushType;
use fxfs_platform::memory_pressure::MemoryPressureMonitor;
use fxfs_platform::volume::{FxVolume, MemoryPressureConfig};
use fxfs_platform::volumes_directory::VolumesDirectory;
use refaults_vmo::PageRefaultCounter;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use storage_device::DeviceHolder;
use vfs::execution_scope::ActiveGuard;

/// A mounted Fxfs image, whose named volumes are filesystems of their own.
///
/// Create volumes with [`create_volume`](Self::create_volume), and open them with
/// [`open_volume`](Self::open_volume). The volumes share the image's journal, allocator and
/// executor; each is unmounted when its [`Filesystem`] is, or when the image is closed.
pub struct Image {
    shared: Arc<ImageShared>,
}

/// What an image's volumes share with it.
pub(crate) struct ImageShared {
    pub executor: Executor,
    pub block_size: u32,
    storage_kind: Kind,
    state: Mutex<State>,
}

struct State {
    /// The filesystem, until the image is closed.
    open: Option<Open>,
    /// The volumes mounted, while their filesystems live: closing the image unmounts them, or
    /// waits for their unmounts under way.
    volumes: Vec<Weak<Volume>>,
}

struct Open {
    filesystem: OpenFxFilesystem,
    /// The image's volumes directory, which mounts its volumes and runs their background tasks,
    /// and limits their dirty pages under memory pressure, as Fxfs's component does.
    volumes_directory: Arc<VolumesDirectory>,
    /// The image's root volume, whose directory lists the volumes.
    root_volume: Arc<RootVolume>,
}

impl Image {
    /// Mounts the image on `storage` with the default [`Options`].
    pub async fn mount(storage: Storage) -> Result<Self, Error> {
        Self::start(&Options::new(), storage, false).await
    }

    /// Formats `storage` with an empty image, which has no volumes, and mounts it with the default
    /// [`Options`].
    pub async fn format(storage: Storage) -> Result<Self, Error> {
        Self::start(&Options::new(), storage, true).await
    }

    pub(crate) async fn start(
        options: &Options,
        storage: Storage,
        format: bool,
    ) -> Result<Self, Error> {
        let (device, storage_kind) = storage.into_parts();
        let executor = Executor::new(options.threads());
        let open = executor
            .run(async move {
                let filesystem = FxFilesystemBuilder::new().format(format).open(device).await?;
                let volumes_directory = VolumesDirectory::new(
                    root_volume(filesystem.clone()).await?,
                    Some(MemoryPressureMonitor::start()?),
                    Arc::new(PageRefaultCounter::new()?),
                    memory_pressure_config(),
                )
                .await?;
                let root_volume = Arc::new(root_volume(filesystem.clone()).await?);
                anyhow::Ok(Open { filesystem, volumes_directory, root_volume })
            })
            .await
            .map_err(Error::new)?;
        let block_size = open.filesystem.block_size().get() as u32;
        let state = Mutex::new(State { open: Some(open), volumes: Vec::new() });
        Ok(Self { shared: Arc::new(ImageShared { executor, block_size, storage_kind, state }) })
    }

    /// Creates the volume `name`, which must not exist, and mounts it.
    pub async fn create_volume(&self, name: &str) -> Result<Filesystem, Error> {
        self.shared.mount_volume(name, true).await
    }

    /// Mounts the volume `name`, which must not be mounted.
    pub async fn open_volume(&self, name: &str) -> Result<Filesystem, Error> {
        self.shared.mount_volume(name, false).await
    }

    /// The names of the image's volumes.
    pub async fn volumes(&self) -> Result<Vec<String>, Error> {
        let root_volume = self.shared.with_open(|open| open.root_volume.clone())?;
        self.shared
            .executor
            .run(async move {
                let directory = root_volume.volume_directory();
                let layer_set = directory.store().tree().layer_set();
                let mut merger = layer_set.merger();
                let mut entries = directory.iter(&mut merger).await?;
                let mut names = Vec::new();
                while let Some((name, _, _)) = entries.get() {
                    names.push(name.to_owned());
                    entries.advance().await?;
                }
                anyhow::Ok(names)
            })
            .await
            .map_err(Error::new)
    }

    /// Deletes the volume `name` and everything in it. Fails with `EBUSY` if it is mounted.
    pub async fn delete_volume(&self, name: &str) -> Result<(), Error> {
        let volumes_directory = self.shared.with_open(|open| open.volumes_directory.clone())?;
        let name = name.to_owned();
        self.shared
            .executor
            .run(async move { volumes_directory.remove_volume(&name).await })
            .await
            .map_err(volume_error)
    }

    /// Snapshots the volume `source` into the new volume `name`: a volume of its own from then on,
    /// which starts with everything the source held, writing back its dirty data first if it is
    /// mounted. Writes to the source while it snapshots may be in the snapshot or not.
    ///
    /// It takes time and space independent of the source's size: the snapshot shares the source's
    /// metadata and data, until either changes them, as Fxfs writes changes to new blocks. What
    /// they share is freed once neither references it. Fails with `EEXIST` if `name` exists, and
    /// `ENOENT` if `source` doesn't.
    pub async fn snapshot_volume(&self, source: &str, name: &str) -> Result<(), Error> {
        let volumes_directory = self.shared.volumes_directory()?;
        let mounted = self.shared.mounted_volume(source);
        let (source, name) = (source.to_owned(), name.to_owned());
        self.shared
            .executor
            .run(async move {
                // Under the guard, so that an unmount begun meanwhile writes the volume back after
                // this has; and no longer, as the snapshot waits for an unmount under way.
                if let Some((volume, _guard)) = mounted {
                    volume.flush_all_files(FlushType::Sync).await;
                }
                volumes_directory.snapshot_volume(&source, &name).await
            })
            .await
            .map_err(volume_error)
    }

    /// Clones the image to a new image at `path`, which must not exist, while it is mounted, and
    /// returns the clone's storage (see [`Storage::clone_to`]).
    ///
    /// Every mounted volume's dirty pages are written back and the journal is committed, flushing
    /// the image, before it is cloned, so the clone has everything written before this was called.
    /// The image's writes wait while it is cloned, so the clone is the image as it was at one
    /// time, which it recovers from as Fxfs does after a crash.
    pub async fn clone_storage_to(&self, path: impl AsRef<Path>) -> Result<Storage, Error> {
        self.shared.clone_storage_to(path.as_ref()).await
    }

    /// Unmounts the volumes still mounted, writing back what is dirty, closes the image, and gives
    /// its storage back.
    pub async fn close(self) -> Result<Storage, Error> {
        // Once closed, dropping the image does nothing.
        self.shared.close().await
    }
}

impl Drop for Image {
    /// Closes the image, unmounting its volumes, blocking the dropping thread until it is done.
    /// Await [`close`](Image::close) rather than dropping an image.
    ///
    /// # Panics
    ///
    /// If an open image is dropped on Fxfs's executor.
    fn drop(&mut self) {
        let Some(closing) = self.shared.begin_close() else {
            return;
        };
        assert_not_on_executor();
        if let Err(error) = self.shared.executor.run_blocking(closing) {
            log::error!("fxdax: failed to close the image: {error:#}");
        }
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let open = self.shared.state.lock().unwrap().open.is_some();
        f.debug_struct("Image").field("open", &open).finish_non_exhaustive()
    }
}

impl ImageShared {
    /// Runs `f` on the open filesystem, or fails with `ENOTCONN` once the image is closed.
    fn with_open<T>(&self, f: impl FnOnce(&Open) -> T) -> Result<T, Errno> {
        self.state.lock().unwrap().open.as_ref().map(f).ok_or(Errno::ENOTCONN)
    }

    /// The image's volumes directory, until it is closed.
    pub fn volumes_directory(&self) -> Result<Arc<VolumesDirectory>, Errno> {
        self.with_open(|open| open.volumes_directory.clone())
    }

    /// The volume `name`, Fxfs's, if it is mounted, with a guard on its scope
    /// ([`Volume::fx_volume_guarded`]). An unmounted filesystem of it may be listed still, beside
    /// its remount's.
    fn mounted_volume(&self, name: &str) -> Option<(Arc<FxVolume>, ActiveGuard)> {
        let state = self.state.lock().unwrap();
        let volumes = state.volumes.iter().filter_map(Weak::upgrade);
        volumes
            .filter(|volume| volume.name() == name)
            .find_map(|volume| volume.fx_volume_guarded().ok())
    }

    /// Creates the volume `name`, or mounts it, and serves it as a filesystem.
    async fn mount_volume(self: &Arc<Self>, name: &str, create: bool) -> Result<Filesystem, Error> {
        let volumes_directory = self.volumes_directory()?;
        let name = name.to_owned();
        let volume = self
            .executor
            .run({
                let name = name.clone();
                async move {
                    if create {
                        let options = CreateOptions::default();
                        volumes_directory.create_and_mount_volume(&name, None, false, options).await
                    } else {
                        volumes_directory.mount_volume(&name, None, false).await
                    }
                }
            })
            .await
            .map_err(volume_error)?;
        let filesystem = Filesystem::new(self.clone(), &name, volume).await;
        let mut state = self.state.lock().unwrap();
        state.volumes.retain(|volume| volume.strong_count() > 0);
        state.volumes.push(Arc::downgrade(filesystem.volume()));
        Ok(filesystem)
    }

    /// Begins unmounting `volume`, one of the image's, unless that has begun, from the image's
    /// volumes directory while the image is open: once it is closed, so are its volumes. Under the
    /// image's lock, so that the image doesn't close meanwhile, as its close begins its volumes'
    /// unmounts under it.
    pub fn unmount_volume(&self, volume: &Volume) -> Unmount {
        let state = self.state.lock().unwrap();
        let volumes_directory = state.open.as_ref().map(|open| open.volumes_directory.clone());
        volume.begin_unmount(volumes_directory)
    }

    pub async fn clone_storage_to(&self, path: &Path) -> Result<Storage, Error> {
        let Kind::Image { store, writes, .. } = &self.storage_kind else {
            return Err(Error::msg("memory cannot be cloned to an image"));
        };
        let (filesystem, volumes) = {
            let state = self.state.lock().unwrap();
            let open = state.open.as_ref().ok_or(Errno::ENOTCONN)?;
            let volumes: Vec<_> = state
                .volumes
                .iter()
                .filter_map(|volume| volume.upgrade()?.fx_volume_guarded().ok())
                .collect();
            ((*open.filesystem).clone(), volumes)
        };
        self.executor
            .run(async move {
                // Each under its guard, so that an unmount begun meanwhile writes the volume back
                // after this has.
                for (volume, _guard) in volumes {
                    volume.flush_all_files(FlushType::Sync).await;
                }
                filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
            })
            .await
            .map_err(Error::new)?;
        clone_image(store.clone(), writes.clone(), path).await
    }

    /// Unmounts the volumes still mounted, closes the filesystem, and gives the storage back.
    pub async fn close(&self) -> Result<Storage, Error> {
        let closing = self.begin_close().ok_or(Errno::ENOTCONN)?;
        let device = self.executor.run(closing).await.map_err(Error::new)?;
        Ok(Storage::closed(device, self.storage_kind.clone()))
    }

    /// Begins closing the image, unless it is closed, and returns the future that unmounts its
    /// volumes and closes the filesystem on Fxfs's executor, giving its device back.
    fn begin_close(
        &self,
    ) -> Option<impl Future<Output = anyhow::Result<DeviceHolder>> + Send + 'static> {
        // The volumes' unmounts begin as the image's filesystem is taken, under one hold of the
        // lock, from the volumes directory they unmount from: an unmount begun in between
        // ([`ImageShared::unmount_volume`]) would find it gone, and not unmount the volume from
        // it. Those under way already are waited for.
        let (Open { filesystem, volumes_directory, root_volume }, unmounts) = {
            let mut state = self.state.lock().unwrap();
            let open = state.open.take()?;
            let volumes = std::mem::take(&mut state.volumes);
            let unmounts: Vec<Unmount> = (volumes.iter().filter_map(Weak::upgrade))
                .map(|volume| volume.begin_unmount(Some(open.volumes_directory.clone())))
                .collect();
            (open, unmounts)
        };
        Some(async move {
            for unmount in unmounts {
                if let Err(error) = unmount.wait().await {
                    log::error!("fxdax: failed to unmount a volume: {error:#}");
                }
            }
            drop(root_volume);
            volumes_directory.terminate().await;
            drop(volumes_directory);
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        })
    }
}

/// The error of a volume operation, with the errno of the Fxfs error causing it, if one did.
fn volume_error(error: anyhow::Error) -> Error {
    let fxfs_error = error.chain().find_map(|cause| cause.downcast_ref::<FxfsError>()).cloned();
    match fxfs_error {
        Some(FxfsError::AlreadyBound) => Error::new(error.context(Errno::EBUSY)),
        Some(fxfs_error) => Error::new(error.context(Errno::of_status(fxfs_error.into()))),
        None => Error::new(error),
    }
}

/// Fxfs's configuration under memory pressure, with more of the directory entries looked up kept
/// while there is none: Fuchsia's 2,000 are a phone's, and a guest's own cache of entries, which
/// it forgets on reclaim, is a whole VM's. Pressure shrinks it as Fuchsia's does.
fn memory_pressure_config() -> MemoryPressureConfig {
    let mut config = MemoryPressureConfig::default();
    config.mem_normal.cache_size_limit = 16_384;
    config
}
