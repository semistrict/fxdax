//! Tests of a filesystem through its public API, as a VMM's virtio-fs device uses it.

use futures::executor::block_on;
use fxdax::{
    Access, DaxWindow, DirEntry, Errno, FileType, Filesystem, Inode, Owner, Protection,
    RenameFlags, Result, SecondStage, SetAttr, SetTime, Storage, VolatileSlice, VolatileSliceMut,
    kernel,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the filesystems the tests format.
const STORAGE_SIZE: u64 = 8 << 20;

async fn format() -> Filesystem {
    Filesystem::format(Storage::memory(STORAGE_SIZE)).await.expect("format failed")
}

/// Closes `fs`, checks the filesystem and returns its storage.
async fn close_and_check(fs: Filesystem) -> Storage {
    fs.close().await.expect("close failed").check().await.expect("check failed")
}

fn errno<T: std::fmt::Debug>(result: Result<T>) -> Errno {
    result.expect_err("succeeded")
}

fn names(entries: &[DirEntry]) -> Vec<(&[u8], FileType)> {
    entries.iter().map(|entry| (entry.name.as_slice(), entry.file_type)).collect()
}

async fn read_all(fs: &Filesystem, inode: Inode) -> Vec<u8> {
    let mut buf = vec![0; fs.getattr(inode).await.unwrap().size as usize + 16];
    let len = fs.read(inode, 0, &mut buf).await.expect("read failed");
    buf.truncate(len);
    buf
}

/// The time `secs` and `nanos` after the Unix epoch.
fn time(secs: u64, nanos: u32) -> SystemTime {
    UNIX_EPOCH + Duration::new(secs, nanos)
}

#[test]
fn root_is_a_directory() {
    block_on(async {
        let fs = format().await;
        let root = fs.getattr(Inode::ROOT).await.expect("getattr failed");
        assert_eq!(root.inode, Inode::ROOT);
        assert_eq!(root.file_type, FileType::Directory);
        assert_eq!(root.perm, 0o755);
        assert_eq!(root.mode(), 0o040755);
        close_and_check(fs).await;
    })
}

#[test]
fn create_then_lookup() {
    block_on(async {
        let fs = format().await;
        let created = fs.create(Inode::ROOT, b"file", 0o640, OWNER).await.expect("create failed");
        assert_eq!(created.file_type, FileType::File);
        assert_eq!(created.perm, 0o640);
        assert_eq!((created.uid, created.gid), (1000, 100));
        assert_eq!(created.size, 0);
        assert_eq!(created.nlink, 1);
        assert_ne!(created.inode, Inode::ROOT);

        let found = fs.lookup(Inode::ROOT, b"file").await.expect("lookup failed");
        assert_eq!(found, created);
        assert_eq!(errno(fs.create(Inode::ROOT, b"file", 0o640, OWNER).await), Errno::EEXIST);
        assert_eq!(errno(fs.lookup(Inode::ROOT, b"missing").await), Errno::ENOENT);
        close_and_check(fs).await;
    })
}

#[test]
fn write_then_read() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        assert_eq!(fs.write(file, 0, b"hello, world").await.expect("write failed"), 12);
        assert_eq!(fs.write(file, 7, b"fxdax").await.expect("write failed"), 5);
        assert_eq!(fs.getattr(file).await.unwrap().size, 12);

        let mut buf = [0; 32];
        assert_eq!(fs.read(file, 0, &mut buf).await.expect("read failed"), 12);
        assert_eq!(&buf[..12], b"hello, fxdax");
        assert_eq!(fs.read(file, 7, &mut buf[..3]).await.expect("read failed"), 3);
        assert_eq!(&buf[..3], b"fxd");
        assert_eq!(fs.read(file, 100, &mut buf).await.expect("read failed"), 0);
        assert_eq!(errno(fs.read(Inode::ROOT, 0, &mut buf).await), Errno::EISDIR);
        close_and_check(fs).await;
    })
}

#[test]
fn write_past_the_end_leaves_a_hole() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 100_000, b"end").await.expect("write failed");
        assert_eq!(fs.getattr(file).await.unwrap().size, 100_003);
        let contents = read_all(&fs, file).await;
        assert!(contents[..100_000].iter().all(|&byte| byte == 0));
        assert_eq!(&contents[100_000..], b"end");
        close_and_check(fs).await;
    })
}

#[test]
fn setattr_changes_size() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"0123456789").await.unwrap();
        let attr =
            fs.setattr(file, &SetAttr { size: Some(4), ..Default::default() }).await.unwrap();
        assert_eq!(attr.size, 4);
        assert_eq!(read_all(&fs, file).await, b"0123");
        let attr =
            fs.setattr(file, &SetAttr { size: Some(8), ..Default::default() }).await.unwrap();
        assert_eq!(attr.size, 8);
        assert_eq!(read_all(&fs, file).await, b"0123\0\0\0\0");
        assert_eq!(
            errno(fs.setattr(Inode::ROOT, &SetAttr { size: Some(0), ..Default::default() }).await),
            Errno::EISDIR
        );
        close_and_check(fs).await;
    })
}

