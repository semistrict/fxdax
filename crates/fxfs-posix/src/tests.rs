// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the queued unlinks of doc-6 (TASK-79.21), on a fake device: what the kernel's calls find
//! of them. Unlinks are queued as the guest's VFS queues them, which nothing on the host does. And
//! what else a VFS's calls find: extended attributes made at once (TASK-199), a volume's read-only.

use super::*;
use fxfs::filesystem::FxFilesystemBuilder;
use fxfs::lock_keys;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

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

    /// Whether an unlink changing `ino` is queued.
    fn queued_for(&self, ino: u64) -> bool {
        self.delayed.queue.lock().unwrap().pending.contains_key(&ino)
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

    /// Queues the unlink of `name`, which links `ino`, from `dir`, as `unlink_later` does, but
    /// waking no worker: it stays queued until something waits for it.
    fn queue_unlink(&self, dir: u64, name: &str, ino: u64) {
        let op = DelayedOp::Unlink { dir, name: name.to_owned(), ino };
        self.delayed.queue.lock().unwrap().push(op);
    }
}

/// fxdax: a node's mode, owner and group are changed one at a time as together: the others keep
/// what they were. A new volume's root directory has no POSIX attributes of its own, whose
/// defaults its attributes give, and which Fxfs makes of the fields a change gives, the rest 0: a
/// chown of it alone left it mode 000, which a FUSE mount of a new store refused its owner
/// (TASK-222). So for the root, and for a file that has them.
#[fuchsia::test]
async fn mode_owner_and_group_change_one_at_a_time() {
    let changes = [
        (SetAttr { valid: SET_MODE, mode: S_IFDIR | 0o700, ..SetAttr::default() }, (0o700, 0, 0)),
        (SetAttr { valid: SET_UID, uid: 7, ..SetAttr::default() }, (0o755, 7, 0)),
        (SetAttr { valid: SET_GID, gid: 8, ..SetAttr::default() }, (0o755, 0, 8)),
    ];
    for (change, (perm, uid, gid)) in changes {
        let (fs, root) = volume().await;
        let before = fs.attr(root).await.unwrap();
        assert_eq!((before.mode, before.uid, before.gid), (S_IFDIR | 0o755, 0, 0));
        fs.setattr(root, &change).await.unwrap();
        let after = fs.attr(root).await.unwrap();
        assert_eq!((after.mode, after.uid, after.gid), (S_IFDIR | perm, uid, gid), "{change:?}");

        let file = fs.create(root, b"f", S_IFREG | 0o755, 0, 0, 0, &[]).await.unwrap()[0].ino;
        let change = SetAttr { mode: S_IFREG | perm, ..change };
        fs.setattr(file, &change).await.unwrap();
        let after = fs.attr(file).await.unwrap();
        assert_eq!((after.mode, after.uid, after.gid), (S_IFREG | perm, uid, gid), "{change:?}");
        fs.stop_delayed().await.unwrap();
    }
}

