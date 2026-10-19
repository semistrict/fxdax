// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of a filesystem through its public API, as a host uses it.

use futures::executor::block_on;
use fxdax::{
    DirEntry, Errno, FileType, Filesystem, Inode, Owner, RenameFlags, Result, SetAttr, SetTime,
    Storage, VolatileSlice, VolatileSliceMut, XattrMode, kernel,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the filesystems the tests format.
const STORAGE_SIZE: u64 = 8 << 20;

async fn format() -> Filesystem {
    Filesystem::format(Storage::memory(STORAGE_SIZE)).await.expect("format failed")
}

/// Runs `test` on a new filesystem, then closes and checks it.
fn with_fs(test: impl AsyncFnOnce(&Filesystem)) {
    block_on(async {
        let fs = format().await;
        test(&fs).await;
        close_and_check(fs).await;
    })
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
    with_fs(async |fs| {
        let root = fs.getattr(Inode::ROOT).await.expect("getattr failed");
        assert_eq!(root.inode, Inode::ROOT);
        assert_eq!(root.file_type, FileType::Directory);
        assert_eq!(root.perm, 0o755);
        assert_eq!(root.mode(), 0o040755);
    })
}

#[test]
fn create_then_lookup() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn write_then_read() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn write_past_the_end_leaves_a_hole() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 100_000, b"end").await.expect("write failed");
        assert_eq!(fs.getattr(file).await.unwrap().size, 100_003);
        let contents = read_all(&fs, file).await;
        assert!(contents[..100_000].iter().all(|&byte| byte == 0));
        assert_eq!(&contents[100_000..], b"end");
    })
}

#[test]
fn setattr_changes_size() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn setattr_changes_owner_permissions_and_times() {
    with_fs(async |fs| {
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
    })
}

/// An open with `O_TRUNC` truncates the file with a setattr, clearing setuid as an unprivileged
/// truncation does.
#[test]
fn truncating_open() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o4755, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        let truncate = SetAttr { size: Some(0), kill_suid_sgid: true, ..Default::default() };
        let attr = fs.setattr(file, &truncate).await.expect("setattr failed");
        assert_eq!((attr.size, attr.perm), (0, 0o755));
        assert_eq!(read_all(&fs, file).await, b"");
    })
}

#[test]
fn mkdir_and_rmdir() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn unlink() {
    with_fs(async |fs| {
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
    })
}

/// A link takes a lookup on the file, as a create does: the kernel forgets each.
#[test]
fn links_take_lookups() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.link(file, Inode::ROOT, b"other").await.expect("link failed");
        fs.unlink(Inode::ROOT, b"file").await.unwrap();
        fs.unlink(Inode::ROOT, b"other").await.unwrap();
        fs.forget(file, 1);
        assert_eq!(read_all(&fs, file).await, b"contents");
        fs.forget(file, 1);
        assert_eq!(errno(fs.getattr(file).await), Errno::ENOENT);
    })
}

/// A file only looked up, with no connection to it yet, is kept too, and its connection opens
/// once it is unlinked.
#[test]
fn unlink_a_file_only_looked_up() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.forget(file, 1);
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        fs.unlink(Inode::ROOT, b"file").await.expect("unlink failed");

        assert_eq!(fs.getattr(file).await.unwrap().nlink, 0);
        assert_eq!(read_all(&fs, file).await, b"contents");
        fs.forget(file, 1);
        assert_eq!(errno(fs.getattr(file).await), Errno::ENOENT);
    })
}

#[test]
fn rename_within_and_across_directories() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn rename_replaces_unless_told_not_to() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn renamed_directory_has_its_new_parent() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn symlink_and_readlink() {
    with_fs(async |fs| {
        let link =
            fs.symlink(Inode::ROOT, b"link", b"some/target", OWNER).await.expect("symlink failed");
        assert_eq!(link.file_type, FileType::Symlink);
        assert_eq!(link.size, 11);
        assert_eq!(fs.readlink(link.inode).await.expect("readlink failed"), b"some/target");
        assert_eq!(fs.lookup(Inode::ROOT, b"link").await.unwrap().file_type, FileType::Symlink);
        assert_eq!(errno(fs.readlink(Inode::ROOT).await), Errno::EINVAL);
        assert_eq!(errno(fs.symlink(Inode::ROOT, b"link", b"other", OWNER).await), Errno::EEXIST);
    })
}

