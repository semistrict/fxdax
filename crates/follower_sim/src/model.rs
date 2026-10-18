// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The writer, Fxfs read-write on the store, its operations on a few files of its root directory,
//! and the model of what they leave: the oracle of what a follower may show. The model knows each
//! file's object, so that what a reader holding one open reads of it can be checked too.

use crate::device::StoreDevice;
use extent_store::{Dir, Store};
use fxfs::filesystem::{FxFilesystem, OpenFxFilesystem, SyncOptions};
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{Directory, ReplacedChild, replace_child_with_purge};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{HandleOptions, ObjectStore};
use shuttle::rand::{Rng, thread_rng};
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

/// A file of the writer's: its object, and its contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    pub object_id: u64,
    pub contents: Contents,
}

/// The writer's files, by name, as its operations left them.
pub type Model = BTreeMap<&'static str, File>;

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
    /// A compaction of the journal: every store's tree flushed to new layers, the old ones freed,
    /// and a new super-block.
    Compact,
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
                7 if rng.gen_range(0..3) == 0 => return Op::Compact,
                _ => {}
            }
        }
    }

    /// What it makes of `model`, the object it created, if it made one, `created`.
    pub fn apply(self, model: &mut Model, created: Option<u64>) {
        match self {
            Op::Create(name) => {
                let object_id = created.unwrap();
                model.insert(name, File { object_id, contents: Contents { fill: 0, blocks: 0 } });
            }
            Op::Write(name, fill) => {
                model.get_mut(name).unwrap().contents = Contents { fill, blocks: BLOCKS };
            }
            Op::Rename(from, to) => {
                let file = model.remove(from).unwrap();
                model.insert(to, file);
            }
            Op::Unlink(name) => {
                model.remove(name);
            }
            Op::Sync | Op::Compact => {}
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

    /// Purges the file an operation unlinked last, if its transaction didn't: as a mount does a
    /// file no one has open, at once (or its graveyard, later).
    async fn purge(&self, replaced: ReplacedChild) {
        if let ReplacedChild::Object(object_id) = replaced {
            let store = self.fs.root_store();
            store
                .tombstone_object(object_id, Options::default(), None)
                .await
                .expect("purge failed");
        }
    }

    /// Makes `op`, returning the object it created, if it made one.
    pub async fn apply(&self, op: Op) -> Option<u64> {
        let store = self.fs.root_store();
        let lock = |object_id| LockKey::object(store.store_object_id(), object_id);
        let object_id = |name| self.object_id(name);
        match op {
            Op::Create(name) => {
                let mut transaction = store
                    .new_transaction(lock_keys![lock(self.root.object_id())], Options::default())
                    .await
                    .expect("new_transaction failed");
                let handle = self
                    .root
                    .create_child_file(&mut transaction, name)
                    .await
                    .expect("create failed");
                transaction.commit().await.expect("commit failed");
                return Some(handle.object_id());
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
                let source = Some((&self.root, from));
                let purge = thread_rng().gen_bool(0.5);
                let replaced =
                    replace_child_with_purge(&mut transaction, source, (&self.root, to), purge)
                        .await
                        .expect("rename failed");
                transaction.commit().await.expect("commit failed");
                self.purge(replaced).await;
            }
            Op::Unlink(name) => {
                let id = object_id(name).await.expect("no file to unlink");
                let keys = lock_keys![lock(self.root.object_id()), lock(id)];
                let mut transaction =
                    store.new_transaction(keys, Options::default()).await.expect("transaction");
                let purge = thread_rng().gen_bool(0.5);
                let replaced =
                    replace_child_with_purge(&mut transaction, None, (&self.root, name), purge)
                        .await
                        .expect("unlink failed");
                transaction.commit().await.expect("commit failed");
                self.purge(replaced).await;
            }
            Op::Sync => {
                let options = SyncOptions { flush_device: true, ..SyncOptions::default() };
                self.fs.sync(options).await.expect("sync failed");
            }
            Op::Compact => {
                self.fs.journal().force_compact().await.expect("compaction failed");
            }
        }
        None
    }
}

/// The object of the file `name` in the root directory of `fs`, if there is one.
pub async fn lookup(fs: &OpenFxFilesystem, name: &str) -> Result<Option<u64>, String> {
    let store = fs.root_store();
    let root = Directory::open(&store, store.root_directory_object_id())
        .await
        .map_err(|error| format!("opening the root directory: {error:#}"))?;
    let found = root.lookup(name).await.map_err(|error| format!("looking {name} up: {error:#}"))?;
    Ok(found.map(|(id, ..)| id))
}

/// What a reader of `fs` finds of the file `name` in its root directory: its contents, if it is
/// there, or `Err` with the bytes of a file whose blocks aren't each one byte.
pub async fn observe(fs: &OpenFxFilesystem, name: &str) -> Result<Option<Contents>, String> {
    match lookup(fs, name).await? {
        Some(object_id) => read(fs, object_id).await.map(Some),
        None => Ok(None),
    }
}

/// What a reader of `fs` reads of the file whose object is `object_id`, in its root store: its
/// contents, or `Err` with the bytes of a file whose blocks aren't each one byte.
pub async fn read(fs: &OpenFxFilesystem, object_id: u64) -> Result<Contents, String> {
    let store = fs.root_store();
    let name = format!("object {object_id}");
    let handle = ObjectStore::open_object(&store, object_id, HandleOptions::default(), None)
        .await
        .map_err(|error| format!("opening {name}: {error:#}"))?;
    let size = handle.get_size();
    if size == 0 {
        return Ok(Contents { fill: 0, blocks: 0 });
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
    Ok(Contents { fill, blocks: size / BLOCK })
}
