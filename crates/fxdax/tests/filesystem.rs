// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of a filesystem through its public API, as a host uses it.

use futures::executor::block_on;
use fxdax::{
    DirEntry, Errno, FileType, Filesystem, Node, Owner, RenameFlags, Result, SetAttr, SetTime,
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

async fn read_all(file: &Node) -> Vec<u8> {
    let mut buf = vec![0; file.getattr().await.unwrap().size as usize + 16];
    let len = file.read(0, &mut buf).await.expect("read failed");
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
        let root = fs.root().getattr().await.expect("getattr failed");
        assert_eq!(root.inode, fs.root().inode());
        assert_eq!(root.file_type, FileType::Directory);
        assert_eq!(root.perm, 0o755);
        assert_eq!(root.mode(), 0o040755);
    })
}

#[test]
fn create_then_lookup() {
    with_fs(async |fs| {
        let root = fs.root();
        let (file, created) = root.create(b"file", 0o640, OWNER).await.expect("create failed");
        assert_eq!(created.file_type, FileType::File);
        assert_eq!(created.perm, 0o640);
        assert_eq!((created.uid, created.gid), (1000, 100));
        assert_eq!(created.size, 0);
        assert_eq!(created.nlink, 1);
        assert_eq!(created.inode, file.inode());
        assert_ne!(created.inode, root.inode());

        let (found, attr) = root.lookup(b"file").await.expect("lookup failed");
        assert_eq!(attr, created);
        assert_eq!(found.inode(), file.inode());
        assert_eq!(errno(root.create(b"file", 0o640, OWNER).await), Errno::EEXIST);
        assert_eq!(errno(root.lookup(b"missing").await), Errno::ENOENT);
    })
}

#[test]
fn write_then_read() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        assert_eq!(file.write(0, b"hello, world").await.expect("write failed"), 12);
        assert_eq!(file.write(7, b"fxdax").await.expect("write failed"), 5);
        assert_eq!(file.getattr().await.unwrap().size, 12);

        let mut buf = [0; 32];
        assert_eq!(file.read(0, &mut buf).await.expect("read failed"), 12);
        assert_eq!(&buf[..12], b"hello, fxdax");
        assert_eq!(file.read(7, &mut buf[..3]).await.expect("read failed"), 3);
        assert_eq!(&buf[..3], b"fxd");
        assert_eq!(file.read(100, &mut buf).await.expect("read failed"), 0);
        assert_eq!(errno(fs.root().read(0, &mut buf).await), Errno::EISDIR);
    })
}

#[test]
fn write_past_the_end_leaves_a_hole() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(100_000, b"end").await.expect("write failed");
        assert_eq!(file.getattr().await.unwrap().size, 100_003);
        let contents = read_all(&file).await;
        assert!(contents[..100_000].iter().all(|&byte| byte == 0));
        assert_eq!(&contents[100_000..], b"end");
    })
}

#[test]
fn setattr_changes_size() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"0123456789").await.unwrap();
        let attr = file.setattr(&SetAttr { size: Some(4), ..Default::default() }).await.unwrap();
        assert_eq!(attr.size, 4);
        assert_eq!(read_all(&file).await, b"0123");
        let attr = file.setattr(&SetAttr { size: Some(8), ..Default::default() }).await.unwrap();
        assert_eq!(attr.size, 8);
        assert_eq!(read_all(&file).await, b"0123\0\0\0\0");
        assert_eq!(
            errno(fs.root().setattr(&SetAttr { size: Some(0), ..Default::default() }).await),
            Errno::EISDIR
        );
    })
}

