// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An image: a mounted Fxfs filesystem whose volumes are each a [`Filesystem`].
//!
//! Fxfs keeps any number of volumes on one device, which share its journal, its allocator and the
//! records it caches. An [`Image`] mounts the device and creates, lists, opens and deletes its
//! volumes; each open volume is a [`Filesystem`], which unmounts on its own. Closing the image
//! unmounts the volumes still open, and gives the storage back.

use crate::error::{Errno, Error};
use crate::executor::{Executor, assert_not_on_executor};
use crate::filesystem::{Filesystem, Options};
use crate::storage::{Kind, Storage, clone_image};
use crate::volume::{Unmount, Volume};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::lsm_tree::LSMTree;
use fxfs::lsm_tree::types::{MergeableKey, Value};
use fxfs::object_store::volume::{RootVolume, root_volume};
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
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
    /// Held while volumes are mounted, created, deleted or snapshotted, which the root volume's
    /// directory lists, one at a time.
    volumes_lock: futures::lock::Mutex<()>,
}

struct State {
    /// The filesystem, until the image is closed.
    open: Option<Open>,
    /// The volumes mounted, while their filesystems live: closing the image unmounts them, or
    /// waits for their unmounts under way.
    volumes: Vec<Weak<Volume>>,
    /// The stores of the volumes mounted, until their unmounts are done: a volume is mounted once
    /// at a time.
    mounted: HashSet<u64>,
}

struct Open {
    filesystem: OpenFxFilesystem,
    /// The image's root volume, whose directory lists the volumes.
    root_volume: Arc<RootVolume>,
}

/// What compacting an image's trees has cost since it was mounted ([`Filesystem::compactions`]).
/// Fxfs compacts them as its journal fills, merging each tree's layers into new ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compactions {
    /// The allocator's tree's.
    pub allocator: TreeCompactions,
    /// The object stores' trees', those unlocked, together.
    pub stores: TreeCompactions,
}

/// What compacting a tree, or trees, has cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeCompactions {
    /// The compactions.
    pub count: u64,
    /// The bytes they wrote.
    pub bytes: u64,
    /// The time they took.
    pub time: Duration,
}

impl TreeCompactions {
    /// What compacting `tree` has cost.
    fn of<K: MergeableKey, V: Value>(tree: &LSMTree<K, V>) -> Self {
        let counters = tree.compaction_counters();
        Self {
            count: counters.compactions,
            bytes: counters.compaction_bytes_written,
            time: Duration::from_nanos(counters.compaction_time_ns),
        }
    }
}

