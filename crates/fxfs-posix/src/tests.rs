// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the queued unlinks of doc-6 (TASK-79.21), on a fake device: what the kernel's calls find
//! of them. Unlinks are queued as the guest's VFS queues them, which nothing on the host does.

use super::*;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use fxfs::object_store::volume::root_volume;
use storage_device::DeviceHolder;
use storage_device::ExtentId;
use storage_device::fake_device::{FakeDevice, Op as DeviceOp};

const FILE: u32 = S_IFREG | 0o644;
const DIR: u32 = S_IFDIR | 0o755;
/// How long what is queued may take, a test's scheduling included, to go by itself.
const GOES_WITHIN: Duration = Duration::from_secs(30);

/// As the host does.
pub(super) struct Tested;

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
    volume_on(FakeDevice::new(16384, 4096)).await
}

/// A new filesystem's volume on `device`, with its root directory.
async fn volume_on(device: FakeDevice) -> (Fxfs<Tested>, u64) {
    let device = DeviceHolder::new(device);
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
    let created = fs.create(root, b"c", FILE, 0, 0, 0, &[]).await;
    assert_eq!(errno(&created.expect_err("a change after a failed batch")), Errno::EIO);
    let unlinked = fs.unlink(root, b"b", other).await;
    assert_eq!(errno(&unlinked.expect_err("an unlink after a failed batch")), Errno::EIO);
    fs.stop_delayed().await.unwrap();
}

/// An object freed in the batch that unlinks it takes what a truncate short of room left in the
/// graveyard with it (TASK-82): fsck found the entry of an object gone. Too narrow for sequences to
/// reach, 6000 of them missing it with the bug back, so this makes the state.
#[fuchsia::test]
async fn an_object_freed_in_a_batch_takes_a_trim_left_in_the_graveyard() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"a").await;
    let entry = ObjectKey::graveyard_entry(fs.volume.graveyard_directory_object_id(), ino);
    let mut transaction = fs.transaction(&[ino]).await.unwrap();
    transaction.add(
        fs.volume.store_object_id(),
        Mutation::replace_or_insert_object(entry, ObjectValue::Trim),
    );
    transaction.commit().await.unwrap();
    assert!(fs.in_graveyard(ino).await.unwrap());

    fs.unlink_later(root, b"a", ino).await.unwrap();
    fs.evict(ino, true);
    fs.sync().await.unwrap();
    assert!(!fs.in_graveyard(ino).await.unwrap(), "the object went, and its entry stayed");
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