#[test]
fn setattr_changes_owner_permissions_and_times() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        let attr = file
            .setattr(&SetAttr {
                perm: Some(0o4755),
                uid: Some(7),
                gid: Some(8),
                atime: Some(SetTime::At(time(1_000, 5))),
                mtime: Some(SetTime::At(time(2_000, 6))),
                ..Default::default()
            })
            .await
            .expect("setattr failed");
        assert_eq!(attr.perm, 0o4755);
        assert_eq!(attr.file_type, FileType::File);
        assert_eq!((attr.uid, attr.gid), (7, 8));
        assert_eq!(attr.atime, time(1_000, 5));
        assert_eq!(attr.mtime, time(2_000, 6));

        // Clearing setuid leaves setgid alone unless the file is group-executable.
        let attr =
            file.setattr(&SetAttr { perm: Some(0o6745), ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o6745);
        let attr =
            file.setattr(&SetAttr { kill_suid_sgid: true, ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o2745);
        let attr =
            file.setattr(&SetAttr { perm: Some(0o6755), ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o6755);
        let attr =
            file.setattr(&SetAttr { kill_suid_sgid: true, ..Default::default() }).await.unwrap();
        assert_eq!(attr.perm, 0o0755);

        let before = SystemTime::now();
        let attr = file.setattr(&SetAttr { mtime: Some(SetTime::Now), ..Default::default() }).await;
        assert!(attr.unwrap().mtime >= before);

        // Fxfs's times can't be before the epoch.
        let before_epoch = SetTime::At(UNIX_EPOCH - Duration::from_secs(1));
        let set = SetAttr { mtime: Some(before_epoch), ..Default::default() };
        assert_eq!(errno(file.setattr(&set).await), Errno::EINVAL);
    })
}

/// An open with `O_TRUNC` truncates the file with a setattr, clearing setuid as an unprivileged
/// truncation does.
#[test]
fn truncating_open() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o4755, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();
        let truncate = SetAttr { size: Some(0), kill_suid_sgid: true, ..Default::default() };
        let attr = file.setattr(&truncate).await.expect("setattr failed");
        assert_eq!((attr.size, attr.perm), (0, 0o755));
        assert_eq!(read_all(&file).await, b"");
    })
}

#[test]
fn mkdir_and_rmdir() {
    with_fs(async |fs| {
        let root = fs.root();
        let (dir, attr) = root.mkdir(b"dir", 0o750, OWNER).await.expect("mkdir failed");
        assert_eq!(attr.file_type, FileType::Directory);
        assert_eq!(attr.perm, 0o750);
        assert_eq!((attr.uid, attr.gid), (1000, 100));
        let (file, _) = dir.create(b"file", 0o644, OWNER).await.unwrap();

        assert_eq!(errno(root.rmdir(b"dir").await), Errno::ENOTEMPTY);
        assert_eq!(errno(dir.rmdir(b"file").await), Errno::ENOTDIR);
        dir.unlink(b"file").await.expect("unlink failed");
        drop(file);
        root.rmdir(b"dir").await.expect("rmdir failed");
        assert_eq!(errno(root.lookup(b"dir").await), Errno::ENOENT);
        assert_eq!(errno(root.rmdir(b"dir").await), Errno::ENOENT);
    })
}

#[test]
fn unlink() {
    with_fs(async |fs| {
        let root = fs.root();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();
        root.unlink(b"file").await.expect("unlink failed");
        assert_eq!(errno(root.lookup(b"file").await), Errno::ENOENT);
        assert_eq!(errno(root.unlink(b"file").await), Errno::ENOENT);

        // The handle still holds the file, which stays readable until it drops.
        assert_eq!(read_all(&file).await, b"contents");
        assert_eq!(file.getattr().await.unwrap().nlink, 0);
    })
}

/// Each handle to a node holds it, a lookup's as a create's: unlinked from every link it has, a
/// file stays readable while one lives.
#[test]
fn each_handle_holds_its_node() {
    with_fs(async |fs| {
        let root = fs.root();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();
        file.link(root, b"other").await.expect("link failed");
        let (other, _) = root.lookup(b"other").await.unwrap();
        assert_eq!(other.inode(), file.inode());
        root.unlink(b"file").await.unwrap();
        root.unlink(b"other").await.unwrap();
        drop(file);
        assert_eq!(read_all(&other).await, b"contents");
        assert_eq!(other.getattr().await.unwrap().nlink, 0);
    })
}

