// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/volume.rs`: FxVolume as the owner of its
// store's node cache and pager, copied verbatim except as follows.
//
// - A volume has no parent `VolumesDirectory`, so the pager's dirty bytes are counted per volume
//   only, and `report_pager_dirty` marks pages dirty without waiting for other volumes' flushes.
// - The directory entry cache, profiles, blobs, the background flush task (which follows the
//   system's memory pressure through FIDL) and the fuchsia.fxfs protocols are not ported, so
//   neither are FxVolume's fields and methods for them.
// - Trace counters name the volume with `name.as_str()`, which the host's no-op trace backend needs
//   for argument keys (Fuchsia's takes any trace string).

use crate::fuchsia::file::{FlushType, FxFile};
use crate::fuchsia::node::{FxNode, NodeCache};
use crate::fuchsia::pager::Pager;
use anyhow::Error;
use fuchsia_async as fasync;
use fuchsia_async::epoch::Epoch;
#[cfg(any(test, feature = "testing"))]
use futures::FutureExt;
use fxfs::filesystem::SyncOptions;
use fxfs::future_with_guard::FutureWithGuard;
use fxfs::log::*;
use fxfs::object_store::{HandleOwner, ObjectStore};
use std::future::Future;
#[cfg(any(test, feature = "testing"))]
use std::pin::pin;
use std::sync::Arc;
#[cfg(any(test, feature = "testing"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use vfs::execution_scope::ExecutionScope;

/// The read ahead/around size to target. Increase reads to be this size within the restrictions of
/// the format for the target object.
pub const READ_AHEAD_SIZE: u64 = 128 * 1024;

/// FxVolume represents an opened volume. It is also a (weak) cache for all opened Nodes within the
/// volume.
pub struct FxVolume {
    cache: NodeCache,
    store: Arc<ObjectStore>,
    pager: Pager,
    executor: fasync::EHandle,
    name: String,

    // Unique identifier of the filesystem that owns this volume.
    fs_id: u64,

    // The execution scope for this volume.
    scope: ExecutionScope,

    #[cfg(any(test, feature = "testing"))]
    poisoned: AtomicBool,

    /// The number of dirty bytes in pager backed VMOs that belong to this volume. VolumesDirectory
    /// holds a count for all volumes. This count is only used for tracing.
    pager_dirty_byte_count: AtomicU64,
}

#[fxfs_trace::trace]
impl FxVolume {
    pub fn new(store: Arc<ObjectStore>, fs_id: u64, name: String) -> Result<Self, Error> {
        let scope = ExecutionScope::new();
        Ok(Self {
            cache: NodeCache::new(),
            store,
            name,
            pager: Pager::new(scope.clone())?,
            executor: fasync::EHandle::local(),
            fs_id,
            scope,
            #[cfg(any(test, feature = "testing"))]
            poisoned: AtomicBool::new(false),
            pager_dirty_byte_count: AtomicU64::new(0),
        })
    }

    pub fn store(&self) -> &Arc<ObjectStore> {
        &self.store
    }

    pub fn cache(&self) -> &NodeCache {
        &self.cache
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

    pub async fn terminate(&self) {
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

    /// Reports that a certain number of bytes will be dirtied in a pager-backed VMO.
    pub fn report_pager_dirty(
        self: Arc<Self>,
        byte_count: u64,
        mark_dirty: impl FnOnce() + Send + 'static,
    ) {
        let prev = self.pager_dirty_byte_count.fetch_add(byte_count, Ordering::Relaxed);
        mark_dirty();
        fxfs_trace::counter!("dirty-bytes", 0, self.name.as_str() => prev.saturating_add(byte_count));
    }

    /// Reports that a certain number of bytes were cleaned in a pager-backed VMO.
    pub fn report_pager_clean(&self, byte_count: u64) {
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
                    "pager strong file refs={}, no tasks={}",
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