#[test]
fn setattr_changes_owner_permissions_and_times() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        let attr = fs
            .setattr(
                file,
                &SetAttr {
                    perm: Some(0o4755),
                    uid: Some(7),
                    gid: Some(8),
                    atime: Some(SetTime::At(time(1_000, 5))),
                    mtime: Some(SetTime::At(time(2_000, 6))),
                    ..Default::default()
                },
            )
            .await
            .expect("setattr failed");
        assert_eq!(attr.perm, 0o4755);
        assert_eq!(attr.file_type, FileType::File);
        assert_eq!((attr.uid, attr.gid), (7, 8));
        assert_eq!(attr.atime, time(1_000, 5));
        assert_eq!(attr.mtime, time(2_000, 6));

        // Clearing setuid leaves setgid alone unless the file is group-executable.
        let attr =
            fs.setattr(file, &SetAttr { perm: Some(0o6745), ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o6745);
        let attr = fs
            .setattr(file, &SetAttr { kill_suid_sgid: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(attr.perm, 0o2745);
        let attr =
            fs.setattr(file, &SetAttr { perm: Some(0o6755), ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o6755);
        let attr = fs
            .setattr(file, &SetAttr { kill_suid_sgid: true, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(attr.perm, 0o0755);

        let before = SystemTime::now();
        let attr =
            fs.setattr(file, &SetAttr { mtime: Some(SetTime::Now), ..Default::default() }).await;
        assert!(attr.unwrap().mtime >= before);

        // Fxfs's times can't be before the epoch.
        let before_epoch = SetTime::At(UNIX_EPOCH - Duration::from_secs(1));
        let set = SetAttr { mtime: Some(before_epoch), ..Default::default() };
        assert_eq!(errno(fs.setattr(file, &set).await), Errno::EINVAL);
        close_and_check(fs).await;
    })
}

/// An open with `O_TRUNC` truncates the file with a setattr, clearing setuid as an unprivileged
/// truncation does.
#[test]
fn truncating_open() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o4755, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        let truncate = SetAttr { size: Some(0), kill_suid_sgid: true, ..Default::default() };
        let attr = fs.setattr(file, &truncate).await.expect("setattr failed");
        assert_eq!((attr.size, attr.perm), (0, 0o755));
        assert_eq!(read_all(&fs, file).await, b"");
        close_and_check(fs).await;
    })
}

#[test]
fn mkdir_and_rmdir() {
    block_on(async {
        let fs = format().await;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o750, OWNER).await.expect("mkdir failed");
        assert_eq!(dir.file_type, FileType::Directory);
        assert_eq!(dir.perm, 0o750);
        assert_eq!((dir.uid, dir.gid), (1000, 100));
        let file = fs.create(dir.inode, b"file", 0o644, OWNER).await.unwrap();

        assert_eq!(errno(fs.rmdir(Inode::ROOT, b"dir").await), Errno::ENOTEMPTY);
        assert_eq!(errno(fs.rmdir(dir.inode, b"file").await), Errno::ENOTDIR);
        fs.unlink(dir.inode, b"file").await.expect("unlink failed");
        fs.forget(file.inode, 1);
        fs.rmdir(Inode::ROOT, b"dir").await.expect("rmdir failed");
        assert_eq!(errno(fs.lookup(Inode::ROOT, b"dir").await), Errno::ENOENT);
        assert_eq!(errno(fs.rmdir(Inode::ROOT, b"dir").await), Errno::ENOENT);
        fs.forget(dir.inode, 1);
        close_and_check(fs).await;
    })
}

#[test]
fn unlink() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.unlink(Inode::ROOT, b"file").await.expect("unlink failed");
        assert_eq!(errno(fs.lookup(Inode::ROOT, b"file").await), Errno::ENOENT);
        assert_eq!(errno(fs.unlink(Inode::ROOT, b"file").await), Errno::ENOENT);

        // The kernel still holds a lookup on the file, which stays readable until it forgets it.
        assert_eq!(read_all(&fs, file).await, b"contents");
        assert_eq!(fs.getattr(file).await.unwrap().nlink, 0);
        fs.forget(file, 1);
        assert_eq!(errno(fs.getattr(file).await), Errno::ENOENT);
        close_and_check(fs).await;
    })
}

/// A file only looked up, with no connection to it yet, is kept too, and its connection opens
/// once it is unlinked.
#[test]
fn unlink_a_file_only_looked_up() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.forget(file, 1);
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        fs.unlink(Inode::ROOT, b"file").await.expect("unlink failed");

        assert_eq!(fs.getattr(file).await.unwrap().nlink, 0);
        assert_eq!(read_all(&fs, file).await, b"contents");
        fs.forget(file, 1);
        assert_eq!(errno(fs.getattr(file).await), Errno::ENOENT);
        close_and_check(fs).await;
    })
}

#[test]
fn rename_within_and_across_directories() {
    block_on(async {
        let fs = format().await;
        let a = fs.mkdir(Inode::ROOT, b"a", 0o755, OWNER).await.unwrap().inode;
        let b = fs.mkdir(Inode::ROOT, b"b", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(a, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();

        fs.rename(a, b"file", a, b"renamed", RenameFlags::empty()).await.expect("rename failed");
        assert_eq!(fs.lookup(a, b"renamed").await.unwrap().inode, file);
        fs.rename(a, b"renamed", b, b"moved", RenameFlags::empty()).await.expect("rename failed");
        assert_eq!(errno(fs.lookup(a, b"renamed").await), Errno::ENOENT);
        let moved = fs.lookup(b, b"moved").await.unwrap().inode;
        assert_eq!(moved, file);
        assert_eq!(read_all(&fs, moved).await, b"contents");
        close_and_check(fs).await;
    })
}

#[test]
fn rename_replaces_unless_told_not_to() {
    block_on(async {
        let fs = format().await;
        let first = fs.create(Inode::ROOT, b"first", 0o644, OWNER).await.unwrap().inode;
        let second = fs.create(Inode::ROOT, b"second", 0o644, OWNER).await.unwrap().inode;
        fs.write(first, 0, b"first").await.unwrap();
        fs.write(second, 0, b"second").await.unwrap();

        assert_eq!(
            errno(
                fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"second", RenameFlags::NOREPLACE)
                    .await
            ),
            Errno::EEXIST
        );
        assert_eq!(
            errno(
                fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"second", RenameFlags::EXCHANGE)
                    .await
            ),
            Errno::EINVAL
        );
        fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"third", RenameFlags::NOREPLACE)
            .await
            .expect("rename failed");
        fs.rename(Inode::ROOT, b"third", Inode::ROOT, b"second", RenameFlags::empty())
            .await
            .expect("rename failed");
        let replaced = fs.lookup(Inode::ROOT, b"second").await.unwrap().inode;
        assert_eq!(replaced, first);
        assert_eq!(read_all(&fs, replaced).await, b"first");
        close_and_check(fs).await;
    })
}