/// A file only looked up, with no connection to it yet, is kept too, and its connection opens
/// once it is unlinked.
#[test]
fn unlink_a_file_only_looked_up() {
    with_fs(async |fs| {
        let root = fs.root();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();
        drop(file);
        let (file, _) = root.lookup(b"file").await.unwrap();
        root.unlink(b"file").await.expect("unlink failed");

        assert_eq!(file.getattr().await.unwrap().nlink, 0);
        assert_eq!(read_all(&file).await, b"contents");
    })
}

#[test]
fn rename_within_and_across_directories() {
    with_fs(async |fs| {
        let (a, _) = fs.root().mkdir(b"a", 0o755, OWNER).await.unwrap();
        let (b, _) = fs.root().mkdir(b"b", 0o755, OWNER).await.unwrap();
        let (file, _) = a.create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();

        a.rename(b"file", &a, b"renamed", RenameFlags::empty()).await.expect("rename failed");
        assert_eq!(a.lookup(b"renamed").await.unwrap().1.inode, file.inode());
        a.rename(b"renamed", &b, b"moved", RenameFlags::empty()).await.expect("rename failed");
        assert_eq!(errno(a.lookup(b"renamed").await), Errno::ENOENT);
        let (moved, _) = b.lookup(b"moved").await.unwrap();
        assert_eq!(moved.inode(), file.inode());
        assert_eq!(read_all(&moved).await, b"contents");
    })
}

#[test]
fn rename_replaces_unless_told_not_to() {
    with_fs(async |fs| {
        let root = fs.root();
        let (first, _) = root.create(b"first", 0o644, OWNER).await.unwrap();
        let (second, _) = root.create(b"second", 0o644, OWNER).await.unwrap();
        first.write(0, b"first").await.unwrap();
        second.write(0, b"second").await.unwrap();

        assert_eq!(
            errno(root.rename(b"first", root, b"second", RenameFlags::NOREPLACE).await),
            Errno::EEXIST
        );
        assert_eq!(
            errno(root.rename(b"first", root, b"second", RenameFlags::EXCHANGE).await),
            Errno::EINVAL
        );
        root.rename(b"first", root, b"third", RenameFlags::NOREPLACE).await.expect("rename failed");
        root.rename(b"third", root, b"second", RenameFlags::empty()).await.expect("rename failed");
        let (replaced, _) = root.lookup(b"second").await.unwrap();
        assert_eq!(replaced.inode(), first.inode());
        assert_eq!(read_all(&replaced).await, b"first");
    })
}

/// A directory renamed is listed by its new parent, under its inode, and not by its old one; its
/// handle lists it as before.
#[test]
fn renamed_directory_has_its_new_parent() {
    with_fs(async |fs| {
        let root = fs.root();
        let (a, _) = root.mkdir(b"a", 0o755, OWNER).await.unwrap();
        let (sub, _) = a.mkdir(b"sub", 0o755, OWNER).await.unwrap();
        a.rename(b"sub", root, b"sub", RenameFlags::empty()).await.expect("rename failed");
        let listed = root.entries().await.expect("entries failed");
        let entry = listed.iter().find(|entry| entry.name == b"sub");
        assert_eq!(
            entry.map(|entry| (entry.inode, entry.file_type)),
            Some((sub.inode(), FileType::Directory))
        );
        assert_eq!(a.entries().await.unwrap(), []);
        assert_eq!(sub.entries().await.unwrap(), []);
    })
}

