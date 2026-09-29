// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/volumes_directory.rs`: VolumesDirectory as
// the owner of a filesystem's mounted volumes, their memory pressure monitor and the count of the
// pages they have dirtied, which it limits under critical memory pressure, copied verbatim except
// as follows.
//
// - Volumes are not served: there is no directory node, outgoing directory, admin scope, or
//   fuchsia.fs.startup, fuchsia.fs or fuchsia.fxfs protocol, so neither are the methods serving
//   them, nor auto-unmounting (so mounts have no sequence numbers), checking, removing or
//   installing volumes. `VolumesDirectory::new` adds no directory entries for the volumes.
// - Profiles, blobs and inspect are not ported, so `VolumesDirectory::new` takes no inspect tree,
//   and there is no callback on mounting.
// - The tests are the ones for what is ported, which open volumes in-process.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::memory_pressure::{MemoryPressureLevel, MemoryPressureMonitor};
use crate::fuchsia::volume::{FxVolume, FxVolumeAndRoot, MemoryPressureConfig, RootDir};
use anyhow::{Context, Error, ensure};
use fidl_fuchsia_fs_startup::CreateOptions;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use fxfs::errors::FxfsError;
use fxfs::log::*;
use fxfs::object_store::directory::Directory;
use fxfs::object_store::volume::RootVolume;
use fxfs::object_store::{NewChildStoreOptions, ObjectStore, StoreOptions};
use fxfs_crypto::Crypt;
use fxfs_trace::{TraceFutureExt, trace_future_args};
use refaults_vmo::PageRefaultCounter;
use rustc_hash::FxHashMap as HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
const MEBIBYTE: u64 = 1024 * 1024;

/// VolumesDirectory is a special pseudo-directory used to enumerate and operate on volumes.
/// Volume creation happens via fuchsia.fs.startup.Volumes.Create, rather than open.
///
/// Note that VolumesDirectory assumes exclusive access to |root_volume| and if volumes are
/// manipulated from elsewhere, strange things will happen.
pub struct VolumesDirectory {
    root_volume: RootVolume,
    mounted_volumes: futures::lock::Mutex<HashMap<u64, MountedVolume>>,
    mem_monitor: Option<MemoryPressureMonitor>,
    blob_resupplied_count: Arc<PageRefaultCounter>,

    /// A running estimate of the number of dirty bytes outstanding in all pager-backed VMOs across
    /// all volumes.
    pager_dirty_bytes_count: PagerDirtyByteCount,

    /// Max outstanding dirty bytes under critical memory pressure. This could be hardcoded, but is
    /// broken out for testing.
    max_dirty_bytes_when_critical: AtomicU64,

    /// The cache configuration to use under different memory pressure levels.
    memory_pressure_config: MemoryPressureConfig,
}

/// Operations on VolumesDirectory that cannot be performed concurrently (i.e. most
/// volume creation/removal ops) should exist on this guard instead of VolumesDirectory.
pub struct MountedVolumesGuard<'a> {
    volumes_directory: Arc<VolumesDirectory>,
    mounted_volumes: futures::lock::MutexGuard<'a, HashMap<u64, MountedVolume>>,
}

struct MountedVolume {
    volume: FxVolumeAndRoot,

    // True if the volume was forcibly locked.
    locked: bool,
}

pub(crate) enum Mode {
    Mount,
    Create { guid: Option<[u8; 16]>, low_32_bit_object_ids: bool },
}

