//! An image: a mounted Fxfs filesystem whose volumes are each a [`Filesystem`].
//!
//! Fxfs keeps any number of volumes on one device, which share its journal, its allocator and the
//! memory they cache. An [`Image`] mounts the device and creates, lists, opens and deletes its
//! volumes; each open volume is a [`Filesystem`], which unmounts on its own. Closing the image
//! unmounts the volumes still open, and gives the storage back.

use crate::error::{Errno, Error};
use crate::executor::{Executor, assert_not_on_executor};
use crate::filesystem::{Filesystem, Mount, Options};
use crate::storage::{CloneKind, Kind, Storage, clone_image};
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
    /// The mounts of the image's volumes, which closing it unmounts.
    mounts: Vec<Weak<Mount>>,
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
                    MemoryPressureConfig::default(),
                )
                .await?;
                let root_volume = Arc::new(root_volume(filesystem.clone()).await?);
                anyhow::Ok(Open { filesystem, volumes_directory, root_volume })
            })
            .await
            .map_err(Error::new)?;
        let block_size = open.filesystem.block_size().get() as u32;
        let state = Mutex::new(State { open: Some(open), mounts: Vec::new() });
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
                if let Some(volume) = mounted {
                    volume.flush_all_files(FlushType::Sync).await;
                }
                volumes_directory.snapshot_volume(&source, &name).await
            })
            .await
            .map_err(volume_error)
    }

    /// Clones the image to a new file at `path`, which must not exist, while it is mounted, and
    /// returns the clone's storage and how it was cloned (see [`Storage::clone_to`]).
    ///
    /// Every mounted volume's dirty pages are written back and the journal is committed, flushing
    /// the image, before it is cloned, so the clone has everything written before this was called.
    /// Writes made while it is cloning may be in the clone or not, as after a crash, which the
    /// clone recovers from as Fxfs does. Where the host filesystem can't share the image's blocks,
    /// the image's writes wait while it is copied.
    pub async fn clone_storage_to(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(Storage, CloneKind), Error> {
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

    /// The volume `name`, if it is mounted.
    fn mounted_volume(&self, name: &str) -> Option<Arc<FxVolume>> {
        let state = self.state.lock().unwrap();
        state.mounts.iter().filter_map(Weak::upgrade).find(|mount| mount.name() == name)?.volume()
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
        let (filesystem, mount) = Filesystem::new(self.clone(), &name, volume).await;
        let mut state = self.state.lock().unwrap();
        state.mounts.retain(|mount| mount.strong_count() > 0);
        state.mounts.push(Arc::downgrade(&mount));
        Ok(filesystem)
    }

    pub async fn clone_storage_to(&self, path: &Path) -> Result<(Storage, CloneKind), Error> {
        let Kind::File { file, writes, .. } = &self.storage_kind else {
            return Err(Error::msg("memory cannot be cloned to a file"));
        };
        let (filesystem, volumes) = {
            let state = self.state.lock().unwrap();
            let open = state.open.as_ref().ok_or(Errno::ENOTCONN)?;
            let volumes: Vec<_> =
                state.mounts.iter().filter_map(|mount| mount.upgrade()?.volume()).collect();
            ((*open.filesystem).clone(), volumes)
        };
        self.executor
            .run(async move {
                for volume in volumes {
                    volume.flush_all_files(FlushType::Sync).await;
                }
                filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
            })
            .await
            .map_err(Error::new)?;
        clone_image(file.clone(), writes.clone(), path).await
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
        // The volumes' unmounts begin first, while the volumes directory they unmount from is open.
        let mounts: Vec<_> = {
            let state = self.state.lock().unwrap();
            state.open.as_ref()?;
            state.mounts.iter().filter_map(Weak::upgrade).collect()
        };
        let unmounts: Vec<_> = mounts.iter().map(|mount| mount.unmounting(self)).collect();
        let Open { filesystem, volumes_directory, root_volume } = {
            let mut state = self.state.lock().unwrap();
            state.mounts.clear();
            state.open.take()?
        };
        Some(async move {
            for unmount in unmounts {
                if let Err(error) = unmount.await {
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
