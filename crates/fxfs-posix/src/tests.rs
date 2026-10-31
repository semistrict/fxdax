// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the queued unlinks of doc-6 (TASK-79.21), on a fake device: what the kernel's calls find
//! of them. Unlinks are queued as the guest's VFS queues them, which nothing on the host does.

use super::*;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
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

fn set_size(size: u64, valid: u32) -> SetAttr {
    SetAttr { valid: SET_SIZE | valid, size, ..SetAttr::default() }
}

/// What a truncate cut from the last block stays gone when a write past the end makes that block
/// part of the file again (xfstests generic/130).
#[fuchsia::test]
async fn a_write_past_the_end_finds_a_truncates_tail_zeroed() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    fs.write(ino, 0, &[b'c'; 8192]).await.unwrap();
    fs.setattr(ino, &set_size(1, 0)).await.unwrap();
    fs.write(ino, 65536, b"a").await.unwrap();

    let mut read = vec![0xff; 65537];
    assert_eq!(fs.read(ino, 0, &mut read).await.unwrap(), 65537);
    let mut expected = vec![0; 65537];
    (expected[0], expected[65536]) = (b'c', b'a');
    assert_eq!(read, expected);
    fs.stop_delayed().await.unwrap();
}

/// A truncate that says to keep the tail leaves the block where a DAX window maps it.
#[fuchsia::test]
async fn a_truncate_keeping_its_tail_leaves_the_last_block_in_place() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    fs.write(ino, 0, &[b'c'; 8192]).await.unwrap();
    let block = fs.dax_extents(ino, 0..4096).await.unwrap();
    fs.setattr(ino, &set_size(1, SET_KEEP_TAIL)).await.unwrap();
    let kept = fs.dax_extents(ino, 0..4096).await.unwrap();
    assert_eq!(format!("{kept:?}"), format!("{block:?}"));
    fs.stop_delayed().await.unwrap();
}

/// The last bytes a file can hold are read back as written (xfstests generic/303, generic/525).
#[fuchsia::test]
async fn a_files_last_bytes_read_back() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    const LAST: u64 = i64::MAX as u64 - 1;
    fs.write(ino, LAST, b"a").await.unwrap();
    let mut read = [0; 1];
    assert_eq!(fs.read(ino, LAST, &mut read).await.unwrap(), 1);
    assert_eq!(&read, b"a");
    let mut parts = [0; 4096];
    let start = LAST & !4095;
    let got = fs.read_into(ino, start, &mut [&mut parts[..]]).await.unwrap();
    assert_eq!((got, parts[4094]), (4095, b'a'));
    // And from the layers a compaction leaves, where a mount finds it.
    fs.filesystem().journal().force_compact().await.unwrap();
    let got = fs.read_into(ino, start, &mut [&mut parts[..]]).await.unwrap();
    assert_eq!((got, parts[4094]), (4095, b'a'));
    // And as readahead asks, and past the end, for far more than is there.
    let mut window = vec![0xff; 4 << 20];
    let before = i64::MAX as u64 - (2 << 20) + 1;
    let got = fs.read_into(ino, before, &mut [&mut window[..]]).await.unwrap();
    assert_eq!(got as u64, i64::MAX as u64 - before);
    assert_eq!((window[got - 1], window[0]), (b'a', 0));
    let got = fs.read(ino, before, &mut window).await.unwrap();
    assert_eq!(window[got - 1], b'a');
    fs.stop_delayed().await.unwrap();
}

/// A new node's ctime is no earlier than its mtime, whatever it is (xfstests generic/423).
#[fuchsia::test]
async fn a_new_nodes_ctime_is_no_earlier_than_its_mtime() {
    let (fs, root) = volume().await;
    let fifo = 0o010000 | 0o644;
    for (name, mode) in [(&b"fifo"[..], fifo), (b"file", FILE), (b"dir", DIR)] {
        let [attr, _] = fs.create(root, name, mode, 0, 0, 0, &[]).await.unwrap();
        assert!(attr.ctime_ns >= attr.mtime_ns, "{name:?}: {attr:?}");
    }
    let [attr, _] = fs.symlink(root, b"link", b"file", 0, 0).await.unwrap();
    assert!(attr.ctime_ns >= attr.mtime_ns, "the symlink: {attr:?}");
    fs.stop_delayed().await.unwrap();
}

/// As `a_files_last_bytes_read_back`, across an unmount, which xfstests' test does between them.
#[fuchsia::test]
async fn a_files_last_bytes_read_back_after_a_remount() {
    const LAST: u64 = i64::MAX as u64 - 1;
    let device = DeviceHolder::new(FakeDevice::new(16384, 4096));
    let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
    let root = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
    let dir = volume.root_directory_object_id();
    let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
    let ino = fs.new_file(dir, b"a").await;
    fs.write(ino, LAST, b"a").await.unwrap();
    fs.stop_delayed().await.unwrap();
    drop((fs, root));
    filesystem.close().await.expect("closing");
    let device = filesystem.take_device().await;
    device.reopen(false);

    let filesystem = FxFilesystem::open(device).await.expect("opening");
    let root = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root.volume("test", StoreOptions::default()).await.unwrap();
    let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
    let mut read = vec![0xff; 4096];
    let start = LAST & !4095;
    assert_eq!(fs.read(ino, start, &mut read).await.unwrap(), 4095);
    assert_eq!(read[4094], b'a');
    let mut window = vec![0xff; 4 << 20];
    let got = fs.read_into(ino, i64::MAX as u64 - (2 << 20) + 1, &mut [&mut window[..]]).await;
    assert_eq!(window[got.unwrap() - 1], b'a');
    fs.stop_delayed().await.unwrap();
}
