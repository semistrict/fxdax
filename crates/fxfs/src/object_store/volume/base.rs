// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of volume.rs: a volume of one another filesystem wrote, a base (backlog
//! doc-10). `RootVolume::publish_volume` gives its manifest, and `new_volume_over_base` makes a
//! volume over it, whose layers and data stay where they are, in extents it doesn't hold.

use super::RootVolume;
use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, ForceMajor};
use crate::object_handle::ObjectHandle;
use crate::object_store::transaction::{Options, lock_keys};
use crate::object_store::{
    HandleOptions, LockKey, MAX_MANIFEST_SIZE, ObjectDescriptor, ObjectStore, StoreInfo,
};
use crate::serialized_types::{LATEST_VERSION, Version, VersionedLatest};
use anyhow::{Context, Error, anyhow, bail, ensure};
use std::sync::Arc;

impl RootVolume {
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
}

#[cfg(test)]
mod tests {
    use crate::errors::FxfsError;
    use crate::filesystem::{FlushReason, ForceMajor, FxFilesystem, SyncOptions};
    use crate::lsm_tree::persistent_layer::PersistentLayerWriter;
    use crate::lsm_tree::types::LayerWriter as _;
    use crate::object_handle::ObjectHandle;
    use crate::object_store::directory::{Directory, ReplacedChild, replace_child};
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::volume::testing::{
        lazily, open_file, open_store, read_file, write_file,
    };
    use crate::object_store::volume::tests::do_fsck;
    use crate::object_store::{
        DirectWriter, HandleOptions, LockKey, NewChildStoreOptions, ObjectKey, ObjectStore,
        ObjectValue, StoreInfo, StoreOptions, tree,
    };
    use crate::serialized_types::{LATEST_VERSION, Version, VersionedLatest};
    use std::sync::Arc;
    use std::sync::atomic::Ordering::Relaxed;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;

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