#[test]
fn renamed_directory_has_its_new_parent() {
    block_on(async {
        let fs = format().await;
        let a = fs.mkdir(Inode::ROOT, b"a", 0o755, OWNER).await.unwrap().inode;
        let sub = fs.mkdir(a, b"sub", 0o755, OWNER).await.unwrap().inode;
        fs.rename(a, b"sub", Inode::ROOT, b"sub", RenameFlags::empty())
            .await
            .expect("rename failed");
        let handle = fs.opendir(sub).await.expect("opendir failed");
        let entries = fs.readdir(handle, 0).expect("readdir failed");
        assert_eq!(
            entries[..],
            [
                DirEntry { inode: sub, file_type: FileType::Directory, name: b".".to_vec() },
                DirEntry {
                    inode: Inode::ROOT,
                    file_type: FileType::Directory,
                    name: b"..".to_vec()
                },
            ]
        );
        fs.releasedir(handle);
        close_and_check(fs).await;
    })
}

#[test]
fn symlink_and_readlink() {
    block_on(async {
        let fs = format().await;
        let link = fs.symlink(Inode::ROOT, b"link", b"some/target").await.expect("symlink failed");
        assert_eq!(link.file_type, FileType::Symlink);
        assert_eq!(link.size, 11);
        assert_eq!(fs.readlink(link.inode).await.expect("readlink failed"), b"some/target");
        assert_eq!(fs.lookup(Inode::ROOT, b"link").await.unwrap().file_type, FileType::Symlink);
        assert_eq!(errno(fs.readlink(Inode::ROOT).await), Errno::EINVAL);
        assert_eq!(errno(fs.symlink(Inode::ROOT, b"link", b"other").await), Errno::EEXIST);
        close_and_check(fs).await;
    })
}

#[test]
fn link() {
    block_on(async {
        let fs = format().await;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();

        let linked = fs.link(file, dir, b"link").await.expect("link failed");
        assert_eq!(linked.inode, file);
        assert_eq!(linked.nlink, 2);
        assert_eq!(fs.lookup(dir, b"link").await.unwrap().inode, file);
        fs.unlink(Inode::ROOT, b"file").await.unwrap();
        assert_eq!(fs.getattr(file).await.unwrap().nlink, 1);
        assert_eq!(read_all(&fs, file).await, b"contents");

        let symlink = fs.symlink(Inode::ROOT, b"symlink", b"target").await.unwrap().inode;
        assert_eq!(fs.link(symlink, dir, b"symlink").await.unwrap().nlink, 2);
        assert_eq!(errno(fs.link(dir, Inode::ROOT, b"dir-link").await), Errno::EPERM);
        close_and_check(fs).await;
    })
}

#[test]
fn mknod_makes_special_files() {
    block_on(async {
        let fs = format().await;
        let fifo = fs
            .mknod(Inode::ROOT, b"fifo", FileType::Fifo, 0o640, 0, OWNER)
            .await
            .expect("mknod failed");
        assert_eq!(fifo.file_type, FileType::Fifo);
        assert_eq!(fifo.perm, 0o640);
        let device = fs
            .mknod(Inode::ROOT, b"device", FileType::CharDevice, 0o600, 0x0103, OWNER)
            .await
            .unwrap();
        assert_eq!(device.file_type, FileType::CharDevice);
        assert_eq!(device.rdev, 0x0103);
        let socket =
            fs.mknod(Inode::ROOT, b"socket", FileType::Socket, 0o755, 0, OWNER).await.unwrap();
        assert_eq!(socket.file_type, FileType::Socket);
        let file = fs.mknod(Inode::ROOT, b"file", FileType::File, 0o600, 0, OWNER).await.unwrap();
        assert_eq!(file.file_type, FileType::File);
        assert_eq!(
            errno(fs.mknod(Inode::ROOT, b"dir", FileType::Directory, 0o755, 0, OWNER).await),
            Errno::EINVAL
        );

        assert_eq!(fs.lookup(Inode::ROOT, b"fifo").await.unwrap().file_type, FileType::Fifo);
        assert_eq!(fs.lookup(Inode::ROOT, b"device").await.unwrap().rdev, 0x0103);
        close_and_check(fs).await;
    })
}

#[test]
fn readdir() {
    block_on(async {
        let fs = format().await;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        let link = fs.symlink(Inode::ROOT, b"link", b"file").await.unwrap().inode;

        let handle = fs.opendir(Inode::ROOT).await.expect("opendir failed");
        let entries = fs.readdir(handle, 0).expect("readdir failed");
        assert_eq!(
            names(&entries),
            [
                (&b"."[..], FileType::Directory),
                (b"..", FileType::Directory),
                (b"dir", FileType::Directory),
                (b"file", FileType::File),
                (b"link", FileType::Symlink),
            ]
        );
        assert_eq!(
            entries.iter().map(|entry| entry.inode).collect::<Vec<_>>(),
            [Inode::ROOT, Inode::ROOT, dir, file, link]
        );
        assert_eq!(
            names(&fs.readdir(handle, 3).unwrap()),
            [(&b"file"[..], FileType::File), (b"link", FileType::Symlink)]
        );
        assert_eq!(fs.readdir(handle, 5).unwrap().len(), 0);

        // An open directory lists what it had when opened.
        fs.create(Inode::ROOT, b"later", 0o644, OWNER).await.unwrap();
        assert_eq!(fs.readdir(handle, 0).unwrap().len(), 5);
        fs.releasedir(handle);
        assert_eq!(errno(fs.readdir(handle, 0)), Errno::EBADF);
        assert_eq!(errno(fs.opendir(file).await), Errno::ENOTDIR);
        close_and_check(fs).await;
    })
}

#[test]
fn readdir_of_many_entries() {
    block_on(async {
        let fs = format().await;
        let names: Vec<Vec<u8>> =
            (0..500).map(|i| format!("file-with-a-long-name-{i:04}").into_bytes()).collect();
        for name in &names {
            fs.create(Inode::ROOT, name, 0o644, OWNER).await.unwrap();
        }
        let handle = fs.opendir(Inode::ROOT).await.unwrap();
        let entries = fs.readdir(handle, 2).unwrap();
        assert_eq!(entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), names);
        fs.releasedir(handle);
        close_and_check(fs).await;
    })
}

