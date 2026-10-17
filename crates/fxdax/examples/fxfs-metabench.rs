// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What Fxfs's metadata operations cost, without a kernel or a device: each as the guest kernel's
//! Fxfs does it (guest/rust/fxfs-linux), on a device in memory, with the guest's options, timed
//! over many files. For profiling Fxfs itself, with perf on Linux, say:
//!
//! ```sh
//! cargo run --release -p fxdax --example fxfs-metabench [FILES [JOURNAL_MIB [io]]]
//! ```
//!
//! JOURNAL_MIB sets the journal's reclaim size, the guest's by default (0). With `io`, the
//! operations follow what test_fs_bench.rs's io workloads leave behind: a file of 256 MiB written
//! sequentially, then 64 MiB of it at random 4 KiB blocks, and removed.
//!
//! prints each operation's rate, and for unlink, the graveyard's freeing of the objects it
//! unlinked, which the guest does as the kernel evicts each inode, timed on its own and with it.

use fidl_fuchsia_io as fio;
use fuchsia_async::LocalExecutor;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{ReplacedChild, replace_child};
use fxfs::object_store::journal::{JournalOptions, LazyFlush};
use fxfs::object_store::transaction::{LockKey, LockKeys, Options, Transaction};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    Directory, HandleOptions, NewChildStoreOptions, ObjectDescriptor, ObjectKey, ObjectStore,
    SetExtendedAttributeMode, StoreObjectHandle, Timestamp,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const BLOCK_SIZE: u32 = 4096;
/// The benchmark's extent store's capacity (test_fs_bench.rs's IMAGE_SIZE).
const CAPACITY: u64 = 8 << 30;
/// The guest's options (guest/rust/fxfs-linux/src/fs.rs), for a guest of 4 GiB.
const MEMORY: u64 = 4 << 30;
const LAZY_FLUSH: LazyFlush = LazyFlush { bytes: 128 << 10, delay: Duration::from_secs(5) };

struct Bench {
    fs: OpenFxFilesystem,
    store: Arc<ObjectStore>,
}

impl Bench {
    async fn new(reclaim_size: u64) -> Self {
        let device = DeviceHolder::new(FakeDevice::new(CAPACITY / BLOCK_SIZE as u64, BLOCK_SIZE));
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .object_cache_items((MEMORY / 64 / 256) as usize)
            // The graveyard frees the objects unlinked when flushed, so that its work is timed
            // apart from the unlinks'.
            .graveyard_batch_delay(Duration::from_secs(3600))
            .journal_options(JournalOptions {
                reclaim_size,
                lazy_flush: Some(LAZY_FLUSH),
                ..Default::default()
            })
            .open(device)
            .await
            .unwrap();
        let volume = root_volume(fs.clone()).await.unwrap();
        let store = volume.new_volume("default", NewChildStoreOptions::default()).await.unwrap();
        Self { fs, store }
    }