#[test]
fn symlinks_have_owners_and_times() {
    block_on(async {
        let fs = format().await;
        let link = fs.symlink(Inode::ROOT, b"link", b"target", OWNER).await.unwrap();
        assert_eq!((link.uid, link.gid, link.perm), (OWNER.uid, OWNER.gid, 0o777));
        let set = SetAttr {
            uid: Some(7),
            atime: Some(SetTime::At(time(1_000, 1))),
            mtime: Some(SetTime::At(time(2_000, 2))),
            ..Default::default()
        };
        let changed = fs.setattr(link.inode, &set).await.expect("setattr failed");
        assert_eq!((changed.uid, changed.gid), (7, OWNER.gid));
        assert_eq!((changed.atime, changed.mtime), (time(1_000, 1), time(2_000, 2)));
        assert_eq!(changed.file_type, FileType::Symlink);
        // A symlink's permissions are always all of them.
        let chmod = SetAttr { perm: Some(0o600), ..Default::default() };
        assert_eq!(fs.setattr(link.inode, &chmod).await.expect("chmod failed").perm, 0o777);
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let link = fs.lookup(Inode::ROOT, b"link").await.unwrap();
        assert_eq!((link.uid, link.gid, link.perm), (7, OWNER.gid, 0o777));
        assert_eq!(link.mtime, time(2_000, 2));
        assert_eq!(fs.readlink(link.inode).await.unwrap(), b"target");
        close_and_check(fs).await;
    })
}

#[test]
fn extended_attributes() {
    block_on(async {
        let fs = format().await;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let link = fs.symlink(Inode::ROOT, b"link", b"file", OWNER).await.unwrap().inode;
        for (inode, value) in [(file, &b"of a file"[..]), (dir, b"of a dir"), (link, b"of a link")]
        {
            fs.setxattr(inode, b"user.what", value, XattrMode::Set).await.expect("setxattr failed");
            assert_eq!(fs.getxattr(inode, b"user.what").await.unwrap(), value);
        }
        fs.setxattr(file, b"security.capability", &[1, 0, 0, 2], XattrMode::Create).await.unwrap();
        assert_eq!(
            errno(fs.setxattr(file, b"user.what", b"again", XattrMode::Create).await),
            Errno::EEXIST
        );
        assert_eq!(
            errno(fs.setxattr(file, b"user.none", b"x", XattrMode::Replace).await),
            Errno::ENODATA
        );
        fs.setxattr(file, b"user.what", b"replaced", XattrMode::Replace).await.unwrap();
        let mut names = fs.listxattr(file).await.expect("listxattr failed");
        names.sort();
        assert_eq!(names, [b"security.capability".to_vec(), b"user.what".to_vec()]);
        assert_eq!(errno(fs.getxattr(file, b"user.none").await), Errno::ENODATA);
        fs.removexattr(dir, b"user.what").await.expect("removexattr failed");
        assert_eq!(errno(fs.removexattr(dir, b"user.what").await), Errno::ENODATA);
        assert_eq!(fs.listxattr(dir).await.unwrap(), Vec::<Vec<u8>>::new());
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        assert_eq!(fs.getxattr(file, b"user.what").await.unwrap(), b"replaced");
        assert_eq!(fs.getxattr(file, b"security.capability").await.unwrap(), [1, 0, 0, 2]);
        let link = fs.lookup(Inode::ROOT, b"link").await.unwrap().inode;
        assert_eq!(fs.getxattr(link, b"user.what").await.unwrap(), b"of a link");
        close_and_check(fs).await;
    })
}

#[test]
fn link() {
    with_fs(async |fs| {
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

        let symlink = fs.symlink(Inode::ROOT, b"symlink", b"target", OWNER).await.unwrap().inode;
        assert_eq!(fs.link(symlink, dir, b"symlink").await.unwrap().nlink, 2);
        assert_eq!(errno(fs.link(dir, Inode::ROOT, b"dir-link").await), Errno::EPERM);
    })
}