impl MountedVolumesGuard<'_> {
    /// Creates or mounts a volume. If |crypt| is set, the volume will be created or mounted as
    /// encrypted. The volume is mounted according to |as_blob|.
    async fn create_or_mount_volume(
        &mut self,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        mode: Mode,
        as_blob: bool,
    ) -> Result<FxVolumeAndRoot, Error> {
        ensure!(!as_blob, "blob volumes are not ported");
        let store = match mode {
            Mode::Create { guid, low_32_bit_object_ids } => self
                .volumes_directory
                .root_volume
                .new_volume(
                    name,
                    NewChildStoreOptions {
                        options: StoreOptions { crypt },
                        guid,
                        low_32_bit_object_ids,
                        ..Default::default()
                    },
                )
                .await
                .context("failed to create new volume")?,
            Mode::Mount => {
                self.volumes_directory.root_volume.volume(name, StoreOptions { crypt }).await?
            }
        };
        ensure!(
            !self.mounted_volumes.contains_key(&store.store_object_id()),
            FxfsError::AlreadyBound
        );

        let volume = self
            .mount_store::<FxDirectory>(name, store, self.volumes_directory.memory_pressure_config)
            .await?;
        Ok(volume)
    }

    // Mounts the given store.  A lock *must* be held on the volume directory.
    async fn mount_store<T: From<Directory<FxVolume>> + RootDir>(
        &mut self,
        name: &str,
        store: Arc<ObjectStore>,
        flush_task_config: MemoryPressureConfig,
    ) -> Result<FxVolumeAndRoot, Error> {
        let unique_id = zx::Event::create();
        let volume = FxVolumeAndRoot::new::<T>(
            Arc::downgrade(&self.volumes_directory),
            store,
            unique_id.koid().unwrap().raw_koid(),
            name.to_owned(),
            self.volumes_directory.blob_resupplied_count.clone(),
            self.volumes_directory.memory_pressure_config,
        )
        .await?;
        volume
            .volume()
            .start_background_task(flush_task_config, self.volumes_directory.mem_monitor.as_ref());
        self.add_mount(name, &volume);
        Ok(volume)
    }

    /// Adds a volume (`FxVolumeAndRoot`) into the mount list.
    pub fn add_mount(&mut self, _name: &str, volume: &FxVolumeAndRoot) {
        self.mounted_volumes.insert(
            volume.volume().store().store_object_id(),
            MountedVolume { volume: volume.clone(), locked: false },
        );
    }

    async fn lock_mount(&self, mounted_volume: &mut MountedVolume) {
        let MountedVolume { volume, locked, .. } = mounted_volume;

        if !*locked {
            volume.volume().terminate().await;
            *locked = true;
        }
    }

    async fn terminate(&mut self) {
        let mut volumes = std::mem::take(&mut *self.mounted_volumes);
        for mounted_volume in volumes.values_mut() {
            self.lock_mount(mounted_volume).await;
        }
    }

    // Unmounts the volume identified by `store_id`.  The caller should take locks to avoid races if
    // necessary.
    pub async fn unmount(&mut self, store_id: u64) -> Result<FxVolumeAndRoot, Error> {
        let mut mounted_volume =
            self.mounted_volumes.remove(&store_id).ok_or(FxfsError::NotFound)?;
        self.lock_mount(&mut mounted_volume).await;
        Ok(mounted_volume.volume)
    }
}