impl std::ops::Add for TreeCompactions {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            bytes: self.bytes + other.bytes,
            time: self.time + other.time,
        }
    }
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
                let root_volume = Arc::new(root_volume(filesystem.clone()).await?);
                anyhow::Ok(Open { filesystem, root_volume })
            })
            .await
            .map_err(Error::new)?;
        let block_size = open.filesystem.block_size().get() as u32;
        let state =
            Mutex::new(State { open: Some(open), volumes: Vec::new(), mounted: HashSet::new() });
        let volumes_lock = futures::lock::Mutex::new(());
        let shared = ImageShared { executor, block_size, storage_kind, state, volumes_lock };
        Ok(Self { shared: Arc::new(shared) })
    }

    /// Creates the volume `name`, which must not exist, and mounts it.
    pub async fn create_volume(&self, name: &str) -> Result<Filesystem, Error> {
        self.shared.mount_volume(name, true).await
    }

    /// Mounts the volume `name`, which must not be mounted.
    pub async fn open_volume(&self, name: &str) -> Result<Filesystem, Error> {
        self.shared.mount_volume(name, false).await
    }

    /// Compacts the journal into the volumes' layers: a mount then has none of it to replay, and
    /// no volume to open for it.
    pub async fn compact(&self) -> Result<(), Error> {
        let filesystem = self.shared.with_open(|open| open.filesystem.clone())?;
        self.shared
            .executor
            .run(async move { filesystem.journal().force_compact().await })
            .await
            .map_err(Error::new)
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
        let _volumes = self.shared.volumes_lock.lock().await;
        let root_volume = self.shared.with_open(|open| open.root_volume.clone())?;
        let (shared, name) = (self.shared.clone(), name.to_owned());
        self.shared
            .executor
            .run(async move {
                let (store_id, transaction) =
                    root_volume.acquire_transaction_for_remove_volume(&name, [], false).await?;
                // Cowardly refuse to delete a mounted volume.
                anyhow::ensure!(!shared.is_mounted(store_id), FxfsError::AlreadyBound);
                root_volume.delete_volume(&name, transaction, || {}).await
            })
            .await
            .map_err(volume_error)
    }

    /// Snapshots the volume `source` into the new volume `name`: a volume of its own from then on,
    /// which starts with everything the source held. Writes to the source while it snapshots may
    /// be in the snapshot or not.
    ///
    /// It takes time and space independent of the source's size: the snapshot shares the source's
    /// metadata and data, until either changes them, as Fxfs writes changes to new blocks. What
    /// they share is freed once neither references it. Fails with `EEXIST` if `name` exists, and
    /// `ENOENT` if `source` doesn't.
    pub async fn snapshot_volume(&self, source: &str, name: &str) -> Result<(), Error> {
        let _volumes = self.shared.volumes_lock.lock().await;
        let root_volume = self.shared.with_open(|open| open.root_volume.clone())?;
        let (source, name) = (source.to_owned(), name.to_owned());
        self.shared
            .executor
            .run(async move { root_volume.snapshot_volume(&source, &name).await.map(|_| ()) })
            .await
            .map_err(volume_error)
    }

    /// Clones the image to a new image at `path`, which must not exist, while it is mounted, and
    /// returns the clone's storage (see [`Storage::clone_to`]).
    ///
    /// The journal is committed, flushing the image, before it is cloned, so the clone has
    /// everything written before this was called.
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

    /// What compacting the image's trees has cost since it was mounted, until it is closed.
    pub fn compactions(&self) -> Result<Compactions, Errno> {
        self.with_open(|open| {
            let object_manager = open.filesystem.object_manager();
            Compactions {
                allocator: TreeCompactions::of(object_manager.allocator().tree()),
                stores: object_manager
                    .unlocked_stores()
                    .iter()
                    .map(|store| TreeCompactions::of(store.tree()))
                    .fold(TreeCompactions::default(), std::ops::Add::add),
            }
        })
    }

    /// Whether the volume whose store is `store_id` is mounted, or its unmount is under way.
    fn is_mounted(&self, store_id: u64) -> bool {
        self.state.lock().unwrap().mounted.contains(&store_id)
    }

    /// Notes that the unmount of the volume whose store is `store_id` is done: it may be mounted
    /// again.
    pub fn unmounted(&self, store_id: u64) {
        self.state.lock().unwrap().mounted.remove(&store_id);
    }

    /// Creates the volume `name`, or mounts it, and serves it as a filesystem.
    async fn mount_volume(self: &Arc<Self>, name: &str, create: bool) -> Result<Filesystem, Error> {
        let _volumes = self.volumes_lock.lock().await;
        let root_volume = self.with_open(|open| open.root_volume.clone())?;
        let store = self
            .executor
            .run({
                let name = name.to_owned();
                async move {
                    if create {
                        root_volume.new_volume(&name, NewChildStoreOptions::default()).await
                    } else {
                        root_volume.volume(&name, StoreOptions::default()).await
                    }
                }
            })
            .await
            .map_err(volume_error)?;
        {
            let mut state = self.state.lock().unwrap();
            // Its volumes' unmounts begin as the image closes, which it does no more once
            // closed.
            if state.open.is_none() {
                return Err(Errno::ENOTCONN.into());
            }
            if !state.mounted.insert(store.store_object_id()) {
                return Err(Errno::EBUSY.into());
            }
        }
        let filesystem = Filesystem::new(self.clone(), store);
        let mut state = self.state.lock().unwrap();
        state.volumes.retain(|volume| volume.strong_count() > 0);
        state.volumes.push(Arc::downgrade(filesystem.volume()));
        Ok(filesystem)
    }

    /// Begins unmounting `volume`, one of the image's, unless that has begun. Under the image's
    /// lock, so that the image doesn't close meanwhile, as its close begins its volumes' unmounts
    /// under it.
    pub fn unmount_volume(&self, volume: &Arc<Volume>) -> Unmount {
        let _state = self.state.lock().unwrap();
        volume.begin_unmount()
    }

    pub async fn clone_storage_to(&self, path: &Path) -> Result<Storage, Error> {
        let Kind::Image { store, writes, .. } = &self.storage_kind else {
            return Err(Error::msg("memory cannot be cloned to an image"));
        };
        let filesystem = self.with_open(|open| (*open.filesystem).clone())?;
        self.executor
            .run(async move {
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
        // lock: those under way already are waited for.
        let (Open { filesystem, root_volume }, unmounts) = {
            let mut state = self.state.lock().unwrap();
            let open = state.open.take()?;
            let volumes = std::mem::take(&mut state.volumes);
            let unmounts: Vec<Unmount> = volumes
                .iter()
                .filter_map(Weak::upgrade)
                .map(|volume| volume.begin_unmount())
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
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        })
    }
}

/// The error of a volume operation, with the errno of the Fxfs error causing it, if one did.
fn volume_error(error: anyhow::Error) -> Error {
    match error.chain().any(|cause| cause.downcast_ref::<FxfsError>().is_some()) {
        true => {
            let errno = Errno::of_fxfs(&error);
            Error::new(error.context(errno))
        }
        false => Error::new(error),
    }
}
