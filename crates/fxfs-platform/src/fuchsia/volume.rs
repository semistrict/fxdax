// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/volume.rs`: FxVolume as the owner of its
// store's node cache and pager, and its background task, which flushes its files periodically and
// follows memory pressure, copied verbatim except as follows.
//
// - Profiles, blobs, inspect and the fuchsia.fxfs protocols are not ported, so neither are
//   FxVolume's fields and methods for them. `FxVolume::new` takes the blob refault counter it does
//   upstream, and ignores it.
// - The background task's timer is fused, and replaced to reset it, as the host's fuchsia-async
//   timer is not fused and has no `reset`.
// - The tests are the ones for the background task.
// - `RootDir` isn't a directory entry of pseudo directories, and `FxVolumeAndRoot` has no admin
//   scope or outgoing directory for fuchsia.fxfs services.
// - Trace counters name the volume with `name.as_str()`, which the host's no-op trace backend needs
//   for argument keys (Fuchsia's takes any trace string).
// - fxdax: `volumes_directory` gives the volume's parent, whose shared blocks its files use.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::dirent_cache::DirentCache;
use crate::fuchsia::file::{FlushType, FxFile};
use crate::fuchsia::memory_pressure::{MemoryPressureLevel, MemoryPressureMonitor};
use crate::fuchsia::node::{FxNode, GetResult, NodeCache};
use crate::fuchsia::pager::Pager;
use crate::fuchsia::symlink::FxSymlink;
use crate::fuchsia::volumes_directory::VolumesDirectory;
use anyhow::{Error, bail};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_async::epoch::Epoch;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::stream::{self, FusedStream, Stream};
use futures::{FutureExt, StreamExt};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{self, SyncOptions, TruncateGuard};
use fxfs::future_with_guard::FutureWithGuard;
use fxfs::log::*;
use fxfs::object_store::directory::Directory;
use fxfs::object_store::transaction::Options;
use fxfs::object_store::{HandleOptions, HandleOwner, ObjectDescriptor, ObjectStore};
use refaults_vmo::PageRefaultCounter;
use std::future::Future;
use std::pin::pin;
#[cfg(any(test, feature = "testing"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use storage_units::BlockSize;
use vfs::execution_scope::ExecutionScope;

/// The read ahead/around size to target. Increase reads to be this size within the restrictions of
/// the format for the target object.
pub const READ_AHEAD_SIZE: u64 = 128 * 1024;

// LINT.IfChange
// TODO:(b/299919008) Fix this number to something reasonable, or maybe just for fxblob.
const DIRENT_CACHE_LIMIT: usize = 2000;
// LINT.ThenChange(//src/storage/stressor/src/aggressive.rs)

#[derive(Clone, Copy, Debug)]
pub struct MemoryPressureLevelConfig {
    /// The period to wait between flushes, as well as perform other background maintenance tasks
    /// (e.g. purging caches).
    pub background_task_period: Duration,

    /// The limit of cached nodes.
    pub cache_size_limit: usize,

    /// The initial delay before the background task runs. The background task has a longer initial
    /// delay to avoid running the task during boot.
    pub background_task_initial_delay: Duration,
}