#[test]
fn allocate() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.allocate(file, 0, 65536).await.expect("allocate failed");
        let attr = fs.getattr(file).await.unwrap();
        assert_eq!(attr.size, 65536);
        assert_eq!(attr.blocks, 128);
        assert_eq!(errno(fs.allocate(Inode::ROOT, 0, 4096).await), Errno::EISDIR);
        close_and_check(fs).await;
    })
}

#[test]
fn fsync() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.fsync(file).await.expect("fsync failed");
        fs.fsync(Inode::ROOT).await.expect("fsync failed");
        close_and_check(fs).await;
    })
}

#[test]
fn statfs() {
    block_on(async {
        let fs = format().await;
        let before = fs.statfs().await.expect("statfs failed");
        assert_eq!(before.block_size, 4096);
        assert_eq!(before.blocks, STORAGE_SIZE / 4096);
        assert_eq!(before.name_max, 255);
        assert!(before.blocks_free < before.blocks);

        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, &vec![0xaa; 1 << 20]).await.unwrap();
        fs.fsync(file).await.unwrap();
        let after = fs.statfs().await.unwrap();
        assert!(after.blocks_free + 256 <= before.blocks_free, "{after:?} vs {before:?}");
        close_and_check(fs).await;
    })
}

#[test]
fn contents_survive_remount() {
    block_on(async {
        let fs = format().await;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(dir, b"file", 0o600, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"persistent").await.unwrap();
        fs.symlink(dir, b"link", b"file").await.unwrap();
        fs.mknod(dir, b"fifo", FileType::Fifo, 0o644, 0, OWNER).await.unwrap();
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let dir = fs.lookup(Inode::ROOT, b"dir").await.unwrap().inode;
        let file = fs.lookup(dir, b"file").await.unwrap();
        assert_eq!((file.perm, file.uid, file.gid), (0o600, 1000, 100));
        assert_eq!(read_all(&fs, file.inode).await, b"persistent");
        let link = fs.lookup(dir, b"link").await.unwrap().inode;
        assert_eq!(fs.readlink(link).await.unwrap(), b"file");
        assert_eq!(fs.lookup(dir, b"fifo").await.unwrap().file_type, FileType::Fifo);
        close_and_check(fs).await;
    })
}

#[test]
fn contents_survive_remount_of_an_image() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image");
        let storage = Storage::create(&path, 64 << 20).expect("create failed");
        assert_eq!(storage.size(), 64 << 20);
        assert!(Storage::create(&path, 64 << 20).is_err(), "created an image over another");

        let fs = Filesystem::format(storage).await.expect("format failed");
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"on disk").await.unwrap();
        // Dropping the filesystem unmounts it.
        drop(fs);
        let storage =
            Storage::open(&path).expect("open failed").check().await.expect("check failed");

        // The storage a filesystem gives back can be mounted again.
        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let storage = fs.close().await.expect("close failed");
        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        assert_eq!(read_all(&fs, file).await, b"on disk");
        fs.unmount().await.expect("unmount failed");
        // Unmounting again does nothing.
        fs.unmount().await.expect("unmount failed");
        assert_eq!(errno(fs.getattr(file).await), Errno::ENOTCONN);
        assert_eq!(errno(fs.lookup(Inode::ROOT, b"file").await), Errno::ENOTCONN);
        // The unmounted filesystem keeps its storage for close.
        let storage = fs.close().await.expect("close failed");
        storage.check().await.expect("check failed");
    })
}

#[test]
fn storage_errors() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let missing =
            Storage::open(dir.path().join("missing")).expect_err("opened a missing image");
        assert!(format!("{missing:#}").contains("missing"), "{missing:#}");
        let empty =
            Storage::create(dir.path().join("empty"), 0).expect_err("created an empty image");
        assert_eq!(empty.to_string(), "the image is smaller than a block");
        assert!(!dir.path().join("empty").exists(), "left an empty image behind");
        let unformatted = Filesystem::mount(Storage::memory(STORAGE_SIZE)).await;
        assert!(unformatted.is_err(), "mounted unformatted storage");
    })
}

#[test]
fn vectored_io() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        let written = fs
            .write_vectored(
                file,
                3,
                &[
                    VolatileSlice::from(&b"hello"[..]),
                    VolatileSlice::from(&b""[..]),
                    VolatileSlice::from(&b", world"[..]),
                ],
            )
            .await
            .expect("write_vectored failed");
        assert_eq!(written, 12);
        assert_eq!(read_all(&fs, file).await, b"\0\0\0hello, world");

        let (mut first, mut second) = ([0; 4], [0; 32]);
        let read = fs
            .read_vectored(
                file,
                1,
                &mut [
                    VolatileSliceMut::from(&mut first[..]),
                    VolatileSliceMut::from(&mut second[..]),
                ],
            )
            .await
            .expect("read_vectored failed");
        assert_eq!(read, 14);
        assert_eq!(&first, b"\0\0he");
        assert_eq!(&second[..10], b"llo, world");
        assert_eq!(
            errno(fs.write_vectored(Inode::ROOT, 0, &[VolatileSlice::from(&b"x"[..])]).await),
            Errno::EISDIR
        );
        close_and_check(fs).await;
    })
}

#[test]
fn readdir_entries_from_an_offset_past_the_end() {
    block_on(async {
        let fs = format().await;
        let handle = fs.opendir(Inode::ROOT).await.unwrap();
        assert_eq!(fs.readdir(handle, 2).unwrap().len(), 0);
        assert_eq!(fs.readdir(handle, u64::MAX).unwrap().len(), 0);
        fs.releasedir(handle);
        close_and_check(fs).await;
    })
}

#[test]
fn errno_names() {
    block_on(async {
        assert_eq!(Errno::ENOENT.raw(), 2);
        assert_eq!(Errno::from_raw(39), Errno::ENOTEMPTY);
        assert_eq!(Errno::ENOENT.to_string(), "ENOENT (errno 2)");
        assert_eq!(format!("{:?}", Errno::from_raw(4000)), "Errno(4000)");
        assert_eq!(fxdax::Error::from(Errno::EIO).errno(), Some(Errno::EIO));
    })
}

