//! Fxfs on extent stores in memory: on a sparse address space, against the store's capacity,
//! releasing what it frees, and crashing at barriers (TASK-26.3).

use extent_store::{ADDRESS_SPACE, ExtentDevice, ExtentStore, MemoryStore};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::fsck::fsck;
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::replace_child;
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{Directory, NewChildStoreOptions, ObjectStore};
use rand::SeedableRng as _;
use std::sync::Arc;
use storage_device::DeviceHolder;

const MIB: u64 = 1 << 20;

fn device(store: Arc<MemoryStore>) -> DeviceHolder {
    DeviceHolder::new(ExtentDevice::new(store))
}

/// A new filesystem on `store`, with barriers, which the extent store orders by.
async fn format(store: Arc<MemoryStore>) -> OpenFxFilesystem {
    FxFilesystemBuilder::new()
        .barriers_enabled(true)
        .format(true)
        .open(device(store))
        .await
        .unwrap()
}

/// The volume "vol" and its root directory.
async fn volume(
    fs: &Arc<FxFilesystem>,
    create: bool,
) -> (Arc<ObjectStore>, Directory<ObjectStore>) {
    let root = root_volume(fs.clone()).await.unwrap();
    let store = if create {
        root.new_volume("vol", NewChildStoreOptions::default()).await.unwrap()
    } else {
        root.volume("vol", Default::default()).await.unwrap()
    };
    let dir = Directory::open(&store, store.root_directory_object_id()).await.unwrap();
    (store, dir)
}

/// Creates `name` in `dir`, holding `len` bytes of `byte`, written a MiB at a time, as no one
/// transaction can hold more.
async fn create_file(
    store: &Arc<ObjectStore>,
    dir: &Directory<ObjectStore>,
    name: &str,
    len: usize,
    byte: u8,
) -> Result<(), anyhow::Error> {
    let mut transaction = store
        .new_transaction(
            lock_keys![LockKey::object(store.store_object_id(), dir.object_id())],
            Options::default(),
        )
        .await?;
    let file = dir.create_child_file(&mut transaction, name).await?;
    transaction.commit().await?;
    let mut buf = file.allocate_buffer(len.min(MIB as usize)).await;
    buf.fill(byte);
    let mut offset = 0;
    while offset < len {
        let chunk = (len - offset).min(buf.len());
        file.write_or_append(Some(offset as u64), buf.subslice(..chunk)).await?;
        offset += chunk;
    }
    Ok(())
}

/// The contents of `name` in `dir`.
async fn read_file(store: &Arc<ObjectStore>, dir: &Directory<ObjectStore>, name: &str) -> Vec<u8> {
    let (id, _, _) = dir.lookup(name).await.unwrap().unwrap();
    let file = ObjectStore::open_object(store, id, Default::default(), None).await.unwrap();
    let mut buf = file.allocate_buffer(file.get_size() as usize).await;
    let len = file.read_aligned(0, buf.as_mut()).await.unwrap();
    buf.as_ref().to_vec()[..len.min(file.get_size() as usize)].to_vec()
}

#[fuchsia::test]
async fn fxfs_lives_in_a_sparse_address_space() {
    let store = Arc::new(MemoryStore::new(64 * MIB));
    let fs = format(store.clone()).await;
    assert_eq!(fs.device().size(), ADDRESS_SPACE);
    let (vol, dir) = volume(&fs, true).await;
    create_file(&vol, &dir, "a", MIB as usize, 1).await.unwrap();
    drop((vol, dir));
    fs.close().await.unwrap();
    let device = fs.take_device().await;
    device.reopen(false);

    let fs = FxFilesystem::open(device).await.unwrap();
    fsck(fs.clone()).await.unwrap();
    let (vol, dir) = volume(&fs, false).await;
    assert_eq!(read_file(&vol, &dir, "a").await, vec![1; MIB as usize]);
    // The store holds what was written: the file, and a little metadata.
    let used = store.info().await.unwrap().used;
    assert!(used > MIB && used < 16 * MIB, "the store holds {used} bytes");
    drop((vol, dir));
    fs.close().await.unwrap();
}