impl Default for MemoryPressureLevelConfig {
    fn default() -> Self {
        Self {
            background_task_period: Duration::from_secs(20),
            cache_size_limit: DIRENT_CACHE_LIMIT,
            background_task_initial_delay: Duration::from_secs(70),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryPressureConfig {
    /// The configuration to use at [`MemoryPressureLevel::Normal`].
    pub mem_normal: MemoryPressureLevelConfig,

    /// The configuration to use at [`MemoryPressureLevel::Warning`].
    pub mem_warning: MemoryPressureLevelConfig,

    /// The configuration to use at [`MemoryPressureLevel::Critical`].
    pub mem_critical: MemoryPressureLevelConfig,
}

impl MemoryPressureConfig {
    pub fn for_level(&self, level: &MemoryPressureLevel) -> &MemoryPressureLevelConfig {
        match level {
            MemoryPressureLevel::Normal => &self.mem_normal,
            MemoryPressureLevel::Warning => &self.mem_warning,
            MemoryPressureLevel::Critical => &self.mem_critical,
        }
    }
}

impl Default for MemoryPressureConfig {
    fn default() -> Self {
        // TODO(https://fxbug.dev/42061389): investigate a smarter strategy for determining flush
        // frequency.
        Self {
            mem_normal: MemoryPressureLevelConfig {
                background_task_period: Duration::from_secs(20),
                cache_size_limit: DIRENT_CACHE_LIMIT,
                background_task_initial_delay: Duration::from_secs(70),
            },
            mem_warning: MemoryPressureLevelConfig {
                background_task_period: Duration::from_secs(5),
                cache_size_limit: 100,
                background_task_initial_delay: Duration::from_secs(5),
            },
            mem_critical: MemoryPressureLevelConfig {
                background_task_period: Duration::from_millis(1500),
                cache_size_limit: 20,
                background_task_initial_delay: Duration::from_millis(1500),
            },
        }
    }
}

/// FxVolume represents an opened volume. It is also a (weak) cache for all opened Nodes within the
/// volume.
pub struct FxVolume {
    parent: Weak<VolumesDirectory>,
    cache: NodeCache,
    store: Arc<ObjectStore>,
    pager: Pager,
    executor: fasync::EHandle,
    name: String,

    // A tuple of the actual task and a channel to signal to terminate the task.
    background_task: Mutex<Option<(fasync::Task<()>, oneshot::Sender<()>)>>,

    // Unique identifier of the filesystem that owns this volume.
    fs_id: u64,

    // The execution scope for this volume.
    scope: ExecutionScope,

    dirent_cache: DirentCache,

    #[cfg(any(test, feature = "testing"))]
    poisoned: AtomicBool,

    /// The number of dirty bytes in pager backed VMOs that belong to this volume. VolumesDirectory
    /// holds a count for all volumes. This count is only used for tracing.
    pager_dirty_byte_count: AtomicU64,
}

#[fxfs_trace::trace]
impl FxVolume {
    pub fn new(
        parent: Weak<VolumesDirectory>,
        store: Arc<ObjectStore>,
        fs_id: u64,
        name: String,
        _blob_resupplied_count: Arc<PageRefaultCounter>,
        memory_pressure_config: MemoryPressureConfig,
    ) -> Result<Self, Error> {
        let scope = ExecutionScope::new();
        Ok(Self {
            parent,
            cache: NodeCache::new(),
            store,
            name,
            pager: Pager::new(scope.clone())?,
            executor: fasync::EHandle::local(),
            background_task: Mutex::new(None),
            fs_id,
            scope,
            dirent_cache: DirentCache::new(memory_pressure_config.mem_normal.cache_size_limit),
            #[cfg(any(test, feature = "testing"))]
            poisoned: AtomicBool::new(false),
            pager_dirty_byte_count: AtomicU64::new(0),
        })
    }

    pub fn store(&self) -> &Arc<ObjectStore> {
        &self.store
    }

    /// fxdax: the directory of the volumes this volume is one of, if it is mounted from one.
    pub fn volumes_directory(&self) -> Option<Arc<VolumesDirectory>> {
        self.parent.upgrade()
    }

    pub fn cache(&self) -> &NodeCache {
        &self.cache
    }

    pub fn dirent_cache(&self) -> &DirentCache {
        &self.dirent_cache
    }

    pub fn pager(&self) -> &Pager {
        &self.pager
    }

    pub fn id(&self) -> u64 {
        self.fs_id
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reports the filesystem info, but if the volume has a space limit applied then the space
    /// available and space used are reported based on the volume instead.
    pub fn filesystem_info_for_volume(&self) -> fio::FilesystemInfo {
        let allocator = self.store.filesystem().allocator();
        let info =
            if let Some(limit) = allocator.get_owner_bytes_limit(self.store.store_object_id()) {
                filesystem::Info {
                    used_bytes: allocator.get_owner_bytes_used(self.store.store_object_id()),
                    total_bytes: limit,
                }
            } else {
                self.store.filesystem().get_info()
            };

        info_to_filesystem_info(info, self.store.block_size(), self.store.object_count(), self.id())
    }

    pub async fn terminate(&self) {
        let task = std::mem::replace(&mut *self.background_task.lock(), None);
        if let Some((task, terminate)) = task {
            let _ = terminate.send(());
            task.await;
        }

        // `NodeCache::terminate` will break any strong reference cycles contained within nodes
        // (pager registration). The only remaining nodes should be those with open FIDL
        // connections or vmo references in the process of handling the VMO_ZERO_CHILDREN signal.
        // `ExecutionScope::shutdown` + `ExecutionScope::wait` will close the open FIDL connections
        // and synchonrize the signal handling which should result in all nodes flushing and then
        // dropping. Any async tasks required to flush a node should take an active guard on the
        // `ExecutionScope` which will prevent `ExecutionScope::wait` from completing until all
        // nodes are flushed.
        self.scope.shutdown();
        self.cache.terminate();
        self.scope.wait().await;

        // Make sure there are no deferred operations still pending for this volume.
        Epoch::global().barrier().await;

        // The dirent_cache must be cleared *after* shutting down the scope because there can be
        // tasks that insert entries into the cache.
        self.dirent_cache.clear();

        if self.store.filesystem().options().read_only {
            // If the filesystem is read only, we don't need to flush/sync anything.
            if self.store.is_unlocked() {
                self.store.lock_read_only();
            }
            return;
        }

        self.flush_all_files(FlushType::LastChance).await;
        self.store.filesystem().graveyard().flush().await;
        if self.store.crypt().is_some() {
            if let Err(e) = self.store.lock().await {
                // The store will be left in a safe state and there won't be data-loss unless
                // there's an issue flushing the journal later.
                warn!(error:? = e; "Locking store error");
            }
        }
        let sync_status = self
            .store
            .filesystem()
            .sync(SyncOptions { flush_device: true, ..Default::default() })
            .await;
        if let Err(e) = sync_status {
            error!(error:? = e; "Failed to sync filesystem; data may be lost");
        }
    }

    /// Attempts to get a node from the node cache. If the node wasn't present in the cache, loads
    /// the object from the object store, installing the returned node into the cache and returns
    /// the newly created FxNode backed by the loaded object.  |parent| is only set on the node if
    /// the node was not present in the cache.  Otherwise, it is ignored.
    pub async fn get_or_load_node(
        self: &Arc<Self>,
        object_id: u64,
        object_descriptor: ObjectDescriptor,
        parent: Option<Arc<FxDirectory>>,
    ) -> Result<Arc<dyn FxNode>, Error> {
        match self.cache.get_or_reserve(object_id).await {
            GetResult::Node(node) => Ok(node),
            GetResult::Placeholder(placeholder) => {
                let node = match object_descriptor {
                    ObjectDescriptor::File => FxFile::new(
                        ObjectStore::open_object(self, object_id, HandleOptions::default(), None)
                            .await?,
                    ) as Arc<dyn FxNode>,
                    ObjectDescriptor::Directory => {
                        // Can't use open_unchecked because we don't know if the dir is casefolded
                        // or encrypted.
                        Arc::new(FxDirectory::new(parent, Directory::open(self, object_id).await?))
                            as Arc<dyn FxNode>
                    }
                    ObjectDescriptor::Symlink => Arc::new(FxSymlink::new(self.clone(), object_id)),
                    _ => bail!(FxfsError::Inconsistent),
                };
                placeholder.commit(&node);
                Ok(node)
            }
        }
    }

    /// Marks the given directory deleted.
    pub fn mark_directory_deleted(&self, object_id: u64) {
        if let Some(node) = self.cache.get(object_id) {
            // It's possible that node is a placeholder, in which case we don't need to wait for it
            // to be resolved because it should be blocked behind the locks that are held by the
            // caller, and once they're dropped, it'll be found to be deleted via the tree.
            if let Ok(dir) = node.into_any().downcast::<FxDirectory>() {
                dir.set_deleted();
            }
        }
    }

    /// Removes resources associated with |object_id| (which ought to be a file), if there are no
    /// open connections to that file.
    ///
    /// This must be called *after committing* a transaction which deletes the last reference to
    /// |object_id|, since before that point, new connections could be established.
    pub(super) async fn maybe_purge_file(
        &self,
        object_id: u64,
        truncate_guard: Option<&TruncateGuard<'_>>,
    ) -> Result<(), Error> {
        if let Some(node) = self.cache.get(object_id) {
            node.clone().mark_to_be_purged();
            return Ok(());
        }
        // If this fails, the graveyard should clean it up on next mount.
        let txn_options = Options { borrow_metadata_space: true, ..Default::default() };
        self.store.tombstone_object(object_id, txn_options, truncate_guard).await?;
        Ok(())
    }

    /// Starts the background work task.  This task will periodically:
    ///   - scan all files and flush them to disk, and
    ///   - purge unused cached data.
    /// The task will hold a strong reference to the FxVolume while it is running, so the task must
    /// be closed later with Self::terminate, or the FxVolume will never be dropped.
    pub fn start_background_task(
        self: &Arc<Self>,
        config: MemoryPressureConfig,
        mem_monitor: Option<&MemoryPressureMonitor>,
    ) {
        let mut background_task = self.background_task.lock();
        if background_task.is_none() {
            let (tx, rx) = oneshot::channel();

            let task = if let Some(mem_monitor) = mem_monitor {
                fasync::Task::spawn(self.clone().background_task(
                    config,
                    mem_monitor.get_level_stream(),
                    rx,
                ))
            } else {
                // With no memory pressure monitoring, just stub the stream out as always pending.
                fasync::Task::spawn(self.clone().background_task(config, stream::pending(), rx))
            };

            *background_task = Some((task, tx));
        }
    }

    #[trace]
    async fn background_task(
        self: Arc<Self>,
        config: MemoryPressureConfig,
        mut level_stream: impl Stream<Item = MemoryPressureLevel> + FusedStream + Unpin,
        terminate: oneshot::Receiver<()>,
    ) {
        debug!(store_id = self.store.store_object_id(); "FxVolume::background_task start");
        let mut terminate = terminate.fuse();
        // Default to the normal period until updates come from the `level_stream`.
        let mut level = MemoryPressureLevel::Normal;
        let mut timer =
            pin!(fasync::Timer::new(config.for_level(&level).background_task_initial_delay).fuse());

        loop {
            let mut should_terminate = false;
            let mut should_flush = false;
            let mut low_mem = false;
            let mut should_purge_layer_files = false;
            let mut should_update_cache_limit = false;

            futures::select_biased! {
                _ = terminate => should_terminate = true,
                new_level = level_stream.next() => {
                    // Because `level_stream` will never terminate, this is safe to unwrap.
                    let new_level = new_level.unwrap();
                    // At critical levels, it's okay to undertake expensive work immediately
                    // to reclaim memory.
                    low_mem = matches!(new_level, MemoryPressureLevel::Critical);
                    should_purge_layer_files = true;
                    if new_level != level {
                        level = new_level;
                        should_update_cache_limit = true;
                        let level_config = config.for_level(&level);
                        timer.set(fasync::Timer::new(level_config.background_task_period).fuse());
                        debug!(
                            "Background task period changed to {:?} due to new memory pressure \
                            level ({:?}).",
                            config.for_level(&level).background_task_period, level
                        );
                    }
                }
                _ = timer => {
                    timer.set(
                        fasync::Timer::new(config.for_level(&level).background_task_period).fuse()
                    );
                    should_flush = true;
                    // Only purge layer file caches once we have elevated memory pressure.
                    should_purge_layer_files = !matches!(level, MemoryPressureLevel::Normal);
                }
            };
            if should_terminate {
                break;
            }
            // Maybe close extra files *before* iterating them for flush/low mem.
            if should_update_cache_limit {
                self.dirent_cache.set_limit(config.for_level(&level).cache_size_limit);
            }
            if should_flush {
                self.flush_all_files(FlushType::Sync).await;
                self.dirent_cache.recycle_stale_files();
            } else if low_mem {
                // This is a softer version of flushing files, so don't bother if we're flushing.
                self.minimize_memory().await;
            }
            if should_purge_layer_files {
                for layer in self.store.tree().immutable_layer_set().layers {
                    layer.purge_cached_data();
                }
            }
        }
        debug!(store_id = self.store.store_object_id(); "FxVolume::background_task end");
    }

    /// Reports that a certain number of bytes will be dirtied in a pager-backed VMO.
    ///
    /// Note that this function may await flush tasks.
    pub fn report_pager_dirty(
        self: Arc<Self>,
        byte_count: u64,
        mark_dirty: impl FnOnce() + Send + 'static,
    ) {
        let this = self.clone();
        let callback = move || {
            let prev = this.pager_dirty_byte_count.fetch_add(byte_count, Ordering::Relaxed);
            mark_dirty();
            fxfs_trace::counter!("dirty-bytes", 0, this.name.as_str() => prev.saturating_add(byte_count));
        };
        if let Some(parent) = self.parent.upgrade() {
            parent.report_pager_dirty(byte_count, self, callback);
        } else {
            callback();
        }
    }

    /// Reports that a certain number of bytes were cleaned in a pager-backed VMO.
    pub fn report_pager_clean(&self, byte_count: u64) {
        if let Some(parent) = self.parent.upgrade() {
            parent.report_pager_clean(byte_count);
        }
        let prev = self.pager_dirty_byte_count.fetch_sub(byte_count, Ordering::Relaxed);
        fxfs_trace::counter!("dirty-bytes", 0, self.name.as_str() => prev.saturating_sub(byte_count));
    }

    #[trace]
    pub async fn flush_all_files(&self, flush_type: FlushType) {
        let mut flushed = 0;
        for file in self.cache.files() {
            if let Some(node) = file.into_opened_node() {
                if let Err(e) = FxFile::flush(&node, flush_type).await {
                    warn!(
                        store_id = self.store.store_object_id(),
                        oid = node.object_id(),
                        error:? = e;
                        "Failed to flush",
                    )
                }
                if flush_type == FlushType::LastChance {
                    let file = node.clone();
                    std::mem::drop(node);
                    file.force_clean();
                }
            }
            flushed += 1;
        }
        debug!(store_id = self.store.store_object_id(), file_count = flushed; "FxVolume flushed");
    }

    /// Flushes only files with dirty pages.
    ///
    /// `PagedObjectHandle` tracks the number dirty pages locally (except for overwrite files) which
    /// makes determining whether flushing a file will reduce the number of dirty pages efficient.
    /// `flush_all_files` checks if any metadata needs to be flushed which involves a syscall making
    /// it significantly slower when there are lots of open files without dirty pages.
    #[trace]
    pub async fn minimize_memory(&self) {
        for file in self.cache.files() {
            if let Some(node) = file.into_opened_node() {
                if let Err(e) = node.handle().minimize_memory().await {
                    warn!(
                        store_id = self.store.store_object_id(),
                        oid = node.object_id(),
                        error:? = e;
                        "Failed to flush",
                    )
                }
            }
        }
    }

    /// Spawns a short term task for the volume that includes a guard that will prevent termination.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        if let Some(guard) = self.scope.try_active_guard() {
            self.executor.spawn_detached(FutureWithGuard::new(guard, task));
        }
    }

    /// Tries to unwrap this volume.  If it fails, it will poison the volume so that when it is
    /// dropped, you get a backtrace.
    #[cfg(any(test, feature = "testing"))]
    pub fn try_unwrap(self: Arc<Self>) -> Option<FxVolume> {
        self.poisoned.store(true, Ordering::Relaxed);
        match Arc::try_unwrap(self) {
            Ok(volume) => {
                volume.poisoned.store(false, Ordering::Relaxed);
                Some(volume)
            }
            Err(this) => {
                // Log details about all the places where there might be a reference cycle.
                info!(
                    "background_task: {}, pager strong file refs={}, no tasks={}",
                    this.background_task.lock().is_some(),
                    crate::pager::STRONG_FILE_REFS.load(Ordering::Relaxed),
                    {
                        let mut no_tasks = pin!(this.scope.wait());
                        no_tasks
                            .poll_unpin(&mut std::task::Context::from_waker(
                                std::task::Waker::noop(),
                            ))
                            .is_ready()
                    },
                );
                None
            }
        }
    }
}

#[cfg(any(test, feature = "testing"))]
impl Drop for FxVolume {
    fn drop(&mut self) {
        assert!(!*self.poisoned.get_mut());
    }
}

impl HandleOwner for FxVolume {}

impl AsRef<ObjectStore> for FxVolume {
    fn as_ref(&self) -> &ObjectStore {
        &self.store
    }
}

pub trait RootDir: FxNode {
    fn serve(self: Arc<Self>, flags: fio::Flags, server_end: ServerEnd<fio::DirectoryMarker>);

    fn as_node(self: Arc<Self>) -> Arc<dyn FxNode>;
}

#[derive(Clone)]
pub struct FxVolumeAndRoot {
    volume: Arc<FxVolume>,
    root: Arc<dyn RootDir>,
}

impl FxVolumeAndRoot {
    pub async fn new<T: From<Directory<FxVolume>> + RootDir>(
        parent: Weak<VolumesDirectory>,
        store: Arc<ObjectStore>,
        unique_id: u64,
        volume_name: String,
        blob_resupplied_count: Arc<PageRefaultCounter>,
        memory_pressure_config: MemoryPressureConfig,
    ) -> Result<Self, Error> {
        let volume = Arc::new(FxVolume::new(
            parent,
            store,
            unique_id,
            volume_name,
            blob_resupplied_count,
            memory_pressure_config,
        )?);
        let root_object_id = volume.store().root_directory_object_id();
        let root_dir = Directory::open(&volume, root_object_id).await?;
        let root = Arc::<T>::new(root_dir.into()) as Arc<dyn RootDir>;
        volume
            .cache
            .get_or_reserve(root_object_id)
            .await
            .placeholder()
            .unwrap()
            .commit(&root.clone().as_node());
        Ok(Self { volume, root })
    }

    pub fn volume(&self) -> &Arc<FxVolume> {
        &self.volume
    }

    pub fn root(&self) -> &Arc<dyn RootDir> {
        &self.root
    }

    // The same as root but downcasted to FxDirectory.
    pub fn root_dir(&self) -> Arc<FxDirectory> {
        self.root().clone().into_any().downcast::<FxDirectory>().expect("Invalid type for root")
    }

    pub fn into_volume(self) -> Arc<FxVolume> {
        self.volume
    }
}

// The correct number here is arguably u64::MAX - 1 (because node 0 is reserved). There's a bug
// where inspect test cases fail if we try and use that, possibly because of a signed/unsigned bug.
// See https://fxbug.dev/42168242.  Until that's fixed, we'll have to use i64::MAX.
const TOTAL_NODES: u64 = i64::MAX as u64;

// An array used to initialize the FilesystemInfo |name| field. This just spells "fxfs" 0-padded to
// 32 bytes.
const FXFS_INFO_NAME_FIDL: [i8; 32] = [
    0x66, 0x78, 0x66, 0x73, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0,
];

fn info_to_filesystem_info(
    info: filesystem::Info,
    block_size: BlockSize,
    object_count: u64,
    fs_id: u64,
) -> fio::FilesystemInfo {
    fio::FilesystemInfo {
        total_bytes: info.total_bytes,
        used_bytes: info.used_bytes,
        total_nodes: TOTAL_NODES,
        used_nodes: object_count,
        // TODO(https://fxbug.dev/42175592): Support free_shared_pool_bytes.
        free_shared_pool_bytes: 0,
        fs_id,
        block_size: block_size.get() as u32,
        max_filename_size: fio::MAX_NAME_LENGTH as u32,
        fs_type: fidl_fuchsia_fs::VfsType::Fxfs.into_primitive(),
        padding: 0,
        name: FXFS_INFO_NAME_FIDL,
    }
}

#[cfg(test)]
mod tests {
    use super::DIRENT_CACHE_LIMIT;
    use crate::fuchsia::memory_pressure::MemoryPressureLevel;
    use crate::fuchsia::testing::{TestFixture, open_file_checked};
    use crate::fuchsia::volume::{MemoryPressureConfig, MemoryPressureLevelConfig};
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fxfs::object_store::{HandleOptions, ObjectStore};
    use std::time::Duration;

    #[fuchsia::test]
    async fn test_background_flush() {
        let fixture = TestFixture::new().await;
        {
            let file = open_file_checked(
                fixture.root(),
                "file",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let object_id = file
                .get_attributes(fio::NodeAttributesQuery::ID)
                .await
                .expect("Fidl get attr")
                .expect("get attr")
                .1
                .id
                .unwrap();

            // Write some data to the file, which will only go to the cache for now.
            file.write_at(&[123u8], 0).await.expect("FIDL write_at").expect("write_at");

            // Initialized to the default size.
            assert_eq!(fixture.volume().volume().dirent_cache().limit(), DIRENT_CACHE_LIMIT);
            let volume = fixture.volume().volume().clone();

            let data_has_persisted = || async {
                // We have to reopen the object each time since this is a distinct handle from the
                // one managed by the FxFile.
                let object =
                    ObjectStore::open_object(&volume, object_id, HandleOptions::default(), None)
                        .await
                        .expect("open_object failed");
                let data = object.contents(8192).await.expect("read failed");
                data.len() == 1 && data[..] == [123u8]
            };
            assert!(!data_has_persisted().await);

            fixture.volume().volume().start_background_task(
                MemoryPressureConfig {
                    mem_normal: MemoryPressureLevelConfig {
                        background_task_period: Duration::from_millis(100),
                        background_task_initial_delay: Duration::from_millis(100),
                        ..Default::default()
                    },
                    mem_warning: Default::default(),
                    mem_critical: Default::default(),
                },
                None,
            );

            const MAX_WAIT: Duration = Duration::from_secs(20);
            let wait_increments = Duration::from_millis(400);
            let mut total_waited = Duration::ZERO;

            while total_waited < MAX_WAIT {
                fasync::Timer::new(wait_increments).await;
                total_waited += wait_increments;

                if data_has_persisted().await {
                    break;
                }
            }

            assert!(data_has_persisted().await);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_background_flush_with_warning_memory_pressure() {
        let fixture = TestFixture::new().await;
        {
            let file = open_file_checked(
                fixture.root(),
                "file",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let object_id = file
                .get_attributes(fio::NodeAttributesQuery::ID)
                .await
                .expect("Fidl get attr")
                .expect("get attr")
                .1
                .id
                .unwrap();

            // Write some data to the file, which will only go to the cache for now.
            file.write_at(&[123u8], 0).await.expect("FIDL write_at").expect("write_at");

            // Initialized to the default size.
            assert_eq!(fixture.volume().volume().dirent_cache().limit(), DIRENT_CACHE_LIMIT);
            let volume = fixture.volume().volume().clone();

            let data_has_persisted = || async {
                // We have to reopen the object each time since this is a distinct handle from the
                // one managed by the FxFile.
                let object =
                    ObjectStore::open_object(&volume, object_id, HandleOptions::default(), None)
                        .await
                        .expect("open_object failed");
                let data = object.contents(8192).await.expect("read failed");
                data.len() == 1 && data[..] == [123u8]
            };
            assert!(!data_has_persisted().await);

            // Configure the flush task to only flush quickly on warning.
            let flush_config = MemoryPressureConfig {
                mem_normal: MemoryPressureLevelConfig {
                    background_task_period: Duration::from_secs(20),
                    cache_size_limit: DIRENT_CACHE_LIMIT,
                    ..Default::default()
                },
                mem_warning: MemoryPressureLevelConfig {
                    background_task_period: Duration::from_millis(100),
                    cache_size_limit: 100,
                    background_task_initial_delay: Duration::from_millis(100),
                    ..Default::default()
                },
                mem_critical: MemoryPressureLevelConfig {
                    background_task_period: Duration::from_secs(20),
                    cache_size_limit: 50,
                    ..Default::default()
                },
            };
            fixture.volume().volume().start_background_task(
                flush_config,
                fixture.volumes_directory().memory_pressure_monitor(),
            );

            // Send the memory pressure update.
            fixture
                .memory_pressure_proxy()
                .on_level_changed(MemoryPressureLevel::Warning)
                .await
                .expect("Failed to send memory pressure level change");

            // Wait a bit of time for the flush to occur (but less than the normal and critical
            // periods).
            const MAX_WAIT: Duration = Duration::from_secs(3);
            let wait_increments = Duration::from_millis(400);
            let mut total_waited = Duration::ZERO;

            while total_waited < MAX_WAIT {
                fasync::Timer::new(wait_increments).await;
                total_waited += wait_increments;

                if data_has_persisted().await {
                    break;
                }
            }

            assert!(data_has_persisted().await);
            assert_eq!(fixture.volume().volume().dirent_cache().limit(), 100);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_background_flush_with_critical_memory_pressure() {
        let fixture = TestFixture::new().await;
        {
            let file = open_file_checked(
                fixture.root(),
                "file",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let object_id = file
                .get_attributes(fio::NodeAttributesQuery::ID)
                .await
                .expect("Fidl get attr")
                .expect("get attr")
                .1
                .id
                .unwrap();

            // Write some data to the file, which will only go to the cache for now.
            file.write_at(&[123u8], 0).await.expect("FIDL write_at").expect("write_at");

            // Initialized to the default size.
            assert_eq!(fixture.volume().volume().dirent_cache().limit(), DIRENT_CACHE_LIMIT);
            let volume = fixture.volume().volume().clone();

            let data_has_persisted = || async {
                // We have to reopen the object each time since this is a distinct handle from the
                // one managed by the FxFile.
                let object =
                    ObjectStore::open_object(&volume, object_id, HandleOptions::default(), None)
                        .await
                        .expect("open_object failed");
                let data = object.contents(8192).await.expect("read failed");
                data.len() == 1 && data[..] == [123u8]
            };
            assert!(!data_has_persisted().await);

            let flush_config = MemoryPressureConfig {
                mem_normal: MemoryPressureLevelConfig {
                    cache_size_limit: DIRENT_CACHE_LIMIT,
                    ..Default::default()
                },
                mem_warning: MemoryPressureLevelConfig {
                    cache_size_limit: 100,
                    ..Default::default()
                },
                mem_critical: MemoryPressureLevelConfig {
                    cache_size_limit: 50,
                    ..Default::default()
                },
            };
            fixture.volume().volume().start_background_task(
                flush_config,
                fixture.volumes_directory().memory_pressure_monitor(),
            );

            // Send the memory pressure update.
            fixture
                .memory_pressure_proxy()
                .on_level_changed(MemoryPressureLevel::Critical)
                .await
                .expect("Failed to send memory pressure level change");

            // Critical memory should trigger a flush immediately so expect a flush very quickly.
            const MAX_WAIT: Duration = Duration::from_secs(2);
            let wait_increments = Duration::from_millis(400);
            let mut total_waited = Duration::ZERO;

            while total_waited < MAX_WAIT {
                fasync::Timer::new(wait_increments).await;
                total_waited += wait_increments;

                if data_has_persisted().await {
                    break;
                }
            }

            assert!(data_has_persisted().await);
            assert_eq!(fixture.volume().volume().dirent_cache().limit(), 50);
        }

        fixture.close().await;
    }
}