#[test]
fn file_types() {
    block_on(async {
        for (file_type, mode, dirent) in [
            (FileType::File, 0o100644, 8),
            (FileType::Directory, 0o040755, 4),
            (FileType::Symlink, 0o120777, 10),
            (FileType::Fifo, 0o010600, 1),
            (FileType::CharDevice, 0o020600, 2),
            (FileType::BlockDevice, 0o060600, 6),
            (FileType::Socket, 0o140755, 12),
        ] {
            assert_eq!(FileType::from_mode(mode), Some(file_type));
            assert_eq!(file_type.mode_bits() | (mode & 0o7777), mode);
            assert_eq!(file_type.dirent_type(), dirent);
        }
        assert_eq!(FileType::from_mode(0o644), None);
    })
}

/// Operations run on any executor: here tokio's, on one thread and on several, where the
/// filesystem is also dropped, which unmounts it.
#[test]
fn operations_run_on_tokio_runtimes() {
    let runtimes = [
        tokio::runtime::Builder::new_current_thread().build().unwrap(),
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap(),
    ];
    for runtime in runtimes {
        runtime.block_on(async {
            let fs = format().await;
            let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
            fs.write(file, 0, b"tokio").await.unwrap();
            assert_eq!(read_all(&fs, file).await, b"tokio");
            drop(fs);
        });
    }
}

/// Many operations wait at once on one thread, each completing as Fxfs completes it.
#[test]
fn concurrent_operations_on_one_thread() {
    block_on(async {
        // Fxfs reserves space for each write in flight, so many at once need more than the others.
        let fs = &Filesystem::format(Storage::memory(64 << 20)).await.unwrap();
        let names: Vec<Vec<u8>> = (0..32).map(|i| format!("file-{i}").into_bytes()).collect();
        let files: Vec<Inode> = futures::future::join_all(
            names.iter().map(|name| fs.create(Inode::ROOT, name, 0o644, OWNER)),
        )
        .await
        .into_iter()
        .map(|attr| attr.unwrap().inode)
        .collect();
        let contents = |i: usize| vec![i as u8; 3 * kernel::page_size() as usize];
        let written = futures::future::join_all(
            files
                .iter()
                .enumerate()
                .map(|(i, &file)| async move { fs.write(file, 0, &contents(i)).await.unwrap() }),
        )
        .await;
        assert!(written.iter().all(|&len| len == 3 * kernel::page_size() as usize));
        fs.fsync(Inode::ROOT).await.unwrap();
        let read = futures::future::join_all(files.iter().map(|&file| read_all(fs, file))).await;
        assert_eq!(read, (0..32).map(contents).collect::<Vec<_>>());
    })
}

/// The futures of the operations are `Send`, so that they may run on any executor's threads.
#[test]
fn futures_are_send() {
    fn send<T: Send>(_: T) {}
    let fs = block_on(format());
    let mut buf = [0u8; 1];
    let mut bufs = [VolatileSliceMut::from(&mut buf[..])];
    send(Filesystem::format(Storage::memory(STORAGE_SIZE)));
    send(Storage::memory(STORAGE_SIZE).check());
    send(fs.lookup(Inode::ROOT, b"file"));
    send(fs.getattr(Inode::ROOT));
    send(fs.setattr(Inode::ROOT, &SetAttr::default()));
    send(fs.create(Inode::ROOT, b"file", 0o644, OWNER));
    send(fs.rename(Inode::ROOT, b"a", Inode::ROOT, b"b", RenameFlags::empty()));
    send(fs.read_vectored(Inode::ROOT, 0, &mut bufs));
    send(fs.write_vectored(Inode::ROOT, 0, &[VolatileSlice::from(&b"x"[..])]));
    send(fs.opendir(Inode::ROOT));
    send(fs.setup_mapping(Inode::ROOT, 0, 0, 0, false));
    let window = fs.create_dax_window(kernel::page_size(), None).unwrap();
    send(window.fault(0, Access::Read));
    send(fs.unmount());
    block_on(close_and_check(fs));
}

/// `len` bytes of `window` at `offset`, as the guest sees them. The pages must have been faulted
/// in.
#[allow(clippy::mut_from_ref)]
fn window_bytes(window: &DaxWindow, offset: u64, len: usize) -> &mut [u8] {
    // SAFETY: The caller faulted the pages in, and nothing else accesses them while the test does.
    unsafe { std::slice::from_raw_parts_mut(window.host_base().add(offset as usize), len) }
}

/// A file of `len` bytes, each its offset modulo 251.
async fn patterned_file(fs: &Filesystem, name: &[u8], len: usize) -> Inode {
    let file = fs.create(Inode::ROOT, name, 0o644, OWNER).await.unwrap().inode;
    let contents: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    fs.write(file, 0, &contents).await.unwrap();
    file
}

#[test]
fn dax_mapping_reads_the_file() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", 2 * ps as usize).await;
        let window = fs.create_dax_window(8 * ps, None).expect("create_dax_window failed");
        assert_eq!(window.size(), 8 * ps);

        fs.setup_mapping(file, 0, 2 * ps, ps, false).await.expect("setup_mapping failed");
        assert!(!window.is_mapped(0));
        assert!(window.is_mapped(ps));
        assert!(window.is_mapped(2 * ps));
        assert!(!window.is_mapped(3 * ps));

        window.fault(2 * ps, Access::Read).await.expect("fault failed");
        let expected: Vec<u8> = (ps as usize..2 * ps as usize).map(|i| (i % 251) as u8).collect();
        assert_eq!(window_bytes(&window, 2 * ps, ps as usize), &expected[..]);

        // The mapping is read-only.
        assert_eq!(errno(window.fault(ps, Access::Write).await), Errno::EACCES);
        close_and_check(fs).await;
    })
}

