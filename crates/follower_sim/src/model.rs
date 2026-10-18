// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The writer, Fxfs read-write on the store, its operations on a few files of its root directory,
//! and the model of what they leave: the oracle of what a follower may show.

use crate::device::StoreDevice;
use extent_store::{Dir, Store};
use fxfs::filesystem::{FxFilesystem, OpenFxFilesystem, SyncOptions};
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{Directory, replace_child};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{HandleOptions, ObjectStore};
use shuttle::rand::Rng;
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_device::DeviceHolder;

pub const BLOCK: u64 = 4096;

/// The names of the files the writer makes.
pub const NAMES: [&str; 4] = ["a", "b", "c", "d"];

/// The blocks a file written holds.
pub const BLOCKS: u64 = 2;

/// A file's contents: `blocks` blocks, each byte of them `fill`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contents {
    pub fill: u8,
    pub blocks: u64,
}

/// The writer's files, by name, as its operations left them.
pub type Model = BTreeMap<&'static str, Contents>;

/// An operation of the writer's.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Create(&'static str),
    Write(&'static str, u8),
    Rename(&'static str, &'static str),
    Unlink(&'static str),
    /// Fxfs's sync, which flushes the device: a commit of the store, after which every operation
    /// before it is in a commit.
    Sync,
}

impl Op {
    /// A random operation valid on `model`.
    pub fn random(model: &Model, rng: &mut impl Rng) -> Op {
        loop {
            let name = NAMES[rng.gen_range(0..NAMES.len())];
            let exists = model.contains_key(name);
            match rng.gen_range(0..8) {
                0 | 1 if !exists => return Op::Create(name),
                2 | 3 if exists => return Op::Write(name, rng.gen_range(1..=255)),
                4 if exists => {
                    let to = NAMES[rng.gen_range(0..NAMES.len())];
                    if to != name {
                        return Op::Rename(name, to);
                    }
                }
                5 if exists => return Op::Unlink(name),
                6 => return Op::Sync,
                _ => {}
            }
        }
    }

    /// What it makes of `model`.
    pub fn apply(self, model: &mut Model) {
        match self {
            Op::Create(name) => {
                model.insert(name, Contents { fill: 0, blocks: 0 });
            }
            Op::Write(name, fill) => {
                model.insert(name, Contents { fill, blocks: BLOCKS });
            }
            Op::Rename(from, to) => {
                let contents = model.remove(from).unwrap();
                model.insert(to, contents);
            }
            Op::Unlink(name) => {
                model.remove(name);
            }
            Op::Sync => {}
        }
    }
}

/// The writer: Fxfs, read-write, on the store's branch.
pub struct Writer {
    pub fs: OpenFxFilesystem,
    root: Directory<ObjectStore>,
}

impl Writer {
    /// Makes a filesystem on `store`, empty.
    pub async fn new<D: Dir + 'static>(store: Arc<Store<D>>) -> Self {
        let device = DeviceHolder::new(StoreDevice::new(store));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let root = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("opening the root directory failed");
        Self { fs, root }
    }

    /// The object ID of the file `name`, if there is one.
    async fn object_id(&self, name: &str) -> Option<u64> {
        self.root.lookup(name).await.expect("lookup failed").map(|(id, ..)| id)
    }

    /// Makes `op`.
    pub async fn apply(&self, op: Op) {
        let store = self.fs.root_store();
        let lock = |object_id| LockKey::object(store.store_object_id(), object_id);
        let object_id = |name| self.object_id(name);
        match op {
            Op::Create(name) => {
                let mut transaction = store
                    .new_transaction(lock_keys![lock(self.root.object_id())], Options::default())
                    .await
                    .expect("new_transaction failed");
                self.root.create_child_file(&mut transaction, name).await.expect("create failed");
                transaction.commit().await.expect("commit failed");
            }
            Op::Write(name, fill) => {
                let id = object_id(name).await.expect("no file to write");
                let handle = ObjectStore::open_object(&store, id, HandleOptions::default(), None)
                    .await
                    .expect("open failed");
                let mut buf = handle.allocate_buffer((BLOCKS * BLOCK) as usize).await;
                buf.fill(fill);
                handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
            }
            Op::Rename(from, to) => {
                let source = object_id(from).await.expect("no file to rename");
                let keys = match object_id(to).await {
                    Some(target) => {
                        lock_keys![lock(self.root.object_id()), lock(source), lock(target)]
                    }
                    None => lock_keys![lock(self.root.object_id()), lock(source)],
                };
                let mut transaction =
                    store.new_transaction(keys, Options::default()).await.expect("transaction");
                replace_child(&mut transaction, Some((&self.root, from)), (&self.root, to))
                    .await
                    .expect("rename failed");
                transaction.commit().await.expect("commit failed");
            }
            Op::Unlink(name) => {
                let id = object_id(name).await.expect("no file to unlink");
                let keys = lock_keys![lock(self.root.object_id()), lock(id)];
                let mut transaction =
                    store.new_transaction(keys, Options::default()).await.expect("transaction");
                replace_child(&mut transaction, None, (&self.root, name))
                    .await
                    .expect("unlink failed");
                transaction.commit().await.expect("commit failed");
            }
            Op::Sync => {
                let options = SyncOptions { flush_device: true, ..SyncOptions::default() };
                self.fs.sync(options).await.expect("sync failed");
            }
        }
    }
}

/// What a reader of `fs` finds of the file `name` in its root directory: its contents, if it is
/// there, or `Err` with the bytes of a file whose blocks aren't each one byte.
pub async fn observe(fs: &OpenFxFilesystem, name: &str) -> Result<Option<Contents>, String> {
    let store = fs.root_store();
    let root = Directory::open(&store, store.root_directory_object_id())
        .await
        .map_err(|error| format!("opening the root directory: {error:#}"))?;
    let Some((id, ..)) =
        root.lookup(name).await.map_err(|error| format!("looking {name} up: {error:#}"))?
    else {
        return Ok(None);
    };
    let handle = ObjectStore::open_object(&store, id, HandleOptions::default(), None)
        .await
        .map_err(|error| format!("opening {name}: {error:#}"))?;
    let size = handle.get_size();
    if size == 0 {
        return Ok(Some(Contents { fill: 0, blocks: 0 }));
    }
    let mut buf = handle.allocate_buffer(size as usize).await;
    let read = handle
        .read_aligned(0, buf.as_mut())
        .await
        .map_err(|error| format!("reading {name}: {error:#}"))?;
    let bytes = buf.to_vec();
    let fill = bytes[0];
    if read != size as usize || bytes.iter().any(|byte| *byte != fill) {
        return Err(format!("{name} reads torn: {read} of {size} bytes, not all {fill:#x}"));
    }
    Ok(Some(Contents { fill, blocks: size / BLOCK }))
}