#[test]
fn symlink_and_readlink() {
    with_fs(async |fs| {
        let root = fs.root();
        let (link, attr) =
            root.symlink(b"link", b"some/target", OWNER).await.expect("symlink failed");
        assert_eq!(attr.file_type, FileType::Symlink);
        assert_eq!(attr.size, 11);
        assert_eq!(link.readlink().await.expect("readlink failed"), b"some/target");
        assert_eq!(root.lookup(b"link").await.unwrap().1.file_type, FileType::Symlink);
        assert_eq!(errno(root.readlink().await), Errno::EINVAL);
        assert_eq!(errno(root.symlink(b"link", b"other", OWNER).await), Errno::EEXIST);
    })
}

#[test]
fn symlinks_have_owners_and_times() {
    block_on(async {
        let fs = format().await;
        let (link, attr) = fs.root().symlink(b"link", b"target", OWNER).await.unwrap();
        assert_eq!((attr.uid, attr.gid, attr.perm), (OWNER.uid, OWNER.gid, 0o777));
        let set = SetAttr {
            uid: Some(7),
            atime: Some(SetTime::At(time(1_000, 1))),
            mtime: Some(SetTime::At(time(2_000, 2))),
            ..Default::default()
        };
        let changed = link.setattr(&set).await.expect("setattr failed");
        assert_eq!((changed.uid, changed.gid), (7, OWNER.gid));
        assert_eq!((changed.atime, changed.mtime), (time(1_000, 1), time(2_000, 2)));
        assert_eq!(changed.file_type, FileType::Symlink);
        // A symlink's permissions are always all of them.
        let chmod = SetAttr { perm: Some(0o600), ..Default::default() };
        assert_eq!(link.setattr(&chmod).await.expect("chmod failed").perm, 0o777);
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let (link, attr) = fs.root().lookup(b"link").await.unwrap();
        assert_eq!((attr.uid, attr.gid, attr.perm), (7, OWNER.gid, 0o777));
        assert_eq!(attr.mtime, time(2_000, 2));
        assert_eq!(link.readlink().await.unwrap(), b"target");
        close_and_check(fs).await;
    })
}

#[test]
fn extended_attributes() {
    block_on(async {
        let fs = format().await;
        let root = fs.root();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        let (dir, _) = root.mkdir(b"dir", 0o755, OWNER).await.unwrap();
        let (link, _) = root.symlink(b"link", b"file", OWNER).await.unwrap();
        for (node, value) in
            [(&file, &b"of a file"[..]), (&dir, b"of a dir"), (&link, b"of a link")]
        {
            node.setxattr(b"user.what", value, XattrMode::Set).await.expect("setxattr failed");
            assert_eq!(node.getxattr(b"user.what").await.unwrap(), value);
        }
        file.setxattr(b"security.capability", &[1, 0, 0, 2], XattrMode::Create).await.unwrap();
        assert_eq!(
            errno(file.setxattr(b"user.what", b"again", XattrMode::Create).await),
            Errno::EEXIST
        );
        assert_eq!(
            errno(file.setxattr(b"user.none", b"x", XattrMode::Replace).await),
            Errno::ENODATA
        );
        file.setxattr(b"user.what", b"replaced", XattrMode::Replace).await.unwrap();
        let mut names = file.listxattr().await.expect("listxattr failed");
        names.sort();
        assert_eq!(names, [b"security.capability".to_vec(), b"user.what".to_vec()]);
        assert_eq!(errno(file.getxattr(b"user.none").await), Errno::ENODATA);
        dir.removexattr(b"user.what").await.expect("removexattr failed");
        assert_eq!(errno(dir.removexattr(b"user.what").await), Errno::ENODATA);
        assert_eq!(dir.listxattr().await.unwrap(), Vec::<Vec<u8>>::new());
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.unwrap();
        let (file, _) = fs.root().lookup(b"file").await.unwrap();
        assert_eq!(file.getxattr(b"user.what").await.unwrap(), b"replaced");
        assert_eq!(file.getxattr(b"security.capability").await.unwrap(), [1, 0, 0, 2]);
        let (link, _) = fs.root().lookup(b"link").await.unwrap();
        assert_eq!(link.getxattr(b"user.what").await.unwrap(), b"of a link");
        close_and_check(fs).await;
    })
}

