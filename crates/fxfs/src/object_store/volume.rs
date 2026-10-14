// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/object_store/volume.rs`. fxdax snapshots (volume snapshots that
// share their source's layers and extents) deviate from upstream here:
// `RootVolume::snapshot_volume` makes them; deleting a volume releases its layers by reference
// count, and locks them. There is no `install_volume` (nor `replace_volume`, which it used): an image within a file is a block device's,
// which fxdax's extent store has none of (backlog doc-4).

use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, ForceMajor, FxFilesystem};
use crate::object_handle::ObjectHandle;
use crate::object_store::directory::Directory;
use crate::object_store::transaction::{LockKeys, Options, Transaction, lock_keys};
use crate::object_store::tree_cache::TreeCache;
use crate::object_store::{
    HandleOptions, INVALID_OBJECT_ID, LockKey, NewChildStoreOptions, ObjectDescriptor, ObjectStore,
    StoreOptions, load_store_info,
};
use crate::serialized_types::{LATEST_VERSION, VersionedLatest};
use anyhow::{Context, Error, anyhow, bail, ensure};
use log::error;
use std::sync::Arc;

// Volumes are a grouping of an object store and a root directory within this object store. They
// model a hierarchical tree of objects within a single store.
//
// Typically there will be one root volume which is referenced directly by the superblock. This root
// volume stores references to all other volumes on the system (as volumes/foo, volumes/bar, ...).
// For now, this hierarchy is only one deep.

pub const VOLUMES_DIRECTORY: &str = "volumes";

/// RootVolume is the top-level volume which stores references to all of the other Volumes.
pub struct RootVolume {
    _root_directory: Directory<ObjectStore>,
    filesystem: Arc<FxFilesystem>,
}

impl RootVolume {
    pub fn volume_directory(&self) -> &Directory<ObjectStore> {
        self.filesystem.object_manager().volume_directory()
    }

    /// The cache of a volume's objects' records, as large as the filesystem's options say.
    fn tree_cache(&self) -> TreeCache {
        TreeCache::with_limit(self.filesystem.options().object_cache_items)
    }