/// Whatever grows a file past its end, what its last block held past the end reads back as zeros,
/// though the truncate that left it there didn't zero it, as one with no room can't: the growth
/// zeroes it, every way a file grows (TASK-150.19, TASK-153.4).
#[fuchsia::test]
async fn every_growth_finds_what_a_cut_left_past_the_end_zeroed() {
    let (fs, root) = volume().await;
    let source = fs.new_file(root, b"source").await;
    fs.write(source, 0, &[b's'; 8192]).await.unwrap();
    let growths: [(&str, u64); 6] = [
        ("a write in the block", 2000),
        ("a write past it", 65537),
        ("a truncate", 65537),
        ("an allocation in the block", 2000),
        ("an allocation past it", 65537),
        ("a clone past it", 65536 + 8192),
    ];
    for (i, (growth, size)) in growths.into_iter().enumerate() {
        let ino = fs.new_file(root, format!("grown{i}").as_bytes()).await;
        fs.write(ino, 0, &[b'c'; 8192]).await.unwrap();
        // A cut that leaves the last block as it was: what one with no room to zero it does.
        fs.setattr(ino, &set_size(1000, SET_KEEP_TAIL)).await.unwrap();
        match growth {
            "a write in the block" => fs.write(ino, 1999, b"w").await.unwrap(),
            "a write past it" => fs.write(ino, 65536, b"w").await.unwrap(),
            "a truncate" => fs.setattr(ino, &set_size(size, 0)).await.unwrap(),
            "an allocation in the block" => fs.allocate(ino, 1000..2000).await.unwrap(),
            "an allocation past it" => fs.allocate(ino, 65536..65537).await.unwrap(),
            "a clone past it" => {
                fs.clone_range(source, 0..8192, ino, 65536, size).await.unwrap();
            }
            _ => unreachable!(),
        }
        let mut read = vec![0xff; size as usize];
        assert_eq!(fs.read(ino, 0, &mut read).await.unwrap(), size as usize, "{growth}");
        let past_cut =
            read[1000..(size as usize).min(4096)].iter().filter(|&&b| b != 0 && b != b'w').count();
        assert_eq!(past_cut, 0, "{growth}: bytes the cut left past the end, read back");
    }
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

/// A write through a DAX window whose extents are found sealed on every try gives up with the
/// seal's error, an I/O error's errno, as it says: not corruption's (TASK-160.21). It gave up
/// with `Inconsistent`, the seal's error lost.
#[fuchsia::test]
async fn a_dax_write_sealed_on_every_try_gives_up_with_the_seal() {
    let sealing = Arc::new(AtomicBool::new(false));
    let mut device = FakeDevice::new(16384, 4096);
    device.set_op_callback({
        let sealing = sealing.clone();
        move |op| match op {
            DeviceOp::Write if sealing.load(Ordering::SeqCst) => {
                Err(SealedExtent(ExtentId::new(0, 1)).into())
            }
            _ => Ok(()),
        }
    });
    let (fs, root) = volume_on(device).await;
    let ino = fs.new_file(root, b"a").await;
    // What the blocks hold is written as they move, in the transaction, where the seal is met.
    let held = vec![1; 4096];
    sealing.store(true, Ordering::SeqCst);
    let mapped = fs.dax_map_write(ino, 0..4096, 4096, false, Some(&held)).await;
    sealing.store(false, Ordering::SeqCst);
    let error = mapped.expect_err("mapped while sealed");
    assert!(error.chain().any(|cause| cause.is::<SealedExtent>()), "{error:#}");
    let tried = format!("its extents sealed still, tried {MAX_SEALED_RETRIES} times");
    assert!(format!("{error:#}").contains(&tried), "{error:#}");
    assert_eq!(errno(&error), Errno::EIO);
    fs.dax_map_write(ino, 0..4096, 4096, false, Some(&held)).await.expect("mapped once unsealed");
}

/// A file's DAX pieces of `range`, to compare.
async fn pieces(fs: &Fxfs<Tested>, ino: u64, range: Range<u64>) -> String {
    format!("{:?}", fs.dax_extents(ino, range).await.unwrap())
}

/// A file of 8192 bytes of `c`, in sealed extents, which a DAX write moves.
async fn sealed_file(fs: &Fxfs<Tested>, root: u64) -> u64 {
    let ino = fs.new_file(root, b"a").await;
    fs.write(ino, 0, &[b'c'; 8192]).await.unwrap();
    fs.sync().await.unwrap();
    ino
}

/// A DAX write's move, dropped before it commits, leaves the file as it was: its blocks, its bytes
/// and its size, which a move committed before the bytes lost to the new blocks' zeros (TASK-153.5).
#[fuchsia::test]
async fn a_dax_write_dropped_leaves_the_file_as_it_was() {
    let (fs, root) = volume().await;
    let ino = sealed_file(&fs, root).await;
    let before = pieces(&fs, ino, 0..8192).await;
    let (write, moved) = fs.dax_begin_write(ino, 0..12288, 12288).await.unwrap();
    assert_ne!(format!("{moved:?}"), before, "the blocks didn't move");
    assert_eq!(pieces(&fs, ino, 0..8192).await, before, "the move is seen uncommitted");
    drop(write);

    assert_eq!(pieces(&fs, ino, 0..8192).await, before);
    assert_eq!(fs.attr(ino).await.unwrap().size, 8192);
    let mut read = vec![0; 8192];
    assert_eq!(fs.read(ino, 0, &mut read).await.unwrap(), 8192);
    assert_eq!(read, [b'c'; 8192]);
    fs.stop_delayed().await.unwrap();
}

/// Committed, the file has the write's new blocks, holding what the first block held around the
/// write and nothing else of the old, and the size the write asked for.
#[fuchsia::test]
async fn a_dax_write_committed_has_its_blocks_and_size() {
    let (fs, root) = volume().await;
    let ino = sealed_file(&fs, root).await;
    let (write, moved) = fs.dax_begin_write(ino, 100..12288, 12288).await.unwrap();
    write.commit().await.unwrap();

    assert_eq!(pieces(&fs, ino, 0..12288).await, format!("{moved:?}"));
    assert_eq!(fs.attr(ino).await.unwrap().size, 12288);
    let mut read = vec![0xff; 12288];
    assert_eq!(fs.read(ino, 0, &mut read).await.unwrap(), 12288);
    let mut expected = vec![0; 12288];
    expected[..4096].fill(b'c');
    assert_eq!(read, expected);
    fs.stop_delayed().await.unwrap();
}

/// Until it ends, a DAX write holds the file's lock: a transaction of the file waits for it, and
/// then sees what it committed.
#[fuchsia::test]
async fn a_dax_write_holds_the_file_until_it_commits() {
    let (fs, root) = volume().await;
    let ino = sealed_file(&fs, root).await;
    let (write, _) = fs.dax_begin_write(ino, 0..4096, 8192).await.unwrap();
    let mut update = pin!(fs.dax_update_size(ino, 4096 * 3));
    assert!(futures::poll!(update.as_mut()).is_pending(), "a transaction of the file went on");
    write.commit().await.unwrap();
    update.await.unwrap();
    assert_eq!(fs.attr(ino).await.unwrap().size, 4096 * 3);
    fs.stop_delayed().await.unwrap();
}