#[test]
fn link() {
    with_fs(async |fs| {
        let root = fs.root();
        let (dir, _) = root.mkdir(b"dir", 0o755, OWNER).await.unwrap();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();

        let linked = file.link(&dir, b"link").await.expect("link failed");
        assert_eq!(linked.inode, file.inode());
        assert_eq!(linked.nlink, 2);
        assert_eq!(dir.lookup(b"link").await.unwrap().1.inode, file.inode());
        root.unlink(b"file").await.unwrap();
        assert_eq!(file.getattr().await.unwrap().nlink, 1);
        assert_eq!(read_all(&file).await, b"contents");

        let (symlink, _) = root.symlink(b"symlink", b"target", OWNER).await.unwrap();
        assert_eq!(symlink.link(&dir, b"symlink").await.unwrap().nlink, 2);
        assert_eq!(errno(dir.link(root, b"dir-link").await), Errno::EPERM);
    })
}

#[test]
fn mknod_makes_special_files() {
    with_fs(async |fs| {
        let root = fs.root();
        let (_, fifo) =
            root.mknod(b"fifo", FileType::Fifo, 0o640, 0, OWNER).await.expect("mknod failed");
        assert_eq!(fifo.file_type, FileType::Fifo);
        assert_eq!(fifo.perm, 0o640);
        let (_, device) =
            root.mknod(b"device", FileType::CharDevice, 0o600, 0x0103, OWNER).await.unwrap();
        assert_eq!(device.file_type, FileType::CharDevice);
        assert_eq!(device.rdev, 0x0103);
        let (_, socket) = root.mknod(b"socket", FileType::Socket, 0o755, 0, OWNER).await.unwrap();
        assert_eq!(socket.file_type, FileType::Socket);
        let (_, file) = root.mknod(b"file", FileType::File, 0o600, 0, OWNER).await.unwrap();
        assert_eq!(file.file_type, FileType::File);
        assert_eq!(
            errno(root.mknod(b"dir", FileType::Directory, 0o755, 0, OWNER).await),
            Errno::EINVAL
        );

        assert_eq!(root.lookup(b"fifo").await.unwrap().1.file_type, FileType::Fifo);
        assert_eq!(root.lookup(b"device").await.unwrap().1.rdev, 0x0103);
    })
}

/// Each listing of a directory lists the entries it holds then.
#[test]
fn each_listing_is_of_its_time() {
    with_fs(async |fs| {
        let first = fs.root().entries().await.unwrap();
        fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        let second = fs.root().entries().await.unwrap();
        assert_eq!(first, []);
        assert_eq!(names(&second), [(&b"file"[..], FileType::File)]);
    })
}

/// A directory renamed moves, with what it holds: its new parent lists it, its old one doesn't,
/// and its siblings stay.
#[test]
fn renamed_directories_move_with_their_entries() {
    with_fs(async |fs| {
        let root = fs.root();
        let (outer, _) = root.mkdir(b"outer", 0o755, OWNER).await.unwrap();
        let (inner, _) = outer.mkdir(b"inner", 0o755, OWNER).await.unwrap();
        let (other, _) = root.mkdir(b"other", 0o755, OWNER).await.unwrap();
        let (sibling, _) = outer.mkdir(b"sibling", 0o755, OWNER).await.unwrap();
        let (child, _) = inner.create(b"child", 0o644, OWNER).await.unwrap();
        outer.rename(b"inner", &other, b"moved", RenameFlags::empty()).await.unwrap();
        assert_eq!(
            other.entries().await.unwrap(),
            [DirEntry {
                inode: inner.inode(),
                file_type: FileType::Directory,
                name: b"moved".to_vec()
            }]
        );
        // Only the directory renamed moves.
        assert_eq!(
            outer.entries().await.unwrap(),
            [DirEntry {
                inode: sibling.inode(),
                file_type: FileType::Directory,
                name: b"sibling".to_vec()
            }]
        );
        assert_eq!(
            inner.entries().await.unwrap(),
            [DirEntry { inode: child.inode(), file_type: FileType::File, name: b"child".to_vec() }]
        );
    })
}