#[test]
fn mknod_makes_special_files() {
    with_fs(async |fs| {
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
    })
}

/// Each open of a directory is a handle of its own, listing the entries as they were then.
#[test]
fn directories_opened_twice_are_two_handles() {
    with_fs(async |fs| {
        let first = fs.opendir(Inode::ROOT).await.unwrap();
        fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap();
        let second = fs.opendir(Inode::ROOT).await.unwrap();
        assert_ne!(first, second);
        assert_eq!(fs.readdir(first, 0).unwrap().len(), 2);
        assert_eq!(fs.readdir(second, 0).unwrap().len(), 3);
    })
}

/// A directory's `..` is its parent, as it was made, or renamed into; its siblings' stay.
#[test]
fn dot_dot_is_the_parent() {
    with_fs(async |fs| {
        let outer = fs.mkdir(Inode::ROOT, b"outer", 0o755, OWNER).await.unwrap().inode;
        let inner = fs.mkdir(outer, b"inner", 0o755, OWNER).await.unwrap().inode;
        let other = fs.mkdir(Inode::ROOT, b"other", 0o755, OWNER).await.unwrap().inode;
        let sibling = fs.mkdir(outer, b"sibling", 0o755, OWNER).await.unwrap().inode;
        let dot_dot = async |inode| {
            let handle = fs.opendir(inode).await.expect("opendir failed");
            let entries = fs.readdir(handle, 0).expect("readdir failed");
            let entry = entries.iter().find(|entry| entry.name == b"..").unwrap();
            entry.inode
        };
        assert_eq!(dot_dot(inner).await, outer);
        fs.rename(outer, b"inner", other, b"moved", RenameFlags::empty()).await.unwrap();
        assert_eq!(dot_dot(inner).await, other);
        // Only the directory renamed moves.
        assert_eq!(dot_dot(sibling).await, outer);
    })
}

#[test]
fn readdir() {
    with_fs(async |fs| {
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        let link = fs.symlink(Inode::ROOT, b"link", b"file", OWNER).await.unwrap().inode;

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
    })
}

#[test]
fn readdir_of_many_entries() {
    with_fs(async |fs| {
        let names: Vec<Vec<u8>> =
            (0..500).map(|i| format!("file-with-a-long-name-{i:04}").into_bytes()).collect();
        for name in &names {
            fs.create(Inode::ROOT, name, 0o644, OWNER).await.unwrap();
        }
        let handle = fs.opendir(Inode::ROOT).await.unwrap();
        let entries = fs.readdir(handle, 2).unwrap();
        assert_eq!(entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), names);
        fs.releasedir(handle);
    })
}

#[test]
fn allocate() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.allocate(file, 0, 65536).await.expect("allocate failed");
        let attr = fs.getattr(file).await.unwrap();
        assert_eq!(attr.size, 65536);
        assert_eq!(attr.blocks, 128);
        assert_eq!(errno(fs.allocate(Inode::ROOT, 0, 4096).await), Errno::EISDIR);
    })
}

#[test]
fn fsync() {
    with_fs(async |fs| {
        let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"contents").await.unwrap();
        fs.fsync(file).await.expect("fsync failed");
        fs.fsync(Inode::ROOT).await.expect("fsync failed");
    })
}

#[test]
fn statfs() {
    with_fs(async |fs| {
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
    })
}

#[test]
fn contents_survive_remount() {
    block_on(async {
        let fs = format().await;
        let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).await.unwrap().inode;
        let file = fs.create(dir, b"file", 0o600, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"persistent").await.unwrap();
        fs.symlink(dir, b"link", b"file", OWNER).await.unwrap();
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
    with_fs(async |fs| {
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
    })
}

#[test]
fn readdir_entries_from_an_offset_past_the_end() {
    with_fs(async |fs| {
        let handle = fs.opendir(Inode::ROOT).await.unwrap();
        assert_eq!(fs.readdir(handle, 2).unwrap().len(), 0);
        assert_eq!(fs.readdir(handle, u64::MAX).unwrap().len(), 0);
        fs.releasedir(handle);
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
    send(fs.unmount());
    block_on(close_and_check(fs));
}