impl VolumesDirectory {
    /// Creates a VolumesDirectory for the volumes in |root_volume|.  No volume is opened during
    /// this.
    pub async fn new(
        root_volume: RootVolume,
        mem_monitor: Option<MemoryPressureMonitor>,
        blob_resupplied_count: Arc<PageRefaultCounter>,
        memory_pressure_config: MemoryPressureConfig,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            root_volume,
            mounted_volumes: futures::lock::Mutex::new(HashMap::default()),
            mem_monitor,
            blob_resupplied_count,
            pager_dirty_bytes_count: PagerDirtyByteCount::new(),
            max_dirty_bytes_when_critical: AtomicU64::new(zx::system_get_physmem() / 100),
            memory_pressure_config,
        }))
    }

    pub fn memory_pressure_monitor(&self) -> Option<&MemoryPressureMonitor> {
        self.mem_monitor.as_ref()
    }

    // This serves as an exclusive lock for operations that manipulate the set of mounted volumes.
    pub async fn lock<'a>(self: &'a Arc<Self>) -> MountedVolumesGuard<'a> {
        MountedVolumesGuard {
            volumes_directory: self.clone(),
            mounted_volumes: self.mounted_volumes.lock().await,
        }
    }

    /// Creates and mounts a new volume. If |crypt| is set, the volume will be encrypted. The
    /// volume is mounted according to |as_blob|.
    pub async fn create_and_mount_volume(
        self: &Arc<Self>,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        as_blob: bool,
        options: CreateOptions,
    ) -> Result<FxVolumeAndRoot, Error> {
        self.lock()
            .await
            .create_or_mount_volume(
                name,
                crypt,
                Mode::Create {
                    guid: options.guid,
                    low_32_bit_object_ids: options.restrict_inode_ids_to_32_bit.unwrap_or(false),
                },
                as_blob,
            )
            .await
    }

    /// Mounts an existing volume. `crypt` will be used to unlock the volume if provided.
    /// If `as_blob` is `true`, the volume will be mounted as a blob filesystem, otherwise
    /// it will be treated as a regular fxfs volume.
    pub async fn mount_volume(
        self: &Arc<Self>,
        name: &str,
        crypt: Option<Arc<dyn Crypt>>,
        as_blob: bool,
    ) -> Result<FxVolumeAndRoot, Error> {
        self.lock().await.create_or_mount_volume(name, crypt, Mode::Mount, as_blob).await
    }

    /// Terminates all opened volumes.
    pub async fn terminate(self: &Arc<Self>) {
        self.lock().await.terminate().await;
        // TODO(https://fxbug.dev/452935329): Turn this into a real assert.
        debug_assert!(
            self.pager_dirty_bytes_count.load() == 0,
            "Leaked {} dirty bytes.",
            self.pager_dirty_bytes_count.load()
        );
    }

    pub fn memory_pressure_config(&self) -> &MemoryPressureConfig {
        &self.memory_pressure_config
    }

    fn is_flush_required_to_dirty(&self, byte_count: u64) -> bool {
        let mem_pressure = self
            .mem_monitor
            .as_ref()
            .map(|mem_monitor| mem_monitor.level())
            .unwrap_or(MemoryPressureLevel::Normal);
        if !matches!(mem_pressure, MemoryPressureLevel::Critical) {
            return false;
        }

        let total_dirty = self.pager_dirty_bytes_count.load();
        total_dirty + byte_count >= self.max_dirty_bytes_when_critical.load(Ordering::Relaxed)
    }

    /// Reports that a certain number of bytes will be dirtied in a pager-backed VMO. If the memory
    /// pressure level is critical and fxfs has lots of dirty pages then a new task will be spawned
    /// in `volume` to flush the dirty pages before `mark_dirty` is called. If the memory pressure
    /// level is not critical then `mark_dirty` will be synchronously called.
    pub fn report_pager_dirty(
        self: Arc<Self>,
        byte_count: u64,
        volume: Arc<FxVolume>,
        mark_dirty: impl FnOnce() + Send + 'static,
    ) {
        if !self.is_flush_required_to_dirty(byte_count) {
            self.pager_dirty_bytes_count.fetch_add(byte_count);
            mark_dirty();
        } else {
            volume.spawn(
                async move {
                    let volumes = self.mounted_volumes.lock().await;

                    // Re-check the number of outstanding pager dirty bytes because another thread
                    // could have raced and flushed the volumes first.
                    if self.is_flush_required_to_dirty(byte_count) {
                        debug!(
                            "Flushing all volumes. Memory pressure is critical & dirty pager bytes \
                            ({} MiB) >= limit ({} MiB)",
                            self.pager_dirty_bytes_count.load() / MEBIBYTE,
                            self.max_dirty_bytes_when_critical.load(Ordering::Relaxed) / MEBIBYTE
                        );

                        let flushes = FuturesUnordered::new();
                        for MountedVolume { volume, .. } in volumes.values() {
                            let vol = volume.volume().clone();
                            flushes.push(async move {
                                vol.minimize_memory().await;
                            });
                        }

                        flushes.collect::<()>().await;
                    }
                    self.pager_dirty_bytes_count.fetch_add(byte_count);
                    mark_dirty();
                }
                .trace(trace_future_args!("flush-before-mark-dirty")),
            )
        }
    }

    /// Reports that a certain number of bytes were cleaned in a pager-backed VMO.
    pub fn report_pager_clean(&self, byte_count: u64) {
        let prev_dirty = self.pager_dirty_bytes_count.fetch_sub(byte_count);
        // TODO(https://fxbug.dev/452935329): Turn this into a real assert.
        debug_assert!(prev_dirty >= byte_count, "Underflowed dirty bytes.");

        if prev_dirty < byte_count {
            // An unlikely scenario, but if there was an underflow, reset the pager dirty bytes to
            // zero.
            self.pager_dirty_bytes_count.store(0);
        }
    }
}

struct PagerDirtyByteCount(AtomicU64);

impl PagerDirtyByteCount {
    pub fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn fetch_add(&self, value: u64) -> u64 {
        let prev = self.0.fetch_add(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => prev.saturating_add(value));
        prev
    }

    pub fn fetch_sub(&self, value: u64) -> u64 {
        let prev = self.0.fetch_sub(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => prev.saturating_sub(value));
        prev
    }

    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn store(&self, value: u64) {
        self.0.store(value, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, "total" => value);
    }
}

#[cfg(test)]
mod tests {
    use crate::fuchsia::memory_pressure::MemoryPressureLevel;
    use crate::fuchsia::testing::{TestFixture, open_file_checked};
    use crate::fuchsia::volume::MemoryPressureConfig;
    use crate::fuchsia::volumes_directory::VolumesDirectory;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_fs_startup::CreateOptions;
    use fidl_fuchsia_io as fio;
    use fuchsia_fs::file;
    use fxfs::errors::FxfsError;
    use fxfs::filesystem::FxFilesystem;
    use fxfs::object_store::volume::root_volume;
    use fxfs_crypto::Crypt;
    use fxfs_insecure_crypto::new_insecure_crypt;
    use refaults_vmo::PageRefaultCounter;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use storage_units::page_size;