    /// Creates a new volume under a transaction lock.
    pub async fn new_volume(
        &self,
        volume_name: &str,
        options: NewChildStoreOptions,
    ) -> Result<Arc<ObjectStore>, Error> {
        let root_store = self.filesystem.root_store();
        let store;
        let mut transaction = root_store
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    self.volume_directory().object_id(),
                )],
                Options::default(),
            )
            .await?;

        ensure!(
            matches!(self.volume_directory().lookup(volume_name).await?, None),
            FxfsError::AlreadyExists
        );
        store = root_store
            .new_child_store(&mut transaction, options, Some(Box::new(self.tree_cache())))
            .await?;
        store.set_trace(self.filesystem.trace());

        // We must register the store here because create will add mutations for the store.
        self.filesystem.object_manager().add_store(store.clone());

        // If the transaction fails, we must unregister the store.
        struct CleanUp<'a>(&'a ObjectStore);
        impl Drop for CleanUp<'_> {
            fn drop(&mut self) {
                self.0.filesystem().object_manager().forget_store(self.0.store_object_id());
            }
        }
        let clean_up = CleanUp(&store);

        // Actually create the store in the transaction.
        store.create(&mut transaction).await?;

        self.volume_directory()
            .add_child_volume(&mut transaction, volume_name, store.store_object_id())
            .await?;
        transaction.commit().await?;

        std::mem::forget(clean_up);

        Ok(store)
    }

    /// fxdax: snapshots the volume `source_name`, which must be unencrypted, into the new volume
    /// `snapshot_name`, which holds what the source held when it was flushed here, and is a volume
    /// of its own from then on.
    ///
    /// The snapshot shares the source's persistent layers, by reference count, and its extents,
    /// which the allocator freezes (see `Allocator::snapshot`), so it takes time and space
    /// independent of the source's size. Writes to the source that are not flushed when this
    /// flushes it, such as a file's dirty pages, are not in the snapshot.
    pub async fn snapshot_volume(
        &self,
        source_name: &str,
        snapshot_name: &str,
    ) -> Result<Arc<ObjectStore>, Error> {
        let filesystem = &self.filesystem;
        let root_store = filesystem.root_store();
        let source_object_id = match self.volume_directory().lookup(source_name).await? {
            Some((object_id, ObjectDescriptor::Volume, _)) => object_id,
            Some(_) => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            None => bail!(FxfsError::NotFound),
        };
        let source = filesystem
            .object_manager()
            .store(source_object_id)
            .ok_or(FxfsError::NotFound)
            .context("Missing volume store")?;

        // Flush the source, and keep its layers as they are until the snapshot shares them.
        let _flush_guard = filesystem
            .lock_manager()
            .write_lock(lock_keys![LockKey::flush(source_object_id)])
            .await;
        source.flush_guarded_with_reason(FlushReason::Journal(ForceMajor::False)).await?;
        let info = source.snapshot_store_info()?;

        let root_store_object_id = root_store.store_object_id();
        let mut keys =
            vec![LockKey::object(root_store_object_id, self.volume_directory().object_id())];
        keys.extend(info.layers.iter().map(|layer| LockKey::object(root_store_object_id, *layer)));
        // Declared first, as the transaction writes it.
        let handle;
        let mut transaction =
            root_store.new_transaction(LockKeys::Vec(keys), Options::default()).await?;
        ensure!(
            matches!(self.volume_directory().lookup(snapshot_name).await?, None),
            FxfsError::AlreadyExists
        );

        // The snapshot's store, whose information is the source's, sharing its layers.
        handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await?;
        let snapshot_object_id = handle.object_id();
        for layer in &info.layers {
            root_store.adjust_refs(&mut transaction, *layer, 1).await?;
        }
        let mut serialized_info = Vec::new();
        info.serialize_with_version(&mut serialized_info)?;
        let mut buf = handle.allocate_buffer(serialized_info.len()).await;
        buf.copy_from_slice(&serialized_info);
        handle.txn_write(&mut transaction, 0u64, buf.as_ref()).await?;

        // From here on, the source allocates under an owner of its own, never used before.
        let new_owner_object_id = root_store.get_next_object_id().await?.release().get();
        filesystem.allocator().snapshot(
            &mut transaction,
            source_object_id,
            snapshot_object_id,
            new_owner_object_id,
        );
        self.volume_directory()
            .add_child_volume(&mut transaction, snapshot_name, snapshot_object_id)
            .await?;
        transaction.commit().await?;

        let store =
            ObjectStore::open(&root_store, snapshot_object_id, Some(Box::new(self.tree_cache())))
                .await?;
        store.set_trace(filesystem.trace());
        filesystem.object_manager().add_store(store.clone());
        Ok(store)
    }

    /// Returns the volume with the given name.  This is not thread-safe.
    pub async fn volume(
        &self,
        volume_name: &str,
        options: StoreOptions,
    ) -> Result<Arc<ObjectStore>, Error> {
        // Lookup the volume object in the volume directory.
        let (store_object_id, descriptor, _) = self
            .volume_directory()
            .lookup(volume_name)
            .await
            .context("Volume lookup failed")?
            .ok_or(FxfsError::NotFound)
            .context("Volume missing in volume directory")?;
        match descriptor {
            ObjectDescriptor::Volume => (),
            _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
        }
        // Lookup the object store corresponding to the volume.
        let store = self
            .filesystem
            .object_manager()
            .store(store_object_id)
            .ok_or(FxfsError::NotFound)
            .context("Missing volume store")?;
        store.set_trace(self.filesystem.trace());
        // Unlock the volume if required.
        if let Some(crypt) = options.crypt {
            let read_only = self.filesystem.options().read_only;
            store.unlock_inner(crypt, read_only).await.context("Failed to unlock volume")?;
        } else if store.is_locked() {
            bail!(FxfsError::AccessDenied);
        }
        // If the layer files are out of date, kick off a full compaction in the background.
        if let Some(scope_guard) = self.filesystem.scope().active_guard()
            && !self.filesystem.options().read_only
            && store.tree().get_earliest_version() != LATEST_VERSION
        {
            let keys = lock_keys![LockKey::flush(store_object_id)];
            let guard = self
                .filesystem
                .lock_manager()
                .write_lock(keys)
                .await
                .into_owned(self.filesystem.clone());
            let store = store.clone();
            self.filesystem.scope().spawn(async move {
                // This needs to be taken outside the task and passed in. The volume cannot be
                // re-locked until the guard is dropped so this prevents a race where the flush
                // starts after locking by taking it synchronously.
                let _guard = guard;
                // Takes a scope guard so that unencrypted stores will finish their flush before
                // allowing the filesystem to close as well. Journal flushes are normally run on
                // the filesystem's scope, and just like that case it is safe since encrypted
                // volumes can't be locked without taking the flush lock so it will be forced to
                // wait for this completion.
                let _scope_guard = scope_guard;
                if let Err(error) =
                    store.flush_guarded_with_reason(FlushReason::UpgradeVersion).await
                {
                    error!(error:?; "Failed background flush");
                }
            });
        }
        Ok(store)
    }

    /// Deletes the given volume.  Consumes `transaction` and runs `callback` during commit. The
    /// caller must have the correct locks for the volumes directory.
    pub async fn delete_volume(
        &self,
        volume_name: &str,
        mut transaction: Transaction<'_>,
        callback: impl FnOnce() + Send,
    ) -> Result<(), Error> {
        let objects_to_delete = self.delete_volume_impl(volume_name, &mut transaction).await?;
        transaction.commit_with_callback(|_| callback()).await.context("commit")?;
        // Tombstone the deleted objects.
        let root_store = self.filesystem.root_store();
        for object_id in &objects_to_delete {
            root_store.tombstone_object(*object_id, Options::default(), None).await?;
        }
        Ok(())
    }

    async fn delete_volume_impl(
        &self,
        volume_name: &str,
        transaction: &mut Transaction<'_>,
    ) -> Result<Vec<u64>, Error> {
        let object_id =
            match self.volume_directory().lookup(volume_name).await?.ok_or(FxfsError::NotFound)? {
                (object_id, ObjectDescriptor::Volume, _) => object_id,
                _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            };
        let root_store = self.filesystem.root_store();

        // Delete all the layers and encrypted mutations stored in root_store for this volume.
        // This includes the StoreInfo itself. fxdax: layers are deleted only once no other volume
        // shares them (see `snapshot_volume`).
        let mut parent_objects = load_store_info(&root_store, object_id).await?.parent_objects();
        parent_objects.push(object_id);

        let mut objects_to_delete = Vec::new();
        for object_id in parent_objects {
            if root_store.adjust_refs(transaction, object_id, -1).await? {
                objects_to_delete.push(object_id);
            }
        }
        // Mark all volume data as deleted.
        self.filesystem.allocator().mark_for_deletion(transaction, object_id);
        // Remove the volume entry from the VolumeDirectory.
        self.volume_directory().delete_child_volume(transaction, volume_name, object_id)?;
        Ok(objects_to_delete)
    }

    /// Acquires a transaction with appropriate locks to remove volume |name|.
    /// Also returns the object ID of the store which will be deleted.
    pub async fn acquire_transaction_for_remove_volume(
        &self,
        name: &str,
        extra_keys: impl IntoIterator<Item = LockKey>,
        allow_not_found: bool,
    ) -> Result<(u64, Transaction<'_>), Error> {
        // Since we don't know the store object ID until we've looked it up in the volumes
        // directory, we need to loop until we have acquired a lock on a store whose ID is the same
        // as it was in the last iteration.
        let volume_dir = self.volume_directory();
        let store = volume_dir.store();
        let extra_keys = extra_keys.into_iter();
        let mut lock_keys = Vec::with_capacity(extra_keys.size_hint().1.unwrap_or(2) + 2);
        lock_keys.extend(extra_keys);
        lock_keys.push(LockKey::object(store.store_object_id(), volume_dir.object_id()));
        let orig_len = lock_keys.len();
        let mut transaction = None;
        loop {
            lock_keys.truncate(orig_len);
            let object_id = match volume_dir.lookup(name).await? {
                Some((object_id, ObjectDescriptor::Volume, _)) => {
                    // We have to ensure that the store isn't flushed while we delete it, because
                    // deleting the store will remove references to it from ObjectManager which are
                    // then updated by flushing.
                    lock_keys.push(LockKey::flush(object_id));
                    // fxdax: and its layers are locked, as other volumes (snapshots) may share
                    // them, and release them too.
                    let root_store = self.filesystem.root_store();
                    for layer in load_store_info(&root_store, object_id).await?.parent_objects() {
                        lock_keys.push(LockKey::object(root_store.store_object_id(), layer));
                    }
                    object_id
                }
                None => {
                    if allow_not_found {
                        INVALID_OBJECT_ID
                    } else {
                        bail!(FxfsError::NotFound);
                    }
                }
                _ => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            };

            // If the IDs match, return the transaction now.
            match transaction {
                Some(result @ (id, _)) if id == object_id => return Ok(result),
                _ => {}
            }

            transaction = Some((
                object_id,
                store
                    .new_transaction(
                        LockKeys::Vec(lock_keys.clone()),
                        Options { borrow_metadata_space: true, ..Default::default() },
                    )
                    .await?,
            ));
        }
    }
}

