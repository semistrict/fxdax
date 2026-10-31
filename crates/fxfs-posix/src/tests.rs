// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the queued unlinks of doc-6 (TASK-79.21), on a fake device: what the kernel's calls find
//! of them. Unlinks are queued as the guest's VFS queues them, which nothing on the host does.

use super::*;
use fxfs::object_store::NewChildStoreOptions;
use fxfs::object_store::volume::root_volume;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const FILE: u32 = S_IFREG | 0o644;
const DIR: u32 = S_IFDIR | 0o755;
/// How long what is queued may take, a test's scheduling included, to go by itself.
const GOES_WITHIN: Duration = Duration::from_secs(30);

/// As the host does.
struct Tested;

impl Platform for Tested {
    const KEEPS_DIRECTORY_TIMES: bool = false;

    type Durable = ();

    fn durable(&self) {}

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        fasync::Task::spawn(future).detach();
    }
}

/// A new filesystem's volume, with its root directory.
async fn volume() -> (Fxfs<Tested>, u64) {
    let device = DeviceHolder::new(FakeDevice::new(16384, 4096));
    let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
    let root_volume = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root_volume.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
    let root = volume.root_directory_object_id();
    (Fxfs::new((*filesystem).clone(), volume, false, Tested), root)
}

impl Fxfs<Tested> {
    fn queued(&self) -> usize {
        self.delayed.queue.lock().unwrap().ops.len()
    }

    /// Waits for the queue to empty by itself.
    async fn goes_by_itself(&self) {
        let start = Instant::now();
        while self.queued() != 0 {
            assert!(start.elapsed() < GOES_WITHIN, "what is queued stays: {}", self.queued());
            fasync::Timer::new(Instant::now() + Duration::from_millis(1)).await;
        }
    }

    /// A new file in `dir`.
    async fn new_file(&self, dir: u64, name: &[u8]) -> u64 {
        self.create(dir, name, FILE, 0, 0, 0, &[]).await.expect("creating")[0].ino
    }

    /// A new directory in `dir`.
    async fn new_dir(&self, dir: u64, name: &[u8]) -> u64 {
        self.create(dir, name, DIR, 0, 0, 0, &[]).await.expect("creating")[0].ino
    }
}

/// An unlink no others join goes by itself: it holds its room, and its place in the journal, for
/// no sync.
#[fuchsia::test]
async fn a_lone_unlink_goes_by_itself() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    fs.unlink_later(root, b"a", ino).await.unwrap();
    assert_eq!(fs.queued(), 1);
    fs.goes_by_itself().await;
    fs.stop_delayed().await.unwrap();
}

/// So does an eviction queued after its unlink: it frees the object.
#[fuchsia::test]
async fn an_eviction_goes_by_itself() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    // As if its unlink were applying, out of the queue, and a batch of its own the eviction.
    fs.delayed.queue.lock().unwrap().pending.insert(ino, 0);
    fs.evict(ino, true);
    assert_eq!(fs.queued(), 1);
    fs.goes_by_itself().await;
    fs.stop_delayed().await.unwrap();
}

/// statfs counts what was unlinked as gone.
#[fuchsia::test]
async fn statfs_applies_what_is_queued() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    fs.unlink_later(root, b"a", ino).await.unwrap();
    fs.statfs().await.unwrap();
    assert_eq!(fs.queued(), 0);
    fs.stop_delayed().await.unwrap();
}

/// A change of a name, wherever it is, comes after the unlinks before it, in the journal too.
#[fuchsia::test]
async fn names_change_after_the_unlinks_queued() {
    let (fs, root) = volume().await;
    let (a, b) = (fs.new_dir(root, b"a").await, fs.new_dir(root, b"b").await);
    let ino = fs.new_file(a, b"lock").await;
    fs.unlink_later(a, b"lock", ino).await.unwrap();
    fs.new_file(b, b"done").await;
    assert_eq!(fs.queued(), 0, "a create in another directory went ahead of an unlink");
    fs.stop_delayed().await.unwrap();
}

/// A lookup, the kernel taking a file's link count from it, finds another of its names gone.
#[fuchsia::test]
async fn lookups_count_links_queued_to_go() {
    let (fs, root) = volume().await;
    let (a, b) = (fs.new_dir(root, b"a").await, fs.new_dir(root, b"b").await);
    let ino = fs.new_file(a, b"one").await;
    fs.link(ino, b, b"two", false).await.unwrap();
    assert_eq!(fs.lookup(b, b"two").await.unwrap().nlink, 2);
    fs.unlink_later(a, b"one", ino).await.unwrap();
    assert_eq!(fs.lookup(b, b"two").await.unwrap().nlink, 1);
    fs.stop_delayed().await.unwrap();
}

/// As does an exchange of names, its attributes being what the kernel keeps.
#[fuchsia::test]
async fn exchanges_count_links_queued_to_go() {
    let (fs, root) = volume().await;
    let (a, b) = (fs.new_dir(root, b"a").await, fs.new_dir(root, b"b").await);
    let ino = fs.new_file(a, b"one").await;
    fs.link(ino, b, b"two", false).await.unwrap();
    let other = fs.new_file(root, b"other").await;
    fs.unlink_later(a, b"one", ino).await.unwrap();
    let [first, second, ..] = fs.exchange(b, b"two", root, b"other").await.unwrap();
    assert_eq!((first.ino, second.ino), (ino, other));
    assert_eq!(first.nlink, 1);
    fs.stop_delayed().await.unwrap();
}

/// A batch that fails leaves the object it may have left a name to: an eviction frees nothing, and
/// changes fail.
#[fuchsia::test]
async fn a_failed_batch_keeps_what_the_kernel_evicts() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    let other = fs.new_file(root, b"b").await;
    // As a failed commit leaves it: the name stays, and the kernel has dropped it.
    fs.delayed.queue.lock().unwrap().failed = true;

    fs.evict(ino, true);
    fs.filesystem.graveyard().flush().await;
    assert_eq!(fs.lookup(root, b"a").await.unwrap().ino, ino);
    fs.create(root, b"c", FILE, 0, 0, 0, &[]).await.expect_err("a change after a failed batch");
    fs.unlink(root, b"b", other).await.expect_err("an unlink after a failed batch");
    fs.stop_delayed().await.unwrap();
}
