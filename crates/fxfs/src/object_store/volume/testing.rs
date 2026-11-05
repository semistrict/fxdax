// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of volume.rs: the files and volumes tests of fxdax's volumes make.

use super::{RootVolume, root_volume};
use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem};
use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use crate::object_store::directory::Directory;
use crate::object_store::transaction::{Options, lock_keys};
use crate::object_store::{DataObjectHandle, HandleOptions, LockKey, ObjectStore};
use std::sync::Arc;
use storage_device::DeviceHolder;

/// Opens the file `name` in the root directory of `store`.
pub(super) async fn open_file(
    store: &Arc<ObjectStore>,
    name: &str,
) -> DataObjectHandle<ObjectStore> {
    let root_directory =
        Directory::open(store, store.root_directory_object_id()).await.expect("open failed");
    let (object_id, _, _) =
        root_directory.lookup(name).await.expect("lookup failed").expect("not found");
    ObjectStore::open_object(store, object_id, HandleOptions::default(), None)
        .await
        .expect("open_object failed")
}

/// The contents of the file `name` in the root directory of `store`.
pub(super) async fn read_file(store: &Arc<ObjectStore>, name: &str) -> Vec<u8> {
    let handle = open_file(store, name).await;
    let size = handle.get_size() as usize;
    let mut buf = handle.allocate_buffer(size).await;
    assert_eq!(handle.read_aligned(0, buf.as_mut()).await.expect("read failed"), size);
    buf.as_ref().to_vec()
}

/// Writes `contents` at the start of the file `name` in the root directory of `store`,
/// creating it if it doesn't exist.
pub(super) async fn write_file(
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

pub(super) fn lazily(device: DeviceHolder) -> impl Future<Output = OpenFxFilesystem> {
    async { FxFilesystemBuilder::new().lazy_volumes(true).open(device).await.expect("open failed") }
}

/// The store of the volume `name`, if it is open.
pub(super) async fn open_store(filesystem: &FxFilesystem, root: &RootVolume, name: &str) -> bool {
    let (object_id, _, _) = root.volume_directory().lookup(name).await.unwrap().unwrap();
    filesystem.object_manager().store(object_id).is_some()
}

/// Deletes the volume `name`.
pub(super) async fn delete_volume(fs: &Arc<FxFilesystem>, name: &str) {
    let root = root_volume(fs.clone()).await.expect("root_volume failed");
    let (_, transaction) = root
        .acquire_transaction_for_remove_volume(name, [], false)
        .await
        .expect("acquire transaction failed");
    root.delete_volume(name, transaction, || {}).await.expect("delete_volume failed");
}
