// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what the fuzz targets (crates/fxfs/fuzz) share: an image to start from, each time the
//! same, and what's asked of what they make of it.

use crate::filesystem::FxFilesystem;
use crate::fsck::{FsckOptions, fsck_with_options};
use crate::object_store::directory::Directory;
use crate::object_store::transaction::{LockKey, Options, lock_keys};
use crate::object_store::volume::root_volume;
use crate::object_store::{
    HandleOptions, NewChildStoreOptions, ObjectDescriptor, ObjectStore, StoreOptions,
};
use fuchsia_async as fasync;
use std::future::Future;
use std::sync::OnceLock;
use storage_device::DeviceHolder;
use storage_device::fake_device::{Crashes, FakeDevice};

const VOLUME: &str = "fuzzed";

/// Runs `done` on one thread, as a test of a case replays what it finds.
pub fn run(done: impl Future<Output = ()>) {
    fasync::LocalExecutorBuilder::new().build().run_singlethreaded(done);
}

/// A filesystem with a volume, a directory and a file, closed.
async fn new_image() -> FakeDevice {
    let crashes = Crashes::new(0, u32::MAX);
    let device = FakeDevice::new(8192, 4096).calm().record_crashes(crashes.clone());
    let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("formatting");
    let volumes = root_volume(fs.clone()).await.expect("the root volume");
    let store =
        volumes.new_volume(VOLUME, NewChildStoreOptions::default()).await.expect("the volume");
    let root = store.root_directory_object_id();
    let keys = lock_keys![LockKey::object(store.store_object_id(), root)];
    let mut transaction =
        store.new_transaction(keys, Options::default()).await.expect("a transaction");
    let root = Directory::open(&store, root).await.expect("the root directory");
    root.create_child_dir(&mut transaction, "directory").await.expect("a directory");
    root.create_child_file(&mut transaction, "file").await.expect("a file");
    transaction.commit().await.expect("committing");
    let device = fs.device();
    drop((volumes, store, root));
    fs.close().await.expect("closing");
    device.reopen(false);
    crashes.mark(0);
    crashes.last();
    device.flush().await.expect("flushing");
    let (_, image) = crashes.take().next().expect("what the device held");
    image
}

/// A copy of the image every input starts from.
pub fn image() -> FakeDevice {
    static IMAGE: OnceLock<FakeDevice> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let mut image = None;
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let made = &mut image;
                    run(async move { *made = Some(new_image().await) });
                });
            });
            image.expect("an image")
        })
        .recovered(0)
}

/// Mounts `device`, which may be refused, and if it isn't, checks it, reads what it can of the
/// volume, compacts it and unmounts it. That none of it panics is what's asked.
pub async fn mounts_without_panicking(device: DeviceHolder) {
    let Ok(fs) = FxFilesystem::open(device).await else { return };
    let options = FsckOptions { on_error: Box::new(|_| {}), quiet: true, ..FsckOptions::default() };
    let _ = fsck_with_options(fs.clone(), &options).await;
    let read = async {
        let volumes = root_volume(fs.clone()).await?;
        let store = volumes.volume(VOLUME, StoreOptions::default()).await?;
        let root = Directory::open(&store, store.root_directory_object_id()).await?;
        let layer_set = root.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut entries = root.iter(&mut merger).await?;
        while let Some((_, object_id, descriptor)) = entries.get() {
            if matches!(descriptor, ObjectDescriptor::File) {
                let handle =
                    ObjectStore::open_object(&store, object_id, HandleOptions::default(), None)
                        .await?;
                drop(handle);
            }
            entries.advance().await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let _ = read.await;
    // What it read goes into layers, which what writes them asserts things of.
    let _ = fs.journal().force_compact().await;
    // It can fail if what it must write finds something wrong: see `fuzz_journal`.
    let _ = fs.close().await;
}