#[test]
fn entries() {
    with_fs(async |fs| {
        let root = fs.root();
        let (dir, _) = root.mkdir(b"dir", 0o755, OWNER).await.unwrap();
        let (file, _) = root.create(b"file", 0o644, OWNER).await.unwrap();
        let (link, _) = root.symlink(b"link", b"file", OWNER).await.unwrap();

        let entries = root.entries().await.expect("entries failed");
        assert_eq!(
            names(&entries),
            [
                (&b"dir"[..], FileType::Directory),
                (b"file", FileType::File),
                (b"link", FileType::Symlink),
            ]
        );
        assert_eq!(
            entries.iter().map(|entry| entry.inode).collect::<Vec<_>>(),
            [dir.inode(), file.inode(), link.inode()]
        );
        assert_eq!(errno(file.entries().await), Errno::ENOTDIR);
    })
}

#[test]
fn entries_of_many() {
    with_fs(async |fs| {
        let names: Vec<Vec<u8>> =
            (0..500).map(|i| format!("file-with-a-long-name-{i:04}").into_bytes()).collect();
        for name in &names {
            fs.root().create(name, 0o644, OWNER).await.unwrap();
        }
        let entries = fs.root().entries().await.unwrap();
        assert_eq!(entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), names);
    })
}

#[test]
fn allocate() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.allocate(0, 65536).await.expect("allocate failed");
        let attr = file.getattr().await.unwrap();
        assert_eq!(attr.size, 65536);
        assert_eq!(attr.blocks, 128);
        assert_eq!(errno(fs.root().allocate(0, 4096).await), Errno::EISDIR);
    })
}

#[test]
fn fsync() {
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"contents").await.unwrap();
        file.fsync().await.expect("fsync failed");
        fs.root().fsync().await.expect("fsync failed");
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

        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, &vec![0xaa; 1 << 20]).await.unwrap();
        file.fsync().await.unwrap();
        let after = fs.statfs().await.unwrap();
        assert!(after.blocks_free + 256 <= before.blocks_free, "{after:?} vs {before:?}");
    })
}

#[test]
fn contents_survive_remount() {
    block_on(async {
        let fs = format().await;
        let (dir, _) = fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
        let (file, _) = dir.create(b"file", 0o600, OWNER).await.unwrap();
        file.write(0, b"persistent").await.unwrap();
        dir.symlink(b"link", b"file", OWNER).await.unwrap();
        dir.mknod(b"fifo", FileType::Fifo, 0o644, 0, OWNER).await.unwrap();
        let storage = close_and_check(fs).await;

        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let (dir, _) = fs.root().lookup(b"dir").await.unwrap();
        let (file, attr) = dir.lookup(b"file").await.unwrap();
        assert_eq!((attr.perm, attr.uid, attr.gid), (0o600, 1000, 100));
        assert_eq!(read_all(&file).await, b"persistent");
        let (link, _) = dir.lookup(b"link").await.unwrap();
        assert_eq!(link.readlink().await.unwrap(), b"file");
        assert_eq!(dir.lookup(b"fifo").await.unwrap().1.file_type, FileType::Fifo);
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
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"on disk").await.unwrap();
        // Dropping the filesystem unmounts it; a handle that outlives it holds nothing open, the
        // image included.
        drop(fs);
        assert_eq!(errno(file.getattr().await), Errno::ENOTCONN);
        let storage =
            Storage::open(&path).expect("open failed").check().await.expect("check failed");
        drop(file);

        // The storage a filesystem gives back can be mounted again.
        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let storage = fs.close().await.expect("close failed");
        let fs = Filesystem::mount(storage).await.expect("mount failed");
        let (file, _) = fs.root().lookup(b"file").await.unwrap();
        assert_eq!(read_all(&file).await, b"on disk");
        fs.unmount().await.expect("unmount failed");
        // Unmounting again does nothing.
        fs.unmount().await.expect("unmount failed");
        assert_eq!(errno(file.getattr().await), Errno::ENOTCONN);
        assert_eq!(errno(fs.root().lookup(b"file").await), Errno::ENOTCONN);
        // The unmounted filesystem keeps its storage for close.
        let storage = fs.close().await.expect("close failed");
        assert_eq!(errno(file.read(0, &mut [0; 8]).await), Errno::ENOTCONN);
        drop(file);
        storage.check().await.expect("check failed");
    })
}