#[fuchsia::test]
async fn space_runs_out_at_the_stores_capacity() {
    let store = Arc::new(MemoryStore::new(32 * MIB));
    let fs = format(store.clone()).await;
    let (vol, dir) = volume(&fs, true).await;
    let mut error = None;
    for i in 0..64 {
        if let Err(e) = create_file(&vol, &dir, &format!("f{i}"), MIB as usize, 2).await {
            error = Some(e);
            break;
        }
    }
    // Fxfs's allocator refuses, before the store would.
    let error = error.expect("32 MiB held 64 MiB of files");
    assert!(FxfsError::NoSpace.matches(&error), "{error:?}");
    drop((vol, dir));
    fs.close().await.unwrap();
}

#[fuchsia::test]
async fn freed_space_is_released_to_the_store() {
    let store = Arc::new(MemoryStore::new(256 * MIB));
    let fs = format(store.clone()).await;
    let (vol, dir) = volume(&fs, true).await;
    create_file(&vol, &dir, "big", 32 * MIB as usize, 3).await.unwrap();
    fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    let full = store.info().await.unwrap().used;
    assert!(full >= 32 * MIB, "the store holds {full} bytes");

    let (id, _, _) = dir.lookup("big").await.unwrap().unwrap();
    let mut transaction = vol
        .new_transaction(
            lock_keys![
                LockKey::object(vol.store_object_id(), dir.object_id()),
                LockKey::object(vol.store_object_id(), id)
            ],
            Options::default(),
        )
        .await
        .unwrap();
    replace_child(&mut transaction, None, (&dir, "big")).await.unwrap();
    transaction.commit().await.unwrap();
    // The graveyard deallocates the file, which the next flush of the device frees.
    fs.graveyard().queue_tombstone_object(vol.store_object_id(), id);
    fs.graveyard().flush().await;
    fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    let used = store.info().await.unwrap().used;
    // The file's 32 MiB is released; metadata may have grown a little meanwhile.
    assert!(used + 31 * MIB <= full, "the store holds {used} bytes, {full} before");
    drop((vol, dir));
    fs.close().await.unwrap();
}

/// Fxfs writes, renames and unlinks while the store crashes, at random barriers, again and again;
/// each crash leaves a filesystem that mounts and checks.
#[fuchsia::test]
async fn fxfs_survives_crashes_at_any_barrier() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(26_3);
    let store = Arc::new(MemoryStore::with_history(256 * MIB));
    let fs = format(store.clone()).await;
    let (vol, dir) = volume(&fs, true).await;
    // Formatting is done, and durable: crashes leave what followed it.
    fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    store.commit(true).await.unwrap();
    let mut crashes = Vec::new();
    for round in 0..40 {
        for i in 0..8 {
            create_file(&vol, &dir, &format!("r{round}f{i}"), 4096 * (1 + i), i as u8)
                .await
                .unwrap();
        }
        if round > 0 {
            let name = format!("r{}f0", round - 1);
            let (id, _, _) = dir.lookup(&name).await.unwrap().unwrap();
            let mut transaction = vol
                .new_transaction(
                    lock_keys![
                        LockKey::object(vol.store_object_id(), dir.object_id()),
                        LockKey::object(vol.store_object_id(), id)
                    ],
                    Options::default(),
                )
                .await
                .unwrap();
            replace_child(&mut transaction, Some((&dir, &name)), (&dir, &format!("moved{round}")))
                .await
                .unwrap();
            transaction.commit().await.unwrap();
        }
        // fsync's ordering: a sync's barrier, which does not wait for durability.
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        crashes.push(store.crash(&mut rng));
    }
    assert!(store.barriers_since_durable() > 40);
    drop((vol, dir));
    fs.close().await.unwrap();

    for (i, crashed) in crashes.into_iter().enumerate() {
        let fs = FxFilesystem::open(device(Arc::new(crashed)))
            .await
            .unwrap_or_else(|e| panic!("crash {i} left a filesystem that does not mount: {e:?}"));
        fsck(fs.clone()).await.unwrap_or_else(|e| panic!("crash {i}: {e:?}"));
        fs.close().await.unwrap();
    }
}