/// Returns the root volume for the filesystem.
pub async fn root_volume(filesystem: Arc<FxFilesystem>) -> Result<RootVolume, Error> {
    let root_store = filesystem.root_store();
    let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
        .await
        .context("Unable to open root volume directory")?;
    Ok(RootVolume { _root_directory: root_directory, filesystem })
}

/// Returns the object IDs for all volumes.
pub async fn list_volumes(volume_directory: &Directory<ObjectStore>) -> Result<Vec<u64>, Error> {
    let layer_set = volume_directory.store().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = volume_directory.iter(&mut merger).await?;
    let mut object_ids = vec![];
    while let Some((_, id, _)) = iter.get() {
        object_ids.push(id);
        iter.advance().await?;
    }
    Ok(object_ids)
}

#[cfg(test)]
mod tests {
    use super::root_volume;
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
    use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
    use crate::lsm_tree::types::LayerWriter as _;
    use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use crate::object_store::directory::Directory;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{
        DataObjectHandle, DirectWriter, HandleOptions, LockKey, NewChildStoreOptions, ObjectKey,
        ObjectStore, ObjectValue, StoreOptions,
    };
    use crate::serialized_types::{EARLIEST_SUPPORTED_VERSION, LATEST_VERSION};
    use fxfs_crypto::{Crypt, EncryptionKey, KeyPurpose};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;