    #[fuchsia::test]
    async fn test_volume_creation() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        {
            let vol = volumes_directory
                .create_and_mount_volume(
                    "encrypted",
                    Some(crypt.clone()),
                    false,
                    CreateOptions::default(),
                )
                .await
                .expect("create encrypted volume failed");
            vol.volume().store().store_object_id()
        };

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let error = volumes_directory
            .create_and_mount_volume(
                "encrypted",
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .err()
            .expect("Creating existing encrypted volume should fail");
        assert!(FxfsError::AlreadyExists.matches(&error));
    }

    #[fuchsia::test]
    async fn test_dirty_pages_accumulate_in_parent() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.unwrap();
        let blob_resupplied_count =
            Arc::new(PageRefaultCounter::new().expect("Failed to create PageRefaultCounter"));
        let volumes_directory = VolumesDirectory::new(
            root_volume(filesystem.clone()).await.unwrap(),
            None,
            blob_resupplied_count,
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();

        let crypt = Arc::new(new_insecure_crypt()) as Arc<dyn Crypt>;
        let vol = volumes_directory
            .create_and_mount_volume(
                "encrypted",
                Some(crypt.clone()),
                false,
                CreateOptions::default(),
            )
            .await
            .expect("create encrypted volume failed");
        let old_dirty = volumes_directory.pager_dirty_bytes_count.load();

        let new_dirty = {
            let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
            vol.root().clone().serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);
            let f = open_file_checked(
                &root,
                "foo",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let buf = vec![0xaa as u8; 8192];
            file::write(&f, buf.as_slice()).await.expect("Write");
            // It's important to check the dirty bytes before closing the file, as closing can
            // trigger a flush.
            volumes_directory.pager_dirty_bytes_count.load()
        };
        assert_ne!(old_dirty, new_dirty);

        volumes_directory.terminate().await;
        std::mem::drop(volumes_directory);
        filesystem.close().await.expect("close filesystem failed");
    }

    // This mostly just ensures that we exercise the code path. It was added because the path at
    // one point contained a deadlock.
    #[fuchsia::test(threads = 10)]
    async fn test_flush_before_mark_dirty_under_critical_memory_pressure() {
        let fixture = TestFixture::new().await;
        // Memory is critical, and it's always our fault.
        let _ = fixture
            .memory_pressure_proxy()
            .on_level_changed(MemoryPressureLevel::Critical)
            .await
            .expect("memory pressure FIDL");
        fixture.volumes_directory().max_dirty_bytes_when_critical.store(1, Ordering::Relaxed);

        let root = fixture.root();
        let file = open_file_checked(
            &root,
            "foo",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.resize((page_size() * 2).into()).await.expect("resize (FIDL)").expect("resize failed");
        // The above resize creates zero pages which aren't dirty pages and don't contribute to the
        // dirty bytes count but still need to be flushed. The first write below will cross the
        // `max_dirty_bytes_when_critical` threshold but `minimize_memory` won't flush the file
        // because the zero pages aren't dirty pages. The second write below will also cross the
        // `max_dirty_bytes_when_critical` threshold and since the first write dirtied a page, a
        // flush will occur. The flush cleans the 2nd page which is a zero page and the kernel is
        // actively trying to dirty. The kernel doesn't end up dirtying the 2nd page because it's
        // concerned that the clean and dirty raced so it issues another dirty request for the same
        // page. The duplicate dirty request again crosses the `max_dirty_bytes_when_critical`
        // threshold. The file thinks that it has a dirty page so `minimize_memory` flushes the
        // file. `zx_pager_query_dirty_ranges` doesn't return any pages because the kernel didn't
        // actually dirty the page that fxfs thought it did. This causes only the file metadata to
        // be flushed and the dirty page count to not be reduced. The 2nd page finally gets dirtied
        // and now fxfs thinks it has 2 dirty pages instead of 1. `PagedObjectHandle` knows that
        // this can happen and will fix up the counts on the next flush. This test doesn't want to
        // deal with all of that so it syncs here to clean the zero pages that would cause that
        // mess.
        file.sync().await.expect("Failed to make sync call").expect("sync failed");

        let vmo = file
            .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::WRITE)
            .await
            .expect("get_backing_memory (FIDL)")
            .expect("get_backing_memory");

        let buf = [0xAAu8];
        // One call to get dirty bytes over 0, the second to force a flush during mark_dirty.
        vmo.write(&buf, 0).expect("Writing to create dirty bytes");
        let before = fixture.volumes_directory().pager_dirty_bytes_count.load();
        vmo.write(&buf, page_size().get()).expect("Writing to force a flush during mark_dirty");
        // This is still the page size because we forced a flush of the first write during the
        // second write.
        assert_eq!(fixture.volumes_directory().pager_dirty_bytes_count.load(), before,);

        fixture.close().await;
    }
}