#[test]
fn dax_stores_reach_disk_on_fsync() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", 2 * ps as usize).await;
        // Write the file back, so that the store through the window is what dirties its page.
        fs.fsync(file).await.unwrap();
        let window = fs.create_dax_window(4 * ps, None).unwrap();
        fs.setup_mapping(file, 0, 2 * ps, 0, true).await.expect("setup_mapping failed");

        window.fault(ps, Access::Write).await.expect("fault failed");
        window_bytes(&window, ps + 10, 25).copy_from_slice(b"stored through the window");
        // The file's stream sees the store, as it shares the file's pages.
        let mut buf = [0; 25];
        fs.read(file, ps + 10, &mut buf).await.unwrap();
        assert_eq!(&buf, b"stored through the window");

        fs.fsync(file).await.expect("fsync failed");
        fs.remove_mapping(0, 2 * ps).expect("remove_mapping failed");
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        let contents = read_all(&fs, file).await;
        let mut expected: Vec<u8> = (0..2 * ps as usize).map(|i| (i % 251) as u8).collect();
        expected[ps as usize + 10..ps as usize + 35].copy_from_slice(b"stored through the window");
        assert_eq!(contents, expected);
        close_and_check(fs).await;
    })
}

#[test]
fn dax_mapping_is_upgraded_to_writable() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", ps as usize).await;
        let window = fs.create_dax_window(4 * ps, None).unwrap();
        fs.setup_mapping(file, 0, ps, 0, false).await.unwrap();
        window.fault(0, Access::Read).await.unwrap();
        assert_eq!(errno(window.fault(0, Access::Write).await), Errno::EACCES);

        // As Linux's FUSE client does on the first store to a range it mapped read-only.
        fs.setup_mapping(file, 0, ps, 0, true).await.expect("setup_mapping failed");
        window.fault(0, Access::Write).await.expect("fault failed");
        window_bytes(&window, 0, 5).copy_from_slice(b"hello");
        let mut buf = [0; 5];
        fs.read(file, 0, &mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        close_and_check(fs).await;
    })
}

#[test]
fn dax_remove_mapping() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", 4 * ps as usize).await;
        let window = fs.create_dax_window(4 * ps, None).unwrap();
        fs.setup_mapping(file, 0, 4 * ps, 0, true).await.unwrap();

        // Removing part of a mapping leaves the rest.
        fs.remove_mapping(ps, 2 * ps).expect("remove_mapping failed");
        assert!(window.is_mapped(0));
        assert!(!window.is_mapped(ps));
        assert!(!window.is_mapped(2 * ps));
        assert!(window.is_mapped(3 * ps));
        assert_eq!(errno(window.fault(ps, Access::Read).await), Errno::ENOENT);
        window.fault(3 * ps, Access::Write).await.expect("fault failed");

        fs.remove_mapping(0, 4 * ps).expect("remove_mapping failed");
        assert!(!window.is_mapped(0));
        assert!(!window.is_mapped(3 * ps));
        close_and_check(fs).await;
    })
}

#[test]
fn dax_past_the_end_of_the_file() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let size = ps + 100;
        let file = patterned_file(&fs, b"file", size as usize).await;
        let window = fs.create_dax_window(4 * ps, None).unwrap();
        // As Linux's FUSE client maps whole 2 MiB ranges, which extend past files' ends.
        fs.setup_mapping(file, 0, 3 * ps, 0, true).await.unwrap();

        // Past the end, the guest reads zeros.
        window.fault(2 * ps, Access::Read).await.expect("fault failed");
        assert!(window_bytes(&window, 2 * ps, ps as usize).iter().all(|&byte| byte == 0));
        window.fault(ps, Access::Read).await.expect("fault failed");
        assert_eq!(window_bytes(&window, ps + 99, 1), [((ps + 99) % 251) as u8]);
        assert!(window_bytes(&window, ps + 100, ps as usize - 100).iter().all(|&byte| byte == 0));

        // A store past the end in the last page doesn't reach the file.
        window.fault(ps, Access::Write).await.expect("fault failed");
        window_bytes(&window, ps + 200, 4).copy_from_slice(b"lost");
        fs.fsync(file).await.unwrap();
        assert_eq!(fs.getattr(file).await.unwrap().size, size);
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        assert_eq!(fs.getattr(file).await.unwrap().size, size);
        fs.setattr(file, &SetAttr { size: Some(2 * ps), ..Default::default() }).await.unwrap();
        let mut buf = [0xff; 4];
        fs.read(file, ps + 200, &mut buf).await.unwrap();
        assert_eq!(buf, [0; 4]);
        close_and_check(fs).await;
    })
}

#[test]
fn dax_mapping_keeps_an_unlinked_file() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", ps as usize).await;
        let window = fs.create_dax_window(ps, None).unwrap();
        fs.setup_mapping(file, 0, ps, 0, false).await.unwrap();
        fs.unlink(Inode::ROOT, b"file").await.unwrap();
        fs.forget(file, 1);

        window.fault(0, Access::Read).await.expect("fault failed");
        assert_eq!(window_bytes(&window, 0, 3), [0, 1, 2]);
        fs.remove_mapping(0, ps).unwrap();
        close_and_check(fs).await;
    })
}

/// The errno a window creation failed with.
fn create_errno(result: std::result::Result<DaxWindow, fxdax::Error>) -> Option<Errno> {
    result.expect_err("succeeded").errno()
}