    async fn do_fsck(
        fs: &Arc<FxFilesystem>,
        volume_name: Option<&str>,
        crypt: Option<Arc<dyn Crypt>>,
    ) {
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            on_error: Box::new(|err| eprintln!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck filesystem");
        if let Some(volume_name) = volume_name {
            let root = root_volume(fs.clone()).await.unwrap();
            let vol = root
                .volume(
                    volume_name,
                    StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
                )
                .await
                .expect("could not open volume");
            fsck_volume_with_options(&fs, &fsck_options, vol.store_object_id(), crypt)
                .await
                .expect("fsck volume");
        }
    }

    #[fuchsia::test]
    async fn test_lookup_nonexistent_volume() {
        let device = DeviceHolder::new(FakeDevice::new(8192, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
        root_volume
            .volume(
                "vol",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .err()
            .expect("Volume shouldn't exist");
        filesystem.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_add_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());
        {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            root_directory
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        };
        {
            filesystem.close().await.expect("Close failed");
            let device = filesystem.take_device().await;
            device.reopen(false);
            let filesystem = FxFilesystem::open(device).await.expect("open failed");
            do_fsck(&filesystem, Some("vol"), Some(crypt)).await;
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            // NOTE: The volume should have been unlocked by `do_fsck` so we omit `crypt` here.
            let volume = root_volume
                .volume("vol", StoreOptions { crypt: None, ..StoreOptions::default() })
                .await
                .expect("volume failed");
            let root_directory = Directory::open(&volume, volume.root_directory_object_id())
                .await
                .expect("open failed");
            root_directory.lookup("foo").await.expect("lookup failed").expect("not found");
            filesystem.close().await.expect("Close failed");
        };
    }

    #[fuchsia::test]
    async fn test_delete_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store_object_id;
        let parent_objects;
        // Add volume and a file (some data).
        let store_id = {
            let root_volume = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let store = root_volume
                .new_volume(
                    "vol",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(crypt.clone()),
                            ..StoreOptions::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");
            store_object_id = store.store_object_id();
            let mut transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(store_object_id, store.root_directory_object_id())],
                    Options::default(),
                )
                .await
                .expect("new transaction failed");
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let handle = root_directory
                .create_child_file(&mut transaction, "foo")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            let mut buf = handle.allocate_buffer(8192).await;
            buf.fill(0xaa);
            handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
            store.flush().await.expect("flush failed");
            filesystem.sync(SyncOptions::default()).await.expect("sync failed");
            parent_objects = store.parent_objects();
            // Confirm parent objects exist.
            for object_id in &parent_objects {
                let _ = filesystem
                    .root_store()
                    .get_file_size(*object_id)
                    .await
                    .expect("Layer file missing? Bug in test.");
            }
            store.store_object_id()
        };
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("vol"), Some(crypt.clone())).await;
        {
            // Expect 8kiB accounted to the new volume.
            assert_eq!(
                filesystem.allocator().get_owner_allocated_bytes().get(&store_object_id),
                Some(&8192)
            );
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let transaction = filesystem
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(
                            root.volume_directory().store().store_object_id(),
                            root.volume_directory().object_id(),
                        ),
                        LockKey::flush(store_id)
                    ],
                    Options { borrow_metadata_space: true, ..Default::default() },
                )
                .await
                .expect("new_transaction failed");
            root.delete_volume("vol", transaction, || {}).await.expect("delete_volume");
            // Confirm data allocation is gone.
            assert_eq!(
                filesystem
                    .allocator()
                    .get_owner_allocated_bytes()
                    .get(&store_object_id)
                    .unwrap_or(&0),
                &0,
            );
            // Confirm volume entry is gone.
            root.volume(
                "vol",
                StoreOptions { crypt: Some(crypt.clone()), ..StoreOptions::default() },
            )
            .await
            .err()
            .expect("volume shouldn't exist anymore.");
        }
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        // All artifacts of the original volume should be gone.
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, None, None).await;
        for object_id in &parent_objects {
            let _ = filesystem
                .root_store()
                .get_file_size(*object_id)
                .await
                .err()
                .expect("File wasn't deleted.");
        }
        filesystem.close().await.expect("Close failed");
    }

    /// Opens the file `name` in the root directory of `store`.
    async fn open_file(store: &Arc<ObjectStore>, name: &str) -> DataObjectHandle<ObjectStore> {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
        let (object_id, _, _) =
            root_directory.lookup(name).await.expect("lookup failed").expect("not found");
        ObjectStore::open_object(store, object_id, HandleOptions::default(), None)
            .await
            .expect("open_object failed")
    }

    /// The contents of the file `name` in the root directory of `store`.
    async fn read_file(store: &Arc<ObjectStore>, name: &str) -> Vec<u8> {
        let handle = open_file(store, name).await;
        let size = handle.get_size() as usize;
        let mut buf = handle.allocate_buffer(size).await;
        assert_eq!(handle.read_aligned(0, buf.as_mut()).await.expect("read failed"), size);
        buf.as_ref().to_vec()
    }

    /// Writes `contents` at the start of the file `name` in the root directory of `store`,
    /// creating it if it doesn't exist.
    async fn write_file(
        fs: &Arc<FxFilesystem>,
        store: &Arc<ObjectStore>,
        name: &str,
        contents: &[u8],
    ) {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
        let handle = match root_directory.lookup(name).await.expect("lookup failed") {
            Some((object_id, _, _)) => {
                ObjectStore::open_object(store, object_id, HandleOptions::default(), None)
                    .await
                    .expect("open_object failed")
            }
            None => {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            store.root_directory_object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new transaction failed");
                let handle = root_directory
                    .create_child_file(&mut transaction, name)
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
                handle
            }
        };
        let mut buf = handle.allocate_buffer(contents.len()).await;
        buf.copy_from_slice(contents);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
    }

    /// fxdax: a snapshot holds what its source held, and each changes on its own after it.
    #[fuchsia::test]
    async fn test_snapshot_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root
            .new_volume("source", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        write_file(&filesystem, &source, "file", &[0xaa; 8192]).await;
        write_file(&filesystem, &source, "kept", &[0x11; 4096]).await;
        let snapshot =
            root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
        assert_ne!(snapshot.store_object_id(), source.store_object_id());
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);

        // Writes to either don't show in the other.
        write_file(&filesystem, &source, "file", &[0xbb; 8192]).await;
        write_file(&filesystem, &snapshot, "kept", &[0x22; 4096]).await;
        assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
        assert_eq!(read_file(&source, "kept").await, vec![0x11; 4096]);
        assert_eq!(read_file(&snapshot, "kept").await, vec![0x22; 4096]);
        assert!(
            root.snapshot_volume("source", "snapshot").await.is_err(),
            "snapshot over a volume"
        );
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        drop((root, source, snapshot));

        // So after a remount.
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root.volume("source", StoreOptions::default()).await.expect("volume failed");
        let snapshot =
            root.volume("snapshot", StoreOptions::default()).await.expect("volume failed");
        assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
        assert_eq!(read_file(&source, "kept").await, vec![0x11; 4096]);
        assert_eq!(read_file(&snapshot, "kept").await, vec![0x22; 4096]);
        drop((root, source, snapshot));
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: the pages of a file that a snapshot shares are frozen, until written.
    #[fuchsia::test]
    async fn test_snapshot_frozen_pages() {
        const PAGE: u64 = 4096;
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root
            .new_volume("source", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        write_file(&filesystem, &source, "file", &[0xaa; 4 * PAGE as usize]).await;
        let frozen_pages = async |store: &Arc<ObjectStore>| {
            open_file(store, "file").await.frozen_pages(0..4 * PAGE, PAGE).await.unwrap()
        };
        assert_eq!(frozen_pages(&source).await, vec![]);

        let snapshot =
            root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
        let owner = filesystem.allocator().shared_owners(source.store_object_id())[0];
        let pages = frozen_pages(&snapshot).await;
        assert_eq!(pages.len(), 4);
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.offset, i as u64 * PAGE);
            assert_eq!(page.extent_offset, pages[0].extent_offset + i as u64 * PAGE);
            assert_eq!(page.owner_object_id, owner);
        }
        assert_eq!(frozen_pages(&source).await, pages);

        // A page written is the source's own.
        write_file(&filesystem, &source, "file", &[0xbb; PAGE as usize]).await;
        assert_eq!(frozen_pages(&source).await, pages[1..]);
        assert_eq!(frozen_pages(&snapshot).await, pages);

        let file = open_file(&source, "file").await;
        assert_eq!(file.frozen_pages(PAGE..3 * PAGE, PAGE).await.unwrap(), pages[1..3]);
        // Pages larger than blocks are frozen if all their blocks are, in one extent.
        assert_eq!(file.frozen_pages(0..4 * PAGE, 2 * PAGE).await.unwrap(), [pages[2]]);
        assert!(file.frozen_pages(1..PAGE, PAGE).await.is_err());
        drop((file, root, source, snapshot));
        filesystem.close().await.expect("Close failed");
    }

    /// Deletes the volume `name`.
    async fn delete_volume(fs: &Arc<FxFilesystem>, name: &str) {
        let root = root_volume(fs.clone()).await.expect("root_volume failed");
        let (_, transaction) = root
            .acquire_transaction_for_remove_volume(name, [], false)
            .await
            .expect("acquire transaction failed");
        root.delete_volume(name, transaction, || {}).await.expect("delete_volume failed");
    }

    /// fxdax: a snapshot replays from the journal.
    #[fuchsia::test]
    async fn test_snapshot_volume_replays() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crashed = {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "file", &[0xaa; 8192]).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            write_file(&filesystem, &source, "file", &[0xbb; 8192]).await;
            write_file(&filesystem, &snapshot, "new", &[0xcc; 4096]).await;
            filesystem
                .sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            filesystem.device().snapshot().expect("device snapshot failed")
        };
        filesystem.close().await.expect("Close failed");

        let filesystem = FxFilesystem::open(crashed).await.expect("open failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source =
                root.volume("source", StoreOptions::default()).await.expect("volume failed");
            let snapshot =
                root.volume("snapshot", StoreOptions::default()).await.expect("volume failed");
            assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
            assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
            assert_eq!(read_file(&snapshot, "new").await, vec![0xcc; 4096]);
            // A new file in the source doesn't take an object id the snapshot's owner has.
            write_file(&filesystem, &source, "after", &[0xdd; 4096]).await;
        }
        do_fsck(&filesystem, Some("source"), None).await;
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: what a snapshot shares is freed once neither it nor its source references it, in
    /// either order of deletion.
    #[test_case(true; "source first")]
    #[test_case(false; "snapshot first")]
    #[fuchsia::test]
    async fn test_snapshot_volume_deletion_frees_space(source_first: bool) {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let allocator = filesystem.allocator();
        let baseline = allocator.get_owner_allocated_bytes();
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "shared", &[0xaa; 65536]).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            write_file(&filesystem, &source, "own", &[0xbb; 16384]).await;
            write_file(&filesystem, &snapshot, "own", &[0xcc; 16384]).await;
            // Overwriting shared data leaves it shared: the other still references it.
            write_file(&filesystem, &source, "shared", &[0xdd; 65536]).await;
            assert_eq!(read_file(&snapshot, "shared").await, vec![0xaa; 65536]);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");

        let (first, second) =
            if source_first { ("source", "snapshot") } else { ("snapshot", "source") };
        delete_volume(&filesystem, first).await;
        do_fsck(&filesystem, Some(second), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let left = root.volume(second, StoreOptions::default()).await.expect("volume failed");
            let shared = read_file(&left, "shared").await;
            assert_eq!(shared, vec![if source_first { 0xaa } else { 0xdd }; 65536]);
        }
        assert_eq!(allocator.frozen_owners().len(), 1, "the shared data has a sharer left");

        delete_volume(&filesystem, second).await;
        do_fsck(&filesystem, None, None).await;
        assert!(allocator.frozen_owners().is_empty());
        assert_eq!(allocator.get_owner_allocated_bytes(), baseline);

        // And on disk.
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, None, None).await;
        assert_eq!(filesystem.allocator().get_owner_allocated_bytes(), baseline);
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: overwriting a preallocated file's blocks that a snapshot shares writes new blocks,
    /// leaving the snapshot's as they were.
    #[fuchsia::test]
    async fn test_snapshot_volume_overwrite_extents() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let block = filesystem.block_size().get() as usize;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "file", &[]).await;
            let root_directory = Directory::open(&source, source.root_directory_object_id())
                .await
                .expect("open failed");
            let (object_id, _, _) =
                root_directory.lookup("file").await.expect("lookup failed").expect("not found");
            let handle =
                ObjectStore::open_object(&source, object_id, HandleOptions::default(), None)
                    .await
                    .expect("open_object failed");
            handle.truncate(10 * block as u64).await.expect("truncate failed");
            handle.allocate(0..10 * block as u64).await.expect("allocate failed");
            let overwrite = |range: std::ops::Range<usize>, byte: u8| {
                let handle = &handle;
                async move {
                    let mut buf = handle.allocate_buffer(range.end - range.start).await;
                    buf.as_mut().fill(byte);
                    let mut transaction = handle.new_transaction().await.expect("transaction");
                    handle
                        .multi_overwrite(
                            &mut transaction,
                            crate::object_store::AttributeId::DATA,
                            &[range.start as u64..range.end as u64],
                            buf.as_mut(),
                        )
                        .await
                        .expect("multi_overwrite failed");
                    transaction.commit().await.expect("commit failed");
                }
            };
            overwrite(0..2 * block, 0xaa).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            // A file open across the snapshot, and an unwritten block.
            overwrite(0..block, 0xbb).await;
            overwrite(5 * block..6 * block, 0xcc).await;

            let mut expected_snapshot = vec![0; 10 * block];
            expected_snapshot[..2 * block].fill(0xaa);
            let mut expected_source = expected_snapshot.clone();
            expected_source[..block].fill(0xbb);
            expected_source[5 * block..6 * block].fill(0xcc);
            assert_eq!(read_file(&snapshot, "file").await, expected_snapshot);
            assert_eq!(read_file(&source, "file").await, expected_source);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        delete_volume(&filesystem, "snapshot").await;
        do_fsck(&filesystem, Some("source"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source =
                root.volume("source", StoreOptions::default()).await.expect("volume failed");
            let contents = read_file(&source, "file").await;
            assert_eq!(&contents[..block], &vec![0xbb; block][..]);
            assert_eq!(&contents[block..2 * block], &vec![0xaa; block][..]);
            assert_eq!(&contents[5 * block..6 * block], &vec![0xcc; block][..]);
        }
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: snapshots of snapshots share what each was made from, and deleting them in any order
    /// frees everything once the last is gone.
    #[fuchsia::test]
    async fn test_snapshot_volume_chain() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let allocator = filesystem.allocator();
        let baseline = allocator.get_owner_allocated_bytes();
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "a", &[1; 16384]).await;
            let first =
                root.snapshot_volume("source", "first").await.ok().expect("snapshot failed");
            write_file(&filesystem, &first, "b", &[2; 16384]).await;
            let second =
                root.snapshot_volume("first", "second").await.ok().expect("snapshot failed");
            write_file(&filesystem, &second, "a", &[3; 16384]).await;
            write_file(&filesystem, &first, "b", &[4; 16384]).await;
            assert_eq!(read_file(&source, "a").await, vec![1; 16384]);
            assert_eq!(read_file(&first, "a").await, vec![1; 16384]);
            assert_eq!(read_file(&first, "b").await, vec![4; 16384]);
            assert_eq!(read_file(&second, "a").await, vec![3; 16384]);
            assert_eq!(read_file(&second, "b").await, vec![2; 16384]);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        for volume in ["source", "first", "second"] {
            do_fsck(&filesystem, Some(volume), None).await;
        }
        delete_volume(&filesystem, "first").await;
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("second"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let second =
                root.volume("second", StoreOptions::default()).await.expect("volume failed");
            assert_eq!(read_file(&second, "b").await, vec![2; 16384]);
        }
        delete_volume(&filesystem, "source").await;
        do_fsck(&filesystem, Some("second"), None).await;
        delete_volume(&filesystem, "second").await;
        do_fsck(&filesystem, None, None).await;
        assert!(allocator.frozen_owners().is_empty());
        assert_eq!(allocator.get_owner_allocated_bytes(), baseline);
        filesystem.close().await.expect("Close failed");
    }
    #[fuchsia::test]
    async fn test_create_volume_with_guid() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let guid = [1u8; 16];
        {
            let root = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root
                .new_volume("vol", NewChildStoreOptions { guid: Some(guid), ..Default::default() })
                .await
                .unwrap();
            assert_eq!(store.store_info().unwrap().guid, guid);
        }
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.unwrap();
        {
            let root = root_volume(fs.clone()).await.unwrap();
            let vol = root.volume("vol", StoreOptions::default()).await.unwrap();
            assert_eq!(vol.guid(), guid);
        }
        fs.close().await.unwrap();
    }

    #[test_case(false; "unencrypted")]
    #[test_case(true; "encrypted")]
    #[fuchsia::test]
    async fn test_flush_on_open_to_update_version(encrypted: bool) {
        // Can't test it if we don't have any versions that we can migrate between.
        if LATEST_VERSION == EARLIEST_SUPPORTED_VERSION {
            return;
        }

        let device = DeviceHolder::new(FakeDevice::new(8192, 1024));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crypt: Option<Arc<dyn Crypt>> =
            if encrypted { Some(Arc::new(new_insecure_crypt())) } else { None };
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() },
                        ..Default::default()
                    },
                )
                .await
                .expect("new_volume failed");

            let parent_store = store.parent_store().unwrap();
            let txn_options = Options {
                skip_journal_checks: true,
                borrow_metadata_space: true,
                allocator_reservation: Some(fs.object_manager().metadata_reservation()),
                ..Default::default()
            };
            let mut transaction = parent_store
                .new_transaction(lock_keys![], txn_options)
                .await
                .expect("new_transaction failed");
            let new_layer_object = if let Some(crypt) = &crypt {
                let raw_id = parent_store.get_next_object_id().await.unwrap();
                let (fxfs_key, unwrapped_key) =
                    crypt.create_key(raw_id.get(), KeyPurpose::Data).await.unwrap();
                ObjectStore::create_object_with_key(
                    &parent_store,
                    &mut transaction,
                    raw_id,
                    HandleOptions { skip_journal_checks: true, ..Default::default() },
                    EncryptionKey::Fxfs(fxfs_key),
                    unwrapped_key,
                )
                .await
                .expect("create_object_with_key failed")
            } else {
                ObjectStore::create_object(
                    &parent_store,
                    &mut transaction,
                    HandleOptions { skip_journal_checks: true, ..Default::default() },
                    None,
                )
                .await
                .expect("create_object failed")
            };
            transaction.commit().await.expect("commit failed");

            // Add an empty layer to the set with the old version.
            {
                let layer_writer = DirectWriter::new(&new_layer_object, txn_options).await;
                let writer = PersistentLayerWriter::<_, ObjectKey, ObjectValue>::new_with_version(
                    layer_writer,
                    0,
                    fs.block_size(),
                    EARLIEST_SUPPORTED_VERSION,
                )
                .await
                .expect("writer failed");
                writer.complete().await.expect("writer complete failed");
            }

            let mut store_info = store.load_store_info().await.unwrap();
            store_info.layers = vec![new_layer_object.object_id()];
            let mut end_transaction = parent_store
                .new_transaction(
                    lock_keys![LockKey::object(
                        parent_store.store_object_id(),
                        store.store_info_handle_object_id().unwrap()
                    )],
                    txn_options,
                )
                .await
                .expect("new_transaction failed");
            store
                .write_store_info(&mut end_transaction, &store_info)
                .await
                .expect("write_store_info failed");
            end_transaction.commit().await.expect("commit failed");
        }
        fs.close().await.expect("close failed");

        // Re-open filesystem.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let _store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() })
                .await
                .expect("volume failed");
        }

        // Closing the filesystem should force the background flush to complete.
        fs.close().await.expect("close failed");

        // Re-open filesystem again to ensure the layer file version has been updated.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..StoreOptions::default() })
                .await
                .expect("volume failed");

            assert_eq!(store.tree.get_earliest_version(), LATEST_VERSION);
        }

        fs.close().await.expect("close failed");
    }
}