/// Unmounts at once are one unmount, which each waits for, and after which the filesystem's
/// storage is closed, for close to give back.
#[test]
fn concurrent_unmounts_wait_for_one() {
    block_on(async {
        let fs = format().await;
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.write(0, b"written").await.unwrap();
        let (first, second) = futures::join!(fs.unmount(), fs.unmount());
        first.expect("unmount failed");
        second.expect("unmount failed");
        assert_eq!(errno(file.getattr().await), Errno::ENOTCONN);
        let fs = Filesystem::mount(close_and_check(fs).await).await.unwrap();
        let (file, _) = fs.root().lookup(b"file").await.unwrap();
        assert_eq!(read_all(&file).await, b"written");
        close_and_check(fs).await;
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
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        let written = file
            .write_vectored(
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
        assert_eq!(read_all(&file).await, b"\0\0\0hello, world");

        let (mut first, mut second) = ([0; 4], [0; 32]);
        let read = file
            .read_vectored(
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
            errno(fs.root().write_vectored(0, &[VolatileSlice::from(&b"x"[..])]).await),
            Errno::EISDIR
        );
    })
}

#[test]
fn an_empty_directory_has_no_entries() {
    with_fs(async |fs| {
        assert_eq!(fs.root().entries().await.unwrap(), []);
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
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            file.write(0, b"tokio").await.unwrap();
            assert_eq!(read_all(&file).await, b"tokio");
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
        let files: Vec<Node> = futures::future::join_all(
            names.iter().map(|name| fs.root().create(name, 0o644, OWNER)),
        )
        .await
        .into_iter()
        .map(|created| created.unwrap().0)
        .collect();
        let contents = |i: usize| vec![i as u8; 3 * kernel::page_size() as usize];
        let written = futures::future::join_all(
            files
                .iter()
                .enumerate()
                .map(|(i, file)| async move { file.write(0, &contents(i)).await.unwrap() }),
        )
        .await;
        assert!(written.iter().all(|&len| len == 3 * kernel::page_size() as usize));
        fs.root().fsync().await.unwrap();
        let read = futures::future::join_all(files.iter().map(read_all)).await;
        assert_eq!(read, (0..32).map(contents).collect::<Vec<_>>());
    })
}

/// The futures of the operations are `Send`, so that they may run on any executor's threads.
#[test]
fn futures_are_send() {
    fn send<T: Send>(_: T) {}
    let fs = block_on(format());
    let root = fs.root();
    let mut buf = [0u8; 1];
    let mut bufs = [VolatileSliceMut::from(&mut buf[..])];
    send(Filesystem::format(Storage::memory(STORAGE_SIZE)));
    send(Storage::memory(STORAGE_SIZE).check());
    send(root.lookup(b"file"));
    send(root.getattr());
    send(root.setattr(&SetAttr::default()));
    send(root.create(b"file", 0o644, OWNER));
    send(root.rename(b"a", root, b"b", RenameFlags::empty()));
    send(root.read_vectored(0, &mut bufs));
    send(root.write_vectored(0, &[VolatileSlice::from(&b"x"[..])]));
    send(root.entries());
    send(fs.unmount());
    block_on(close_and_check(fs));
}