/// A readdir lists FIFOs, sockets and devices as what they are, as their attributes say: their
/// entries say only that they are files, and a FIFO was listed as a regular file (TASK-149.36).
#[fuchsia::test]
async fn readdir_lists_special_files_as_what_they_are() {
    let (fs, root) = volume().await;
    let kinds = [S_IFREG, S_IFIFO, S_IFSOCK, S_IFCHR, S_IFBLK, S_IFDIR];
    for (name, kind) in (b'a'..).zip(kinds) {
        let attr = fs.create(root, &[name], kind | 0o600, 7, 0, 0, &[]).await.expect("creating");
        assert_eq!(attr[0].mode & S_IFMT, kind);
    }
    let mut listed = Vec::new();
    let read = fs.readdir(root, ReaddirStart::Index(0), |name, _, kind| {
        listed.push((name.to_owned(), kind));
        true
    });
    read.await.expect("readdir failed");
    let expected: Vec<(String, u32)> =
        (b'a'..).zip(kinds).map(|(name, kind)| (char::from(name).to_string(), kind)).collect();
    assert_eq!(listed, expected);
    fs.stop_delayed().await.unwrap();
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

/// A lookup applies no unlink queued of the directory's other names: with the kernel's dentries
/// cold, each unlinkat's lookup applied the unlink before it, alone (TASK-92.26). It finds no name
/// queued to go, though the name's record is there until the unlink is made.
#[fuchsia::test]
async fn a_lookup_leaves_the_unlinks_of_other_names_queued() {
    let (fs, root) = volume().await;
    let (a, b) = (fs.new_file(root, b"a").await, fs.new_file(root, b"b").await);
    fs.queue_unlink(root, "a", a);
    assert_eq!(fs.lookup(root, b"b").await.unwrap().ino, b);
    assert_eq!(fs.queued(), 1, "a lookup of another name applied the unlink queued");
    let found = fs.lookup(root, b"a").await.expect_err("a name queued to go, found");
    assert_eq!(errno(&found), Errno::ENOENT);
    assert_eq!(fs.queued(), 0);
    fs.stop_delayed().await.unwrap();
}

/// Nor one another case of it names, in a casefolding directory, which looks the name up as it is
/// in neither.
#[fuchsia::test]
async fn a_lookup_finds_no_name_queued_to_go_in_another_case() {
    let (fs, root) = volume().await;
    let dir = fs.new_dir(root, b"folded").await;
    fs.directory(dir).await.unwrap().set_casefold(true).await.unwrap();
    let ino = fs.new_file(dir, b"Name").await;
    assert_eq!(fs.lookup(dir, b"name").await.unwrap().ino, ino);
    fs.queue_unlink(dir, "Name", ino);
    let found = fs.lookup(dir, b"name").await.expect_err("a name queued to go, found");
    assert_eq!(errno(&found), Errno::ENOENT);
    fs.stop_delayed().await.unwrap();
}

/// A drain of what is queued, an fsync's or a sync's, applies what was queued before it began,
/// not what is queued while it goes on: another's bulk delete held an fsync for as long as it went
/// on (TASK-92.27).
#[fuchsia::test]
async fn a_drain_leaves_what_is_queued_after_it_began() {
    let (fs, root) = volume().await;
    let (a, b) = (fs.new_file(root, b"a").await, fs.new_file(root, b"b").await);
    fs.queue_unlink(root, "a", a);
    // The batch unlinking `a` waits for the object's truncate guard (`apply_unlinks`).
    let guard = fs.filesystem.truncate_guard(fs.volume.store_object_id(), a).await;
    let mut drain = pin!(fs.drain_all());
    assert!(futures::poll!(drain.as_mut()).is_pending(), "the batch didn't wait");
    fs.queue_unlink(root, "b", b);
    drop(guard);
    drain.await.unwrap();
    assert_eq!(fs.queued(), 1, "the drain applied what was queued after it began");
    fs.stop_delayed().await.unwrap();
    assert_eq!(fs.queued(), 0);
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
    let keys = lock_keys![LockKey::object(fs.volume.store_object_id(), ino)];
    let mut transaction = fs.volume.new_transaction(keys, Options::default()).await.unwrap();
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

/// fxdax: a writeback paid with the room the page cache reserved for the pages it dirtied, growing
/// the file past an end within a block that a cut left as it was (SET_KEEP_TAIL, a DAX file's),
/// rewrites that block too, zeroed past the end (`txn_grow_size`), which no page of it paid for: it
/// failed NoSpace, on a filesystem all but empty (TASK-149.29, found by crates/fxfs-sim-diff).
#[fuchsia::test]
async fn a_writeback_paid_with_dirty_room_has_room_for_the_tail_its_growth_zeroes() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"grown").await;
    fs.write(ino, 0, &[b'c'; 8192]).await.unwrap();
    fs.setattr(ino, &set_size(1000, SET_KEEP_TAIL)).await.unwrap();
    fs.reserve_dirty(ino, 4096).unwrap();
    fs.write_dirty(ino, &mut [(8192, &[b'w'; 4096][..])], 4096).await.unwrap();
    let mut read = vec![0xff; 12288];
    assert_eq!(fs.read(ino, 0, &mut read).await.unwrap(), 12288);
    assert_eq!((&read[..1000], &read[1000..8192], &read[8192..]), (
        &[b'c'; 1000][..],
        &[0; 7192][..],
        &[b'w'; 4096][..]
    ));
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

/// fxdax: an extended attribute set or removed is in Fxfs at once, in a transaction of its own,
/// as ext4's setxattr journals it: no writeback of the inode (`write_inode`) is waited for, which
/// lost a change it failed to write behind fsync's 0 (TASK-199). A new object's, inline or not, are
/// made in its creation's transaction.
#[fuchsia::test]
async fn xattrs_are_made_at_once() {
    use SetExtendedAttributeMode::{Create, Replace, Set};
    let device = DeviceHolder::new(FakeDevice::new(16384, 4096));
    let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
    let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = volumes.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
    let root = volume.root_directory_object_id();
    let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
    let large = vec![7; 8192];
    let inherited: &[(&[u8], &[u8])] = &[(b"user.inherited", &large), (b"user.inline", b"i")];
    let ino = fs.create(root, b"f", FILE, 0, 0, 0, inherited).await.unwrap()[0].ino;
    fs.set_xattr(ino, b"user.small", b"s", Set).await.unwrap();
    fs.set_xattr(ino, b"user.large", &large, Set).await.unwrap();
    fs.set_xattr(ino, b"user.small", b"t", Replace).await.unwrap();
    fs.set_xattr(ino, b"user.gone", b"g", Create).await.unwrap();
    fs.remove_xattr(ino, b"user.gone").await.unwrap();
    let exists = fs.set_xattr(ino, b"user.small", b"u", Create).await.unwrap_err();
    assert!(FxfsError::AlreadyExists.matches(&exists), "{exists:#}");
    let missing = fs.set_xattr(ino, b"user.none", b"n", Replace).await.unwrap_err();
    assert!(FxfsError::NotFound.matches(&missing), "{missing:#}");
    let removed = fs.remove_xattr(ino, b"user.gone").await.unwrap_err();
    assert!(FxfsError::NotFound.matches(&removed), "{removed:#}");
    fs.sync().await.unwrap();
    // Closed with no inode written back, as an unmount evicts none: each change is made already.
    fs.stop_delayed().await.unwrap();
    fs.close().await;
    drop(volumes);
    filesystem.close().await.expect("closing");
    let device = filesystem.take_device().await;
    device.reopen(false);

    let filesystem = FxFilesystemBuilder::new().open(device).await.expect("opening");
    let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = volumes.volume("test", StoreOptions::default()).await.unwrap();
    let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
    let mut names = fs.list_xattrs(ino).await.unwrap();
    names.sort();
    let expected: [&[u8]; 4] = [b"user.inherited", b"user.inline", b"user.large", b"user.small"];
    assert_eq!(names, expected.map(<[u8]>::to_vec));
    assert_eq!(fs.get_xattr(ino, b"user.inherited").await.unwrap(), large);
    assert_eq!(fs.get_xattr(ino, b"user.inline").await.unwrap(), b"i");
    assert_eq!(fs.get_xattr(ino, b"user.large").await.unwrap(), large);
    assert_eq!(fs.get_xattr(ino, b"user.small").await.unwrap(), b"t");
    fs.stop_delayed().await.unwrap();
    fs.close().await;
    drop(volumes);
    filesystem.close().await.expect("closing");
}

/// fxdax: a full filesystem refuses an extended attribute it has no room for (NoSpace), as ext4
/// does, where one kept in memory was lost as its writeback failed; what it refused it hasn't; and
/// it removes one, which frees what it held, as an unlink does (TASK-199).
#[fuchsia::test]
async fn a_full_filesystem_refuses_xattrs_and_removes_them() {
    let (fs, root) = volume().await;
    let ino = fs.new_file(root, b"attributed").await;
    let large = vec![7; 32768];
    fs.set_xattr(ino, b"user.large", &large, SetExtendedAttributeMode::Set).await.unwrap();
    let filler = fs.new_file(root, b"filler").await;
    let mut at = 0;
    for len in [1 << 20, 4096] {
        loop {
            match fs.write(filler, at, &vec![1; len]).await {
                Ok(()) => at += len as u64,
                Err(error) if FxfsError::NoSpace.matches(&error) => break,
                Err(error) => panic!("filling: {error:#}"),
            }
        }
    }
    // Small ones, which take only the journal's room, until there is none.
    let mode = SetExtendedAttributeMode::Set;
    let name = |n: usize| format!("user.small{n}").into_bytes();
    let mut refused = None;
    for n in 0..100_000 {
        if let Err(error) = fs.set_xattr(ino, &name(n), b"s", mode).await {
            refused = Some((n, error));
            break;
        }
    }
    let (refused, error) = refused.expect("a full filesystem took 100,000 extended attributes");
    assert!(FxfsError::NoSpace.matches(&error), "{error:#}");
    let missing = fs.get_xattr(ino, &name(refused)).await.unwrap_err();
    assert!(FxfsError::NotFound.matches(&missing), "{missing:#}");
    fs.remove_xattr(ino, b"user.large").await.unwrap();
    let names = fs.list_xattrs(ino).await.unwrap();
    assert_eq!(names.len(), refused, "{refused} set, then user.large removed");
    fs.stop_delayed().await.unwrap();
}

/// A volume is read-only if mounted so, or if its filesystem is, whatever its mount says: the
/// filesystem owns read-only, and its volumes derive theirs from it (TASK-160.26). The guest's
/// `flush_volume` of a volume mounted -o ro on a writable filesystem checked the filesystem's.
#[fuchsia::test]
async fn a_volume_is_read_only_as_it_or_its_filesystem_is() {
    let device = DeviceHolder::new(FakeDevice::new(16384, 4096));
    let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
    let root = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
    let mounted = |read_only| Fxfs::new((*filesystem).clone(), volume.clone(), read_only, Tested);
    assert_eq!([mounted(false).read_only(), mounted(true).read_only()], [false, true]);
    drop((volume, root));
    filesystem.close().await.expect("closing");
    let device = filesystem.take_device().await;
    device.reopen(true);

    let filesystem =
        FxFilesystemBuilder::new().read_only(true).open(device).await.expect("opening");
    let root = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root.volume("test", StoreOptions::default()).await.unwrap();
    let mounted = |read_only| Fxfs::new((*filesystem).clone(), volume.clone(), read_only, Tested);
    assert_eq!([mounted(false).read_only(), mounted(true).read_only()], [true, true]);
    drop((volume, root));
    filesystem.close().await.expect("closing");
}

/// Every operation of the table (`Op::rules`), as `call` asks it.
const EVERY_OP: [Op; 24] = [
    Op::Lookup,
    Op::Readdir,
    Op::AttrByHandle,
    Op::Create,
    Op::Symlink,
    Op::Tmpfile,
    Op::Link,
    Op::Unlink,
    Op::UnlinkLater,
    Op::Rename,
    Op::Exchange,
    Op::Clone,
    Op::Punch,
    Op::Allocate,
    Op::Write,
    Op::WriteDirty,
    Op::Cut,
    Op::Grow,
    Op::SetAttr,
    Op::SetXattr,
    Op::RemoveXattr,
    Op::Sync,
    Op::Fsync,
    Op::Statfs,
];

/// What every operation of the table's tests finds: in the root, `q` and `q2` link the file `q`;
/// `f` and `g` are files of a page each, `g` with the extended attribute `user.b`; `d` a
/// directory, and `e` one with the file `h` in it. Nothing is queued.
struct Objects {
    fs: Fxfs<Tested>,
    root: u64,
    q: u64,
    f: u64,
    g: u64,
    d: u64,
    e: u64,
    h: u64,
}

async fn objects() -> Objects {
    let (fs, root) = volume().await;
    let q = fs.new_file(root, b"q").await;
    fs.link(q, root, b"q2", false).await.unwrap();
    let (f, g) = (fs.new_file(root, b"f").await, fs.new_file(root, b"g").await);
    for ino in [f, g] {
        fs.write(ino, 0, &[b'a'; 4096]).await.unwrap();
    }
    fs.set_xattr(g, b"user.b", b"b", SetExtendedAttributeMode::Set).await.unwrap();
    let (d, e) = (fs.new_dir(root, b"d").await, fs.new_dir(root, b"e").await);
    let h = fs.new_file(e, b"h").await;
    Objects { fs, root, q, f, g, d, e, h }
}

/// Asks `op` of `objects` as a VFS would, naming `q`, or the root, where it names an inode.
async fn call(objects: &Objects, op: Op) -> Result<(), Error> {
    let Objects { ref fs, root, q, f, g, d, .. } = *objects;
    match op {
        Op::Lookup => fs.lookup(root, b"q2").await.map(drop),
        Op::Readdir => fs.readdir(root, ReaddirStart::Index(0), |_, _, _| true).await,
        Op::AttrByHandle => fs.attr_by_handle(q).await.map(drop),
        Op::Create => fs.create(d, b"new", FILE, 0, 0, 0, &[]).await.map(drop),
        Op::Symlink => fs.symlink(d, b"link", b"f", 0, 0).await.map(drop),
        Op::Tmpfile => fs.tmpfile(d, FILE, 0, 0).await.map(drop),
        Op::Link => fs.link(f, d, b"f", false).await.map(drop),
        Op::Unlink => fs.unlink(root, b"q2", q).await.map(drop),
        Op::UnlinkLater => fs.unlink_later(root, b"f", f).await,
        Op::Rename => fs.rename(root, b"f", f, root, b"q2", Some(q)).await.map(drop),
        Op::Exchange => fs.exchange(root, b"f", root, b"g").await.map(drop),
        Op::Clone => fs.clone_range(f, 0..4096, g, 4096, 8192).await.map(drop),
        Op::Punch => fs.punch(f, 0..4096).await,
        Op::Allocate => fs.allocate(g, 4096..8192).await,
        Op::Write => fs.write(f, 0, b"w").await,
        Op::WriteDirty => {
            fs.reserve_dirty(f, 4096)?;
            fs.write_dirty(f, &mut [(0, &[b'w'; 4096][..])], 4096).await
        }
        Op::Cut => fs.setattr(f, &set_size(1, 0)).await,
        Op::Grow => fs.setattr(f, &set_size(8192, 0)).await,
        Op::SetAttr => {
            fs.setattr(f, &SetAttr { valid: SET_MODE, mode: FILE, ..SetAttr::default() }).await
        }
        Op::SetXattr => fs.set_xattr(f, b"user.a", b"a", SetExtendedAttributeMode::Set).await,
        Op::RemoveXattr => fs.remove_xattr(g, b"user.b").await,
        Op::Sync => fs.sync().await,
        Op::Fsync => fs.fsync().await,
        Op::Statfs => fs.statfs().await.map(drop),
    }
}

/// Whether `op` applied the unlink of a name in a directory, linking an inode, that `unlink` says
/// of `objects`, queued before it.
async fn applies(op: Op, unlink: fn(&Objects) -> (u64, &'static str, u64)) -> bool {
    let objects = objects().await;
    let (dir, name, ino) = unlink(&objects);
    objects.fs.queue_unlink(dir, name, ino);
    call(&objects, op).await.unwrap_or_else(|e| panic!("{op:?}: {e:#}"));
    let applied = !objects.fs.queued_for(ino);
    objects.fs.stop_delayed().await.unwrap();
    applied
}

/// Each operation comes after the queued unlinks its row says (`After`), the TASK-79.21 cases among
/// them: those of what it names, which it reads (a lookup's link count, a directory's names), and
/// every one queued before it, if it changes names, makes them durable or counts what they free.
/// Not those it needn't: each applied alone, under its directory's lock (TASK-92.26).
#[fuchsia::test]
async fn each_operation_comes_after_the_unlinks_its_row_says() {
    let mut wrong = Vec::new();
    for op in EVERY_OP {
        // An unlink of a name of the inode it names, and of one no operation names.
        let of_named = applies(op, |objects| (objects.root, "q", objects.q)).await;
        let of_another = applies(op, |objects| (objects.e, "h", objects.h)).await;
        let after = op.rules().after;
        if (of_named, of_another) != (after != After::Nothing, after == After::All) {
            wrong.push(format!("{op:?}, {after:?}: applied {of_named} named, {of_another} other"));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// Once a batch of queued unlinks failed, Fxfs may hold names the kernel doesn't: every change a VFS
/// asks fails, until the filesystem remounts; what reads, or makes durable what's there, works.
/// The page cache's writeback refused leaves the room it reserved as one that fails does.
#[fuchsia::test]
async fn every_change_fails_once_a_batch_failed() {
    let mut refused = Vec::new();
    for op in EVERY_OP {
        let objects = objects().await;
        objects.fs.delayed.queue.lock().unwrap().failed = true;
        match call(&objects, op).await {
            Ok(()) => {}
            Err(error) => {
                assert_eq!(errno(&error), Errno::EIO, "{op:?}: {error:#}");
                refused.push(op);
            }
        }
        assert_eq!(objects.fs.dirty.dirty_bytes(objects.f), 0, "{op:?}: dirty room left");
        objects.fs.stop_delayed().await.unwrap();
    }
    let reads = [Op::Lookup, Op::Readdir, Op::AttrByHandle, Op::Sync, Op::Fsync, Op::Statfs];
    let changes: Vec<Op> = EVERY_OP.into_iter().filter(|op| !reads.contains(op)).collect();
    assert_eq!(refused, changes);
}

/// fxdax: a platform whose syncs' durability is counted, as the guest's device has it: commits
/// made while a sync wants durability (`Platform::durable`) are durable, the others ordered alone.
struct Durably(Arc<std::sync::atomic::AtomicUsize>);

/// While it lives, a sync wants the device's commits durable.
struct WantsDurable(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for WantsDurable {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Platform for Durably {
    const KEEPS_DIRECTORY_TIMES: bool = false;

    type Durable = WantsDurable;

    fn durable(&self) -> WantsDurable {
        self.0.fetch_add(1, Ordering::SeqCst);
        WantsDurable(self.0.clone())
    }

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        fasync::Task::spawn(future).detach();
    }
}

/// fxdax: a sync ends with a durable commit of the device, whatever the journal had flushed: an
/// fsync's flush of it is ordered alone, and a sync after it, finding the journal flushed as far,
/// asked the device nothing, and returned with the fsync's commit not yet durable, which a lost
/// host lost (fxfs-guest-soak, TASK-67: syncfs returned for files a lost host's successor didn't
/// have).
#[fuchsia::test]
async fn syncs_make_what_fsyncs_ordered_durable() {
    let wanted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let durable_barriers = Arc::new(AtomicU64::new(0));
    let mut device = FakeDevice::new(16384, 4096);
    let (counted, counting) = (wanted.clone(), durable_barriers.clone());
    device.set_op_callback(move |op| {
        let barrier = matches!(op, storage_device::fake_device::Op::Flush)
            || matches!(op, storage_device::fake_device::Op::PostBarrier);
        if barrier && counted.load(Ordering::SeqCst) > 0 {
            counting.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    });
    let filesystem = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("formatting");
    let root_volume = root_volume((*filesystem).clone()).await.expect("the root volume");
    let volume = root_volume.new_volume("test", NewChildStoreOptions::default()).await.unwrap();
    let root = volume.root_directory_object_id();
    let fs = Fxfs::new((*filesystem).clone(), volume, false, Durably(wanted));
    for round in 0..3u8 {
        let name = format!("file-{round}");
        let ino = fs.create(root, name.as_bytes(), FILE, 0, 0, 0, &[]).await.unwrap()[0].ino;
        fs.write(ino, 0, &[round; 8192]).await.unwrap();
        fs.fsync().await.unwrap();
        let before = durable_barriers.load(Ordering::SeqCst);
        fs.sync().await.unwrap();
        assert!(
            durable_barriers.load(Ordering::SeqCst) > before,
            "round {round}: the sync after an fsync made nothing durable"
        );
    }
}