#[test]
fn dax_errors() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", ps as usize).await;
        assert_eq!(errno(fs.setup_mapping(file, 0, ps, 0, false).await), Errno::ENOSYS);
        assert_eq!(errno(fs.remove_mapping(0, ps)), Errno::ENOSYS);
        assert_eq!(create_errno(fs.create_dax_window(ps + 1, None)), Some(Errno::EINVAL));
        assert_eq!(create_errno(fs.create_dax_window(0, None)), Some(Errno::EINVAL));

        fs.create_dax_window(4 * ps, None).unwrap();
        assert_eq!(create_errno(fs.create_dax_window(4 * ps, None)), Some(Errno::EEXIST));
        assert_eq!(errno(fs.setup_mapping(file, 0, ps, 1, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.setup_mapping(file, 1, ps, 0, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.setup_mapping(file, 0, ps + 1, 0, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.setup_mapping(file, 0, 0, 0, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.setup_mapping(file, 0, ps, 4 * ps, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.setup_mapping(Inode::ROOT, 0, ps, 0, false).await), Errno::EINVAL);
        assert_eq!(errno(fs.remove_mapping(0, 5 * ps)), Errno::EINVAL);
        close_and_check(fs).await;
    })
}

/// Unmounting unmaps the window's files, so that the guest's faults there fail.
#[test]
fn dax_unmount_unmaps_the_window() {
    block_on(async {
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", ps as usize).await;
        let window = fs.create_dax_window(ps, None).unwrap();
        fs.setup_mapping(file, 0, ps, 0, true).await.unwrap();
        window.fault(0, Access::Write).await.unwrap();
        window_bytes(&window, 0, 4).copy_from_slice(b"dirt");

        fs.unmount().await.unwrap();
        assert!(!window.is_mapped(0));
        assert_eq!(errno(window.fault(0, Access::Read).await), Errno::ENOENT);
        assert_eq!(errno(fs.setup_mapping(file, 0, ps, 0, false).await), Errno::ENOTCONN);
        let storage = fs.close().await.unwrap();

        // The store was written back.
        let fs = Filesystem::mount(storage.check().await.unwrap()).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        assert_eq!(&read_all(&fs, file).await[..5], b"dirt\x04");
        close_and_check(fs).await;
    })
}

/// A second stage recording the changes made to it, with maps and changes of protection page by
/// page, however many pages each covers.
#[derive(Default)]
struct RecordingSecondStage(std::sync::Mutex<Vec<SecondStageOp>>);

#[derive(Debug, PartialEq, Eq)]
enum SecondStageOp {
    Map { offset: u64, host_addr: usize, len: u64, protection: Protection },
    Protect { offset: u64, len: u64, protection: Protection },
    Unmap { offset: u64, len: u64 },
}

impl RecordingSecondStage {
    fn take(&self) -> Vec<SecondStageOp> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl SecondStage for RecordingSecondStage {
    fn map(
        &self,
        offset: u64,
        host_addr: *mut u8,
        _phys_offset: u64,
        len: u64,
        protection: Protection,
    ) {
        let ps = kernel::page_size();
        let mut ops = self.0.lock().unwrap();
        for page in (0..len).step_by(ps as usize) {
            let (offset, host_addr) = (offset + page, host_addr as usize + page as usize);
            ops.push(SecondStageOp::Map { offset, host_addr, len: ps, protection });
        }
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        let ps = kernel::page_size();
        let mut ops = self.0.lock().unwrap();
        for page in (0..len).step_by(ps as usize) {
            ops.push(SecondStageOp::Protect { offset: offset + page, len: ps, protection });
        }
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.0.lock().unwrap().push(SecondStageOp::Unmap { offset, len });
    }
}

#[test]
fn dax_second_stage_follows_the_window() {
    block_on(async {
        use Protection::{Read, ReadWrite};
        use SecondStageOp::*;
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", 2 * ps as usize).await;
        fs.fsync(file).await.unwrap();
        let second_stage = Arc::new(RecordingSecondStage::default());
        let window = fs.create_dax_window(4 * ps, Some(second_stage.clone())).unwrap();
        let map = |offset: u64, protection| Map {
            offset,
            host_addr: window.host_base() as usize + offset as usize,
            len: ps,
            protection,
        };

        // Mapping a file maps the pages it has in memory, read-only, so that loads do not fault and
        // the first store to each dirties it.
        fs.setup_mapping(file, 0, 2 * ps, ps, true).await.unwrap();
        assert_eq!(second_stage.take(), [map(ps, Read), map(2 * ps, Read)]);
        window.fault(ps, Access::Write).await.unwrap();
        assert_eq!(second_stage.take(), [Protect { offset: ps, len: ps, protection: ReadWrite }]);
        window.fault(2 * ps, Access::Write).await.unwrap();
        assert_eq!(
            second_stage.take(),
            [Protect { offset: 2 * ps, len: ps, protection: ReadWrite }]
        );

        // Writing the dirty pages back write-protects them, so that the next store dirties them again.
        window_bytes(&window, ps, 5).copy_from_slice(b"first");
        window_bytes(&window, 2 * ps, 6).copy_from_slice(b"second");
        fs.fsync(file).await.unwrap();
        assert_eq!(
            second_stage.take(),
            [
                Protect { offset: ps, len: ps, protection: Read },
                Protect { offset: 2 * ps, len: ps, protection: Read }
            ]
        );

        fs.remove_mapping(ps, 2 * ps).unwrap();
        assert_eq!(second_stage.take(), [Unmap { offset: ps, len: 2 * ps }]);
        close_and_check(fs).await;
    })
}

/// A store fault that follows on from the last dirties the pages after it too, where a sequential
/// writer stores next; one that does not, its own page alone.
#[test]
fn dax_sequential_stores_dirty_runs_of_pages() {
    block_on(async {
        use Protection::ReadWrite;
        use SecondStageOp::*;
        let fs = format().await;
        let ps = kernel::page_size();
        let file = patterned_file(&fs, b"file", 32 * ps as usize).await;
        fs.fsync(file).await.unwrap();
        let second_stage = Arc::new(RecordingSecondStage::default());
        let window = fs.create_dax_window(32 * ps, Some(second_stage.clone())).unwrap();
        fs.setup_mapping(file, 0, 32 * ps, 0, true).await.unwrap();
        second_stage.take();
        let dirtied = |pages: std::ops::Range<u64>| -> Vec<SecondStageOp> {
            pages
                .map(|page| Protect { offset: page * ps, len: ps, protection: ReadWrite })
                .collect()
        };

        window.fault(0, Access::Write).await.unwrap();
        assert_eq!(second_stage.take(), dirtied(0..1));
        window.fault(ps, Access::Write).await.unwrap();
        assert_eq!(second_stage.take(), dirtied(1..17));
        window.fault(20 * ps, Access::Write).await.unwrap();
        assert_eq!(second_stage.take(), dirtied(20..21));
        // A run ends with the file's mapping.
        window.fault(21 * ps, Access::Write).await.unwrap();
        assert_eq!(second_stage.take(), dirtied(21..32));

        fs.remove_mapping(0, 32 * ps).unwrap();
        close_and_check(fs).await;
    })
}

/// Anonymous memory a hypervisor follows, such as the memory backing a KVM memory slot.
#[cfg(target_os = "linux")]
struct HostMemory {
    base: std::ptr::NonNull<u8>,
    size: usize,
}

#[cfg(target_os = "linux")]
impl HostMemory {
    fn new(size: usize) -> Self {
        // SAFETY: Maps anonymous memory, which this owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        Self { base: std::ptr::NonNull::new(memory.cast()).unwrap(), size }
    }

    /// `len` bytes at `offset`, whose faults the filesystem serves.
    #[allow(clippy::mut_from_ref)]
    fn bytes(&self, offset: usize, len: usize) -> &mut [u8] {
        assert!(offset + len <= self.size);
        // SAFETY: The range is within the memory, which only the test accesses.
        unsafe { std::slice::from_raw_parts_mut(self.base.as_ptr().add(offset), len) }
    }
}

#[cfg(target_os = "linux")]
impl Drop for HostMemory {
    fn drop(&mut self) {
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(self.base.as_ptr().cast(), self.size) }, 0);
    }
}

/// A window in host memory serves the faults on the memory, here taken by the host, as KVM takes
/// them for a guest.
#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_serves_faults() {
    block_on(async {
        let memory = HostMemory::new(4 * kernel::page_size() as usize);
        let fs = format().await;
        let ps = kernel::page_size() as usize;
        let file = patterned_file(&fs, b"file", 2 * ps).await;
        fs.fsync(file).await.unwrap();
        // SAFETY: Only the test uses the memory, which outlives the filesystem.
        let window =
            unsafe { fs.create_dax_window_in_host_memory(memory.base, memory.size as u64, None) }
                .expect("create_dax_window_in_host_memory failed");
        assert_eq!(window.size(), memory.size as u64);
        fs.setup_mapping(file, 0, 2 * ps as u64, ps as u64, true).await.unwrap();

        // Loads fault the file's pages in, and stores to its clean pages fault to dirty them, before
        // and after writeback cleans them again.
        let mut expected: Vec<u8> = (0..2 * ps).map(|i| (i % 251) as u8).collect();
        assert_eq!(memory.bytes(ps, 2 * ps), &expected[..]);
        memory.bytes(ps + 10, 5).copy_from_slice(b"first");
        fs.fsync(file).await.unwrap();
        memory.bytes(ps + 20, 5).copy_from_slice(b"again");
        memory.bytes(2 * ps, 6).copy_from_slice(b"second");
        expected[10..15].copy_from_slice(b"first");
        expected[20..25].copy_from_slice(b"again");
        expected[ps..ps + 6].copy_from_slice(b"second");
        assert_eq!(read_all(&fs, file).await, expected);
        fs.fsync(file).await.unwrap();

        fs.remove_mapping(ps as u64, 2 * ps as u64).unwrap();
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        assert_eq!(read_all(&fs, file).await, expected);
        close_and_check(fs).await;
    })
}

