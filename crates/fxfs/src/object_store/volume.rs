// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/object_store/volume.rs`. fxdax snapshots (volume snapshots that
// share their source's layers and extents) deviate from upstream here:
// `RootVolume::snapshot_volume` makes them; deleting a volume releases its layers by reference
// count, and locks them. There is no `install_volume` (nor `replace_volume`, which it used): an image within a file is a block device's,
// which fxdax's extent store has none of (backlog doc-4). `RootVolume::volume` opens a volume left
// closed (`Options::lazy_volumes`). `RootVolume::publish_volume` and `new_volume_over_base` make a
// volume of one another filesystem wrote (backlog doc-10).

use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, ForceMajor, FxFilesystem};
use crate::object_handle::ObjectHandle;
use crate::object_store::directory::Directory;
use crate::object_store::transaction::{LockKeys, Options, Transaction, lock_keys};
use crate::object_store::{
    HandleOptions, INVALID_OBJECT_ID, LockKey, MAX_MANIFEST_SIZE, NewChildStoreOptions,
    ObjectDescriptor, ObjectStore, StoreInfo, StoreOptions, load_store_info,
};
use crate::serialized_types::{LATEST_VERSION, Version, VersionedLatest};
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
            .new_child_store(
                &mut transaction,
                options,
                Some(Box::new(self.filesystem.new_tree_cache())),
            )
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

        let store = ObjectStore::open(
            &root_store,
            snapshot_object_id,
            Some(Box::new(filesystem.new_tree_cache())),
        )
        .await?;
        store.set_trace(filesystem.trace());
        filesystem.object_manager().add_store(store.clone());
        Ok(store)
    }

    /// fxdax: flushes the volume `name`, which must be unencrypted and open, whole, to one layer,
    /// and returns its manifest: what lets a filesystem on a device reading its extents make a
    /// volume over it (`new_volume_over_base`, backlog doc-10), with no replay of this journal.
    /// The device must keep what the manifest names, as of now: a compaction here frees the
    /// layers it names, which a fork of the device holds on to.
    pub async fn publish_volume(&self, name: &str) -> Result<Vec<u8>, Error> {
        let filesystem = &self.filesystem;
        let object_id = match self.volume_directory().lookup(name).await? {
            Some((object_id, ObjectDescriptor::Volume, _)) => object_id,
            Some(_) => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            None => bail!(FxfsError::NotFound),
        };
        let store = filesystem
            .object_manager()
            .store(object_id)
            .ok_or(FxfsError::NotFound)
            .context("Missing volume store")?;
        let _flush_guard =
            filesystem.lock_manager().write_lock(lock_keys![LockKey::flush(object_id)]).await;
        store.flush_guarded_with_reason(FlushReason::Journal(ForceMajor::True)).await?;
        store.manifest().await
    }

    /// fxdax: creates the volume `name`, private to this filesystem, over the volume `manifest`
    /// describes (`publish_volume`), and opens it. It holds what that volume held when published,
    /// which the device must read: its layers and data stay where they are, in extents it doesn't
    /// hold, and what it writes goes in layers above. Takes one transaction, whatever the base's
    /// size, and reads nothing of it until the volume opens.
    pub async fn new_volume_over_base(
        &self,
        name: &str,
        manifest: &[u8],
    ) -> Result<Arc<ObjectStore>, Error> {
        let filesystem = &self.filesystem;
        ensure!(manifest.len() <= MAX_MANIFEST_SIZE, FxfsError::TooBig);
        let mut reader = manifest;
        // fxdax: a manifest that doesn't parse is an argument wrong, as one that names no base.
        let version = Version::deserialize_from(&mut reader).context(FxfsError::InvalidArgs)?;
        ensure!(version <= LATEST_VERSION, FxfsError::InvalidVersion);
        let mut info = StoreInfo::deserialize_from_version(&mut reader, version)
            .context("Failed to deserialize manifest")
            .context(FxfsError::InvalidArgs)?;
        info.check_manifest()?;
        info.set_new_guid();
        filesystem.allocator().check_base_branches(&info.base_branches)?;

        let root_store = filesystem.root_store();
        let directory_key =
            LockKey::object(root_store.store_object_id(), self.volume_directory().object_id());
        // Declared first, as the transaction writes it.
        let handle;
        let mut transaction =
            root_store.new_transaction(lock_keys![directory_key], Options::default()).await?;
        ensure!(
            matches!(self.volume_directory().lookup(name).await?, None),
            FxfsError::AlreadyExists
        );
        handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await?;
        let object_id = handle.object_id();
        let mut serialized_info = Vec::new();
        info.serialize_with_version(&mut serialized_info)?;
        let mut buf = handle.allocate_buffer(serialized_info.len()).await;
        buf.copy_from_slice(&serialized_info);
        handle.txn_write(&mut transaction, 0u64, buf.as_ref()).await?;
        self.volume_directory().add_child_volume(&mut transaction, name, object_id).await?;
        transaction.commit().await?;

        let store =
            ObjectStore::open(&root_store, object_id, Some(Box::new(filesystem.new_tree_cache())))
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
        // Lookup the object store corresponding to the volume. fxdax: which may be closed.
        if self.filesystem.options().lazy_volumes {
            self.filesystem.object_manager().open_volume(store_object_id).await?;
        }
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
            && store.earliest_writable_version() != LATEST_VERSION
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
    use super::{RootVolume, root_volume};
    use crate::errors::FxfsError;
    use crate::filesystem::{
        FlushReason, ForceMajor, FxFilesystem, FxFilesystemBuilder, SyncOptions,
    };
    use crate::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
    use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
    use crate::lsm_tree::types::LayerWriter as _;
    use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use crate::object_store::directory::{Directory, ReplacedChild, replace_child};
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{
        DataObjectHandle, DirectWriter, HandleOptions, LockKey, NewChildStoreOptions, ObjectKey,
        ObjectStore, ObjectValue, StoreInfo, StoreOptions, tree,
    };
    use crate::serialized_types::{
        EARLIEST_SUPPORTED_VERSION, LATEST_VERSION, Version, VersionedLatest,
    };
    use fxfs_crypto::{Crypt, EncryptionKey, KeyPurpose};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::sync::Arc;
    use std::sync::atomic::Ordering::Relaxed;
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

    /// Makes volumes "a" and "b", each with a file, flushed, so that the journal holds nothing of
    /// them, and returns the filesystem, open still.
    async fn two_flushed_volumes() -> crate::filesystem::OpenFxFilesystem {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        for (name, byte) in [("a", 0xaa), ("b", 0xbb)] {
            let store =
                root.new_volume(name, NewChildStoreOptions::default()).await.expect("new_volume");
            write_file(&filesystem, &store, "file", &[byte; 8192]).await;
        }
        filesystem.journal().force_compact().await.expect("compact failed");
        filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        filesystem
    }

    fn lazily(device: DeviceHolder) -> impl Future<Output = crate::filesystem::OpenFxFilesystem> {
        async {
            FxFilesystemBuilder::new().lazy_volumes(true).open(device).await.expect("open failed")
        }
    }

    /// The store of the volume `name`, if it is open.
    async fn open_store(filesystem: &FxFilesystem, root: &RootVolume, name: &str) -> bool {
        let (object_id, _, _) = root.volume_directory().lookup(name).await.unwrap().unwrap();
        filesystem.object_manager().store(object_id).is_some()
    }

    /// fxdax: volumes open as they are asked for, and only then, with what they require of the
    /// metadata reservation paid for.
    #[fuchsia::test]
    async fn test_lazy_volumes() {
        let filesystem = two_flushed_volumes().await;
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = lazily(device).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        assert!(!open_store(&filesystem, &root, "a").await);
        assert!(!open_store(&filesystem, &root, "b").await);

        let a = root.volume("a", StoreOptions::default()).await.expect("volume failed");
        assert!(open_store(&filesystem, &root, "a").await);
        assert!(!open_store(&filesystem, &root, "b").await);
        assert_eq!(read_file(&a, "file").await, vec![0xaa; 8192]);
        // A second ask finds it.
        let again = root.volume("a", StoreOptions::default()).await.expect("volume failed");
        assert!(Arc::ptr_eq(&a, &again));

        let objects = filesystem.object_manager();
        assert_eq!(
            objects.metadata_reservation().amount() + objects.borrowed_metadata_space(),
            objects.required_reservation()
        );
        write_file(&filesystem, &a, "more", &[0xcc; 4096]).await;
        std::mem::drop((a, again, root));
        filesystem.close().await.expect("Close failed");

        // Opened eagerly, as fsck needs, the stores are as they should be.
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("a"), None).await;
        do_fsck(&filesystem, Some("b"), None).await;
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: replay opens a volume the journal holds something of, and leaves the others closed.
    #[fuchsia::test]
    async fn test_lazy_volumes_replay() {
        let filesystem = two_flushed_volumes().await;
        let crashed = {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
            write_file(&filesystem, &b, "in-the-journal", &[0xcc; 4096]).await;
            filesystem
                .sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            filesystem.device().snapshot().expect("device snapshot failed")
        };
        filesystem.close().await.expect("Close failed");

        let filesystem = lazily(crashed).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        assert!(!open_store(&filesystem, &root, "a").await);
        assert!(open_store(&filesystem, &root, "b").await);
        let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
        assert_eq!(read_file(&b, "in-the-journal").await, vec![0xcc; 4096]);
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: a volume closed through a mount is reaped as it opens: what a crash left in its
    /// graveyard goes then.
    #[fuchsia::test]
    async fn test_lazy_volumes_reap_as_they_open() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store =
            root.new_volume("a", NewChildStoreOptions::default()).await.expect("new_volume");
        let mut transaction = filesystem
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new transaction failed");
        let handle =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        let object_id = handle.object_id();
        store.add_to_graveyard(&mut transaction, object_id);
        transaction.commit().await.expect("commit failed");
        drop(handle);
        filesystem.journal().force_compact().await.expect("compact failed");
        filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        let crashed = filesystem.device().snapshot().expect("device snapshot failed");
        filesystem.close().await.expect("Close failed");

        let filesystem = lazily(crashed).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        assert!(!open_store(&filesystem, &root, "a").await);
        let store = root.volume("a", StoreOptions::default()).await.expect("volume failed");
        filesystem.graveyard().flush().await;
        let found = store.tree().find(&ObjectKey::object(object_id)).await.expect("find failed");
        assert_eq!(found.map(|item| item.value), None);
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

    // fxdax: volumes over a base (backlog doc-10). The branches of the devices these tests use:
    // those of the base, the fork of it that keeps a generation, and of what reads through to it.
    const BASE: u64 = 10;
    const GENERATION: u64 = 11;
    const SANDBOX: u64 = 20;

    const ORIGINAL: [(&str, u8); 3] = [("a", 0xaa), ("b", 0xbb), ("c", 0xcc)];

    /// A filesystem on a device of branch `BASE` holding the volume "repo", published, with the
    /// files `ORIGINAL` and `more` others, and the volume's manifest. The filesystem stays open.
    async fn published(more: usize) -> (crate::filesystem::OpenFxFilesystem, Vec<u8>) {
        let device = DeviceHolder::new(FakeDevice::new(65536, 512).with_branch(BASE));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store =
            root.new_volume("repo", NewChildStoreOptions::default()).await.expect("new_volume");
        for (name, byte) in ORIGINAL {
            write_file(&filesystem, &store, name, &[byte; 8192]).await;
        }
        for i in 0..more {
            write_file(&filesystem, &store, &format!("more-{i}"), &[1; 4096]).await;
        }
        let manifest = root.publish_volume("repo").await.expect("publish_volume failed");
        filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        (filesystem, manifest)
    }

    /// A new filesystem on a device of branch `SANDBOX` reading through to a fork of `base`, and
    /// what counts the reads that reach the fork.
    async fn over(
        base: &FxFilesystem,
    ) -> (crate::filesystem::OpenFxFilesystem, Arc<std::sync::atomic::AtomicU64>) {
        let mut device = FakeDevice::new(16384, 512).with_branch(SANDBOX);
        let reads = device.attach(base.device().fork(GENERATION).expect("fork failed"));
        let filesystem = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new");
        (filesystem, reads)
    }

    async fn check_originals(store: &Arc<ObjectStore>) {
        for (name, byte) in ORIGINAL {
            assert_eq!(read_file(store, name).await, vec![byte; 8192], "{name}");
        }
    }

    /// Removes the file `name` from the root directory of `store`, and its object.
    async fn remove_file(store: &Arc<ObjectStore>, name: &str) {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
        let mut context = root_directory
            .acquire_context_for_replace(None, name, true)
            .await
            .expect("acquire_context_for_replace failed");
        let replaced = replace_child(&mut context.transaction, None, (&root_directory, name))
            .await
            .expect("replace_child failed");
        let ReplacedChild::Object(object_id) = replaced else {
            panic!("expected a file, found {replaced:?}");
        };
        context.transaction.commit().await.expect("commit failed");
        store
            .tombstone_object(object_id, Options::default(), context.truncate_guard.as_ref())
            .await
            .expect("tombstone_object failed");
    }

    async fn rename_file(store: &Arc<ObjectStore>, from: &str, to: &str) {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
        let mut context = root_directory
            .acquire_context_for_replace(Some((&root_directory, from)), to, true)
            .await
            .expect("acquire_context_for_replace failed");
        replace_child(
            &mut context.transaction,
            Some((&root_directory, from)),
            (&root_directory, to),
        )
        .await
        .expect("replace_child failed");
        context.transaction.commit().await.expect("commit failed");
    }

    async fn exists(store: &Arc<ObjectStore>, name: &str) -> bool {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
        root_directory.lookup(name).await.expect("lookup failed").is_some()
    }

    /// A volume over a base reads as it did, without the base's journal. Opening it reads a layer's
    /// ends and bloom filter, and nothing of what it holds.
    #[test_case(0, 3)]
    #[test_case(500, 4)]
    #[fuchsia::test]
    async fn test_base_reads_as_it_was(more: usize, opening_reads: u64) {
        let (base, manifest) = published(more).await;
        // What the base does next is not in the generation.
        let base_root = root_volume(base.clone()).await.expect("root_volume failed");
        let base_store = base_root.volume("repo", StoreOptions::default()).await.expect("volume");
        let (filesystem, reads) = over(&base).await;
        write_file(&base, &base_store, "later", &[0xdd; 4096]).await;

        assert_eq!(reads.load(Relaxed), 0);
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        assert_eq!(reads.load(Relaxed), opening_reads);

        check_originals(&store).await;
        for i in 0..more {
            assert_eq!(read_file(&store, &format!("more-{i}")).await, [1; 4096]);
        }
        assert!(!exists(&store, "later").await);
        // The allocator holds none of it.
        assert_eq!(
            filesystem.allocator().data_owner(store.store_object_id()),
            store.store_object_id()
        );
        assert!(filesystem.allocator().branches_of(&[store.store_object_id()]).is_empty());
        std::mem::drop((store, root, base_store, base_root));
        do_fsck(&filesystem, Some("repo"), None).await;
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// What a volume over a base writes, renames and deletes is its own, whatever compacts it, and
    /// frees nothing of the base (the fake device fails a free of what it reads through to).
    #[fuchsia::test]
    async fn test_base_changes_stay_private() {
        let (base, manifest) = published(0).await;
        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");

        write_file(&filesystem, &store, "new", &[0x11; 4096]).await;
        write_file(&filesystem, &store, "a", &[0x22; 4096]).await;
        rename_file(&store, "b", "renamed").await;
        remove_file(&store, "c").await;

        let check = async |store: &Arc<ObjectStore>| {
            assert_eq!(read_file(store, "new").await, [0x11; 4096]);
            let mut a = vec![0x22; 4096];
            a.resize(8192, 0xaa);
            assert_eq!(read_file(store, "a").await, a);
            assert_eq!(read_file(store, "renamed").await, [0xbb; 8192]);
            assert!(!exists(store, "b").await);
            assert!(!exists(store, "c").await);
        };
        check(&store).await;

        let base_layers = store.store_info().unwrap().base_layers;
        assert_eq!(base_layers.len(), 1);
        for reason in [
            FlushReason::Journal(ForceMajor::False),
            FlushReason::Journal(ForceMajor::True),
            FlushReason::Journal(ForceMajor::True),
        ] {
            store.flush_with_reason(reason).await.expect("flush failed");
            filesystem.graveyard().flush().await;
            check(&store).await;
            let info = store.store_info().unwrap();
            assert_eq!(info.base_layers, base_layers);
            assert_eq!(info.layers.len(), 1);
        }

        // The filesystem, and the base, are as they should be, and a second volume over the base
        // sees none of this.
        let other = root.new_volume_over_base("other", &manifest).await.expect("over_base failed");
        check_originals(&other).await;
        std::mem::drop((store, other, root));
        do_fsck(&filesystem, Some("repo"), None).await;
        do_fsck(&filesystem, Some("other"), None).await;
        filesystem.close().await.expect("Close failed");
        do_fsck(&base, Some("repo"), None).await;
        base.close().await.expect("Close failed");
    }

    /// No reservation pays for a base's layers, which no compaction rewrites: however much a base
    /// holds, a volume over it reserves what its own layers ask.
    #[fuchsia::test]
    async fn test_base_layers_are_not_reserved() {
        let (base, manifest) = published(500).await;
        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        let objects = filesystem.object_manager();
        assert_eq!(objects.reservation(store.store_object_id()), Some(0));

        write_file(&filesystem, &store, "new", &[0x11; 4096]).await;
        store.flush().await.expect("flush failed");
        let info = store.store_info().unwrap();
        let root_store = filesystem.root_store();
        let size = root_store.get_file_size(info.layers[0]).await.expect("get_file_size failed");
        assert_eq!(
            objects.reservation(store.store_object_id()),
            Some(tree::reservation_amount_from_layer_size(size))
        );
        assert_eq!(
            objects.metadata_reservation().amount() + objects.borrowed_metadata_space(),
            objects.required_reservation()
        );
        std::mem::drop((store, root));
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// Reflinks of a base's extents, and snapshots of a volume over a base, share them as
    /// reflinks and snapshots do, and whatever deletes them leaves the base be.
    #[fuchsia::test]
    async fn test_base_reflinks_and_snapshots() {
        let (base, manifest) = published(0).await;
        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");

        write_file(&filesystem, &store, "copy", &[]).await;
        let source = open_file(&store, "a").await;
        let copy = open_file(&store, "copy").await;
        let keys = lock_keys![
            LockKey::object(store.store_object_id(), source.object_id()),
            LockKey::object(store.store_object_id(), copy.object_id()),
        ];
        let mut transaction =
            store.new_transaction(keys, Options::default()).await.expect("new_transaction failed");
        source
            .txn_clone_range(&mut transaction, 0..8192, &copy, 0)
            .await
            .expect("txn_clone_range failed");
        copy.txn_update_size(&mut transaction, 8192, None).await.expect("size failed");
        transaction.commit().await.expect("commit failed");
        std::mem::drop((source, copy));
        assert_eq!(read_file(&store, "copy").await, [0xaa; 8192]);
        remove_file(&store, "a").await;
        assert_eq!(read_file(&store, "copy").await, [0xaa; 8192]);

        let snapshot = root.snapshot_volume("repo", "snapshot").await.expect("snapshot failed");
        remove_file(&store, "copy").await;
        assert_eq!(read_file(&snapshot, "copy").await, [0xaa; 8192]);
        assert_eq!(read_file(&snapshot, "b").await, [0xbb; 8192]);
        filesystem.graveyard().flush().await;
        std::mem::drop((store, snapshot));
        do_fsck(&filesystem, Some("repo"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;

        // A volume over a base deletes as any does.
        let (_, transaction) = root
            .acquire_transaction_for_remove_volume("repo", [], false)
            .await
            .expect("acquire_transaction_for_remove_volume failed");
        root.delete_volume("repo", transaction, || {}).await.expect("delete_volume failed");
        std::mem::drop(root);
        do_fsck(&filesystem, Some("snapshot"), None).await;
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// A volume over a base survives a crash, with what it wrote since replayed over its base,
    /// and a clean mount leaves it closed, reading nothing of the base, until it is asked for.
    #[fuchsia::test]
    async fn test_base_survives_crashes_and_stays_closed() {
        let (base, manifest) = published(0).await;
        let (filesystem, reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        write_file(&filesystem, &store, "in-the-journal", &[0x11; 4096]).await;
        remove_file(&store, "a").await;
        filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        let crashed = filesystem.device().snapshot().expect("device snapshot failed");
        std::mem::drop((store, root));
        filesystem.close().await.expect("Close failed");

        let filesystem = FxFilesystem::open(crashed).await.expect("open failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.volume("repo", StoreOptions::default()).await.expect("volume failed");
        assert_eq!(read_file(&store, "in-the-journal").await, [0x11; 4096]);
        assert!(!exists(&store, "a").await);
        assert_eq!(read_file(&store, "b").await, [0xbb; 8192]);
        filesystem.journal().force_compact().await.expect("compact failed");
        std::mem::drop((store, root));
        do_fsck(&filesystem, Some("repo"), None).await;
        filesystem.close().await.expect("Close failed");

        let device = filesystem.take_device().await;
        device.reopen(false);
        reads.store(0, Relaxed);
        let filesystem = lazily(device).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        assert!(!open_store(&filesystem, &root, "repo").await);
        assert_eq!(reads.load(Relaxed), 0);
        let store = root.volume("repo", StoreOptions::default()).await.expect("volume failed");
        assert_ne!(reads.load(Relaxed), 0);
        assert_eq!(read_file(&store, "c").await, [0xcc; 8192]);
        std::mem::drop((store, root));
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// A volume over a base publishes as any does, its base layers below the ones it wrote.
    #[fuchsia::test]
    async fn test_base_over_a_base() {
        let (base, manifest) = published(0).await;
        let (middle, _reads) = over(&base).await;
        let root = root_volume(middle.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        write_file(&middle, &store, "new", &[0x11; 4096]).await;
        remove_file(&store, "a").await;
        let upper = root.publish_volume("repo").await.expect("publish_volume failed");
        middle.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();

        let mut device = FakeDevice::new(16384, 512).with_branch(30);
        device.attach(middle.device().fork(SANDBOX + 1).expect("fork failed"));
        let filesystem = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let top = root.new_volume_over_base("repo", &upper).await.expect("over_base failed");
        assert_eq!(top.store_info().unwrap().base_layers.len(), 2);
        assert_eq!(top.store_info().unwrap().base_branches, [BASE, SANDBOX]);
        assert_eq!(read_file(&top, "new").await, [0x11; 4096]);
        assert!(!exists(&top, "a").await);
        assert_eq!(read_file(&top, "b").await, [0xbb; 8192]);
        std::mem::drop((top, root));
        do_fsck(&filesystem, Some("repo"), None).await;
        filesystem.close().await.expect("Close failed");
        middle.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// A base's layers of an older version are no reason for a volume over it to compact: none
    /// would rewrite them.
    #[fuchsia::test]
    async fn test_base_layers_of_older_versions_stay() {
        const OLDER: Version = Version { major: LATEST_VERSION.major - 1, minor: 0 };
        let (base, _manifest) = published(0).await;
        let root = root_volume(base.clone()).await.expect("root_volume failed");
        let store = root.volume("repo", StoreOptions::default()).await.expect("volume failed");

        // An empty layer of an older version at the bottom of the volume's tree.
        let root_store = base.root_store();
        let options = Options {
            skip_journal_checks: true,
            borrow_metadata_space: true,
            allocator_reservation: Some(base.object_manager().metadata_reservation()),
            ..Default::default()
        };
        let mut transaction =
            root_store.new_transaction(lock_keys![], options).await.expect("new failed");
        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions { skip_journal_checks: true, ..Default::default() },
            None,
        )
        .await
        .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
        PersistentLayerWriter::<_, ObjectKey, ObjectValue>::new_with_version(
            DirectWriter::new(&handle, options).await,
            0,
            base.block_size(),
            OLDER,
        )
        .await
        .expect("writer failed")
        .complete()
        .await
        .expect("complete failed");
        store.store_info.lock().as_mut().unwrap().layers.push(handle.object_id());
        let manifest = store.manifest().await.expect("manifest failed");
        base.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();

        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let over = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        assert_eq!(over.tree.get_earliest_version(), OLDER);
        assert_eq!(over.earliest_writable_version(), LATEST_VERSION);
        let flushes = over.counters.lock().num_flushes;
        over.flush_with_reason(FlushReason::UpgradeVersion).await.expect("flush failed");
        assert_eq!(over.counters.lock().num_flushes, flushes);
        std::mem::drop((over, root));
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// A follower taking the layers its writer's flush wrote keeps the store's base layers below
    /// them: followers open every volume, so that a writer flushing one over a base gets here.
    #[fuchsia::test]
    async fn test_base_layers_stay_as_followers_take_flushed_layers() {
        let (base, manifest) = published(0).await;
        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.new_volume_over_base("repo", &manifest).await.expect("over_base failed");
        write_file(&filesystem, &store, "new", &[0x11; 4096]).await;
        store.flush().await.expect("flush failed");
        let layers = store.tree.immutable_layer_set().layers.len();

        store.take_flushed_layers().await.expect("take_flushed_layers failed");
        assert_eq!(store.tree.immutable_layer_set().layers.len(), layers);
        assert_eq!(store.base_layer_count(), 1);
        assert_eq!(read_file(&store, "new").await, [0x11; 4096]);
        check_originals(&store).await;
        std::mem::drop((store, root));
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }

    /// Manifests that can't be honoured leave no volume.
    /// A manifest says only what layers hold, so none is made while the journal holds more.
    #[fuchsia::test]
    async fn test_no_manifest_of_what_the_journal_holds() {
        let (filesystem, _) = published(0).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let store = root.volume("repo", StoreOptions::default()).await.expect("volume failed");
        store.manifest().await.expect("manifest of a flushed volume failed");

        write_file(&filesystem, &store, "later", &[1; 4096]).await;
        let error = store.manifest().await.expect_err("manifest succeeded");
        assert_eq!(error.downcast::<FxfsError>().expect("not fxfs"), FxfsError::Unavailable);

        store.flush().await.expect("flush failed");
        store.manifest().await.expect("manifest of a flushed volume failed");
        std::mem::drop(store);
        std::mem::drop(root);
        filesystem.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_base_manifests_refused() {
        let (base, manifest) = published(0).await;
        let error = |result: Result<Arc<ObjectStore>, anyhow::Error>| {
            result.err().expect("over_base succeeded").downcast::<FxfsError>().expect("not fxfs")
        };

        // Of the device's own branch.
        let device = DeviceHolder::new(FakeDevice::new(16384, 512).with_branch(BASE));
        let own = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(own.clone()).await.expect("root_volume failed");
        assert_eq!(
            error(root.new_volume_over_base("repo", &manifest).await),
            FxfsError::InvalidArgs
        );
        std::mem::drop(root);
        own.close().await.expect("Close failed");

        // Of a branch the allocator holds extents of: a fork of the base has them already.
        let fork = base.device().fork(SANDBOX).expect("fork failed");
        let forked = FxFilesystem::open(fork).await.expect("open failed");
        let root = root_volume(forked.clone()).await.expect("root_volume failed");
        assert_eq!(
            error(root.new_volume_over_base("copy", &manifest).await),
            FxfsError::InvalidArgs
        );
        std::mem::drop(root);
        forked.close().await.expect("Close failed");

        let (filesystem, _reads) = over(&base).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        // Of a later format.
        let mut later = Vec::new();
        Version { major: LATEST_VERSION.major + 1, minor: 0 }
            .serialize_into(&mut later)
            .expect("serialize failed");
        later.extend_from_slice(&manifest[later.len()..]);
        assert_eq!(
            error(root.new_volume_over_base("repo", &later).await),
            FxfsError::InvalidVersion
        );
        // Of no base at all.
        let mut empty = Vec::new();
        StoreInfo::default().serialize_with_version(&mut empty).expect("serialize failed");
        assert_eq!(error(root.new_volume_over_base("repo", &empty).await), FxfsError::InvalidArgs);
        // Of a version and no more.
        let mut short = Vec::new();
        LATEST_VERSION.serialize_into(&mut short).expect("serialize failed");
        assert_eq!(error(root.new_volume_over_base("repo", &short).await), FxfsError::InvalidArgs);
        root.new_volume_over_base("repo", &[0xff; 16]).await.err().expect("garbage accepted");
        assert_eq!(root.volume_directory().lookup("repo").await.expect("lookup failed"), None);
        // Of a name taken.
        std::mem::drop(root.new_volume_over_base("repo", &manifest).await.expect("over_base"));
        assert_eq!(
            error(root.new_volume_over_base("repo", &manifest).await),
            FxfsError::AlreadyExists
        );
        std::mem::drop(root);
        filesystem.close().await.expect("Close failed");
        base.close().await.expect("Close failed");
    }
}