    async fn transaction(&self, inos: &[u64]) -> Transaction<'_> {
        let mut keys = LockKeys::with_capacity(inos.len());
        for &ino in inos {
            keys.push(LockKey::object(self.store.store_object_id(), ino));
        }
        self.store.new_transaction(keys, Options::default()).await.unwrap()
    }

    fn handle(&self, ino: u64) -> StoreObjectHandle<ObjectStore> {
        StoreObjectHandle::new(self.store.clone(), ino, false, HandleOptions::default(), false)
    }

    async fn mkdir(&self, parent: &Directory<ObjectStore>, name: &str) -> Directory<ObjectStore> {
        let mut transaction = self.transaction(&[parent.object_id()]).await;
        let dir = parent.create_child_dir(&mut transaction, name).await.unwrap();
        transaction.commit().await.unwrap();
        dir
    }

    /// The guest's create of a file: its mode and owner set in the same transaction, and the
    /// new file's and the directory's attributes read from the transaction.
    async fn create(&self, dir: &Directory<ObjectStore>, name: &str) -> u64 {
        let mut transaction = self.transaction(&[dir.object_id()]).await;
        let child = dir.create_child_file(&mut transaction, name).await.unwrap();
        let attributes = fio::MutableNodeAttributes {
            mode: Some(0o100644),
            uid: Some(0),
            gid: Some(0),
            rdev: Some(0),
            ..Default::default()
        };
        child.update_attributes(&mut transaction, Some(&attributes), None).await.unwrap();
        let store = self.store.store_object_id();
        for ino in [child.object_id(), dir.object_id()] {
            assert!(transaction.get_object_mutation(store, ObjectKey::object(ino)).is_some());
        }
        transaction.commit().await.unwrap();
        child.object_id()
    }

    /// What the bench's io workloads leave behind: a file of 256 MiB written sequentially, then
    /// 64 MiB of it at random 4 KiB blocks, and removed, its object freed.
    async fn io(&self, dir: &Directory<ObjectStore>) {
        const SIZE: u64 = 256 << 20;
        const CHUNK: usize = 1 << 20;
        const BLOCK: usize = 4096;
        let mut transaction = self.transaction(&[dir.object_id()]).await;
        let file = dir.create_child_file(&mut transaction, "data").await.unwrap();
        transaction.commit().await.unwrap();
        let mut buf = file.allocate_buffer(CHUNK).await;
        buf.as_mut().fill(0x5a);
        for offset in (0..SIZE).step_by(CHUNK) {
            file.write_or_append(Some(offset), buf.as_ref()).await.unwrap();
        }
        // fio's random writes: a block at a time, at offsets of a fixed sequence.
        let mut block = file.allocate_buffer(BLOCK).await;
        block.as_mut().fill(0xa5);
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..(64 << 20) / BLOCK {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let offset = (x % (SIZE / BLOCK as u64)) * BLOCK as u64;
            file.write_or_append(Some(offset), block.as_ref()).await.unwrap();
        }
        drop((buf, block));
        let ino = file.object_id();
        drop(file);
        self.replace(None, (dir, "data"), Some(ino)).await;
        self.fs.graveyard().flush().await;
    }

    /// The guest's lookup: the name, and the object's record.
    async fn stat(&self, dir: &Directory<ObjectStore>, name: &str) {
        let (ino, _, _) = dir.lookup(name).await.unwrap().unwrap();
        self.store.tree().find(&ObjectKey::object(ino)).await.unwrap().unwrap();
    }

    async fn setxattr(&self, ino: u64) {
        self.handle(ino)
            .set_extended_attribute(
                b"user.bench".to_vec(),
                b"a value".to_vec(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
    }

    /// The guest's link: the name, a reference more, and the object's change time.
    async fn link(&self, ino: u64, dir: &Directory<ObjectStore>, name: &str) {
        let mut transaction = self.transaction(&[dir.object_id(), ino]).await;
        dir.insert_child(&mut transaction, name, ino, ObjectDescriptor::File).await.unwrap();
        self.store.adjust_refs(&mut transaction, ino, 1).await.unwrap();
        self.handle(ino)
            .update_attributes(&mut transaction, None, Some(Timestamp::now()))
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    }

    /// The guest's rename or unlink, locking what the kernel says the names link.
    async fn replace(
        &self,
        src: Option<(&Directory<ObjectStore>, &str, u64)>,
        dst: (&Directory<ObjectStore>, &str),
        replaced: Option<u64>,
    ) {
        let store = self.store.store_object_id();
        let _guard = match replaced {
            Some(ino) => Some(self.fs.truncate_guard(store, ino).await),
            None => None,
        };
        let mut inos = vec![dst.0.object_id()];
        inos.extend(src.map(|(dir, _, ino)| [dir.object_id(), ino]).into_iter().flatten());
        inos.extend(replaced);
        let mut keys = LockKeys::with_capacity(inos.len());
        for ino in inos {
            keys.push(LockKey::object(store, ino));
        }
        let options = Options { borrow_metadata_space: src.is_none(), ..Default::default() };
        let mut transaction = self.store.new_transaction(keys, options).await.unwrap();
        let src = src.map(|(dir, name, _)| (dir, name));
        let replaced = replace_child(&mut transaction, src, dst).await.unwrap();
        transaction.commit().await.unwrap();
        if let ReplacedChild::Object(ino) = replaced {
            // The kernel evicts the inode, whose last name is gone.
            self.fs.graveyard().queue_tombstone_object(store, ino);
        }
    }
}

fn rate(name: &str, count: usize, elapsed: Duration) {
    println!(
        "{name:<22} {:>10.0} ops/s {:>8.2} µs/op",
        count as f64 / elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1e6 / count as f64
    );
}

async fn time<T>(name: &str, count: usize, work: impl Future<Output = T>) -> T {
    let start = Instant::now();
    let value = work.await;
    rate(name, count, start.elapsed());
    value
}

async fn workload(files: usize, reclaim_size: u64, io: bool) {
    let bench = Bench::new(reclaim_size).await;
    let store = &bench.store;
    let root = Directory::open(store, store.root_directory_object_id()).await.unwrap();
    if io {
        time("io", 1, bench.io(&root)).await;
    }
    let dir = bench.mkdir(&root, "meta").await;
    let other = bench.mkdir(&root, "meta-other").await;
    let names: Vec<String> = (0..files).map(|i| format!("file-{i}")).collect();

    let inos = time("create", files, async {
        let mut inos = Vec::with_capacity(files);
        for name in &names {
            inos.push(bench.create(&dir, name).await);
        }
        inos
    })
    .await;
    time("stat", files, async {
        for name in &names {
            bench.stat(&dir, name).await;
        }
    })
    .await;
    time("lookup-missing", files, async {
        for i in 0..files {
            assert!(dir.lookup(&format!("missing-{i}")).await.unwrap().is_none());
        }
    })
    .await;
    time("setxattr", files, async {
        for &ino in &inos {
            bench.setxattr(ino).await;
        }
    })
    .await;
    time("link", files, async {
        for (i, &ino) in inos.iter().enumerate() {
            bench.link(ino, &dir, &format!("link-{i}")).await;
        }
    })
    .await;
    time("rename", files, async {
        for (i, &ino) in inos.iter().enumerate() {
            let (from, to) = (format!("link-{i}"), format!("renamed-{i}"));
            bench.replace(Some((&dir, &from, ino)), (&dir, &to), None).await;
        }
    })
    .await;
    time("rename-across", files, async {
        for (i, &ino) in inos.iter().enumerate() {
            let (from, to) = (format!("renamed-{i}"), format!("moved-{i}"));
            bench.replace(Some((&dir, &from, ino)), (&other, &to), None).await;
        }
    })
    .await;
    // Linked twice: the first unlink leaves a name.
    time("unlink (linked)", files, async {
        for (name, &ino) in names.iter().zip(&inos) {
            bench.replace(None, (&dir, name), Some(ino)).await;
        }
    })
    .await;
    let start = Instant::now();
    time("unlink (last name)", files, async {
        for (i, &ino) in inos.iter().enumerate() {
            bench.replace(None, (&other, &format!("moved-{i}")), Some(ino)).await;
        }
    })
    .await;
    time("graveyard", files, bench.fs.graveyard().flush()).await;
    rate("unlink + graveyard", files, start.elapsed());
    // A file made and removed, each change fsynced, as a database's temporary files are: the
    // unlink borrows metadata space for its record in the journal, which each fsync pads.
    time("create, unlink, fsync", files, async {
        for i in 0..files {
            let name = format!("temp-{i}");
            let ino = bench.create(&dir, &name).await;
            bench.fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
            bench.replace(None, (&dir, &name), Some(ino)).await;
            bench.fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
        }
    })
    .await;
    bench.fs.close().await.unwrap();
}

fn main() {
    assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
    let mut args = std::env::args().skip(1);
    let files = args.next().map_or(20_000, |n| n.parse().unwrap());
    let reclaim_size = match args.next().map(|mib| mib.parse::<u64>().unwrap()) {
        None | Some(0) => (MEMORY.min(CAPACITY) / 32).min(8 << 20),
        Some(mib) => mib << 20,
    };
    let io = args.next().is_some_and(|arg| arg == "io");
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(move || {
            LocalExecutor::new().run_singlethreaded(Box::pin(workload(files, reclaim_size, io)))
        })
        .unwrap()
        .join()
        .unwrap();
}