/// A write the window refuses, to a page a read-only mapping maps, is resolved as a read, as KVM's
/// asynchronous page faults fault pages in for writing whatever the guest's access. Here the kernel
/// writes into the memory, as KVM's worker does.
#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_resolves_a_refused_write_as_a_read() {
    block_on(async {
        use std::os::fd::AsRawFd;
        let ps = kernel::page_size() as usize;
        let memory = HostMemory::new(ps);
        let fs = format().await;
        let file = patterned_file(&fs, b"file", ps).await;
        // SAFETY: Only the test uses the memory, which outlives the filesystem.
        unsafe { fs.create_dax_window_in_host_memory(memory.base, ps as u64, None) }.unwrap();
        fs.setup_mapping(file, 0, ps as u64, 0, false).await.unwrap();

        // The kernel's write into the page faults it in, and then fails.
        let zero = std::fs::File::open("/dev/zero").unwrap();
        // SAFETY: Reads into the memory, whose faults the filesystem serves.
        let read = unsafe { libc::read(zero.as_raw_fd(), memory.base.as_ptr().cast(), 1) };
        assert_eq!(read, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));
        let expected: Vec<u8> = (0..ps).map(|i| (i % 251) as u8).collect();
        assert_eq!(memory.bytes(0, ps), &expected[..]);

        fs.remove_mapping(0, ps as u64).unwrap();
        close_and_check(fs).await;
    })
}

/// A hypervisor that maps the window's host memory for the guest, recording the pages it is told
/// the window maps there.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct RecordingPrefault(std::sync::Mutex<Vec<u64>>);

#[cfg(target_os = "linux")]
impl fxdax::Prefault for RecordingPrefault {
    fn prefault(&self, offset: u64, len: u64) {
        let ps = kernel::page_size();
        self.0.lock().unwrap().extend((offset..offset + len).step_by(ps as usize));
    }
}

/// The hypervisor is told of the pages the window maps in its host memory, so that it can map them
/// for the guest before the guest touches them.
#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_tells_the_hypervisor_what_it_maps() {
    block_on(async {
        let ps = kernel::page_size() as usize;
        let memory = HostMemory::new(4 * ps);
        let fs = format().await;
        let file = patterned_file(&fs, b"file", 2 * ps).await;
        fs.fsync(file).await.unwrap();
        let prefault = Arc::new(RecordingPrefault::default());
        // SAFETY: Only the test uses the memory, which outlives the filesystem.
        unsafe {
            fs.create_dax_window_in_host_memory(
                memory.base,
                memory.size as u64,
                Some(prefault.clone()),
            )
        }
        .unwrap();

        fs.setup_mapping(file, 0, 2 * ps as u64, ps as u64, false).await.unwrap();
        assert_eq!(*prefault.0.lock().unwrap(), [ps as u64, 2 * ps as u64]);

        fs.remove_mapping(ps as u64, 2 * ps as u64).unwrap();
        close_and_check(fs).await;
    })
}

#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_must_be_whole_pages() {
    block_on(async {
        let ps = kernel::page_size() as usize;
        let memory = HostMemory::new(2 * ps);
        let fs = format().await;
        // SAFETY: Fails before using the memory.
        let result =
            unsafe { fs.create_dax_window_in_host_memory(memory.base, ps as u64 + 1, None) };
        assert_eq!(
            result.expect_err("succeeded").to_string(),
            "DAX window memory must be whole pages"
        );
        close_and_check(fs).await;
    })
}
