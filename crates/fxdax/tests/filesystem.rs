//! Tests of a filesystem through its public API, as a VMM's virtio-fs device uses it.

use fxdax::{
    Access, DaxWindow, DirEntry, Errno, FileType, Filesystem, Inode, Owner, Protection,
    RenameFlags, Result, SecondStage, SetAttr, SetTime, Storage, kernel,
};
use std::io::{IoSlice, IoSliceMut};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the filesystems the tests format.
const STORAGE_SIZE: u64 = 8 << 20;

fn format() -> Filesystem {
    Filesystem::format(Storage::memory(STORAGE_SIZE)).expect("format failed")
}

/// Closes `fs`, checks the filesystem and returns its storage.
fn close_and_check(fs: Filesystem) -> Storage {
    fs.close().expect("close failed").check().expect("check failed")
}

fn errno<T: std::fmt::Debug>(result: Result<T>) -> Errno {
    result.expect_err("succeeded")
}

fn names(entries: &[DirEntry]) -> Vec<(&[u8], FileType)> {
    entries.iter().map(|entry| (entry.name.as_slice(), entry.file_type)).collect()
}

fn read_all(fs: &Filesystem, inode: Inode) -> Vec<u8> {
    let mut buf = vec![0; fs.getattr(inode).unwrap().size as usize + 16];
    let len = fs.read(inode, 0, &mut buf).expect("read failed");
    buf.truncate(len);
    buf
}

/// The time `secs` and `nanos` after the Unix epoch.
fn time(secs: u64, nanos: u32) -> SystemTime {
    UNIX_EPOCH + Duration::new(secs, nanos)
}

#[test]
fn root_is_a_directory() {
    let fs = format();
    let root = fs.getattr(Inode::ROOT).expect("getattr failed");
    assert_eq!(root.inode, Inode::ROOT);
    assert_eq!(root.file_type, FileType::Directory);
    assert_eq!(root.perm, 0o755);
    assert_eq!(root.mode(), 0o040755);
    close_and_check(fs);
}

#[test]
fn create_then_lookup() {
    let fs = format();
    let created = fs.create(Inode::ROOT, b"file", 0o640, OWNER).expect("create failed");
    assert_eq!(created.file_type, FileType::File);
    assert_eq!(created.perm, 0o640);
    assert_eq!((created.uid, created.gid), (1000, 100));
    assert_eq!(created.size, 0);
    assert_eq!(created.nlink, 1);
    assert_ne!(created.inode, Inode::ROOT);

    let found = fs.lookup(Inode::ROOT, b"file").expect("lookup failed");
    assert_eq!(found, created);
    assert_eq!(errno(fs.create(Inode::ROOT, b"file", 0o640, OWNER)), Errno::EEXIST);
    assert_eq!(errno(fs.lookup(Inode::ROOT, b"missing")), Errno::ENOENT);
    close_and_check(fs);
}

#[test]
fn write_then_read() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    assert_eq!(fs.write(file, 0, b"hello, world").expect("write failed"), 12);
    assert_eq!(fs.write(file, 7, b"fxdax").expect("write failed"), 5);
    assert_eq!(fs.getattr(file).unwrap().size, 12);

    let mut buf = [0; 32];
    assert_eq!(fs.read(file, 0, &mut buf).expect("read failed"), 12);
    assert_eq!(&buf[..12], b"hello, fxdax");
    assert_eq!(fs.read(file, 7, &mut buf[..3]).expect("read failed"), 3);
    assert_eq!(&buf[..3], b"fxd");
    assert_eq!(fs.read(file, 100, &mut buf).expect("read failed"), 0);
    assert_eq!(errno(fs.read(Inode::ROOT, 0, &mut buf)), Errno::EISDIR);
    close_and_check(fs);
}

#[test]
fn write_past_the_end_leaves_a_hole() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 100_000, b"end").expect("write failed");
    assert_eq!(fs.getattr(file).unwrap().size, 100_003);
    let contents = read_all(&fs, file);
    assert!(contents[..100_000].iter().all(|&byte| byte == 0));
    assert_eq!(&contents[100_000..], b"end");
    close_and_check(fs);
}

#[test]
fn setattr_changes_size() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"0123456789").unwrap();
    let attr = fs.setattr(file, &SetAttr { size: Some(4), ..Default::default() }).unwrap();
    assert_eq!(attr.size, 4);
    assert_eq!(read_all(&fs, file), b"0123");
    let attr = fs.setattr(file, &SetAttr { size: Some(8), ..Default::default() }).unwrap();
    assert_eq!(attr.size, 8);
    assert_eq!(read_all(&fs, file), b"0123\0\0\0\0");
    assert_eq!(
        errno(fs.setattr(Inode::ROOT, &SetAttr { size: Some(0), ..Default::default() })),
        Errno::EISDIR
    );
    close_and_check(fs);
}

#[test]
fn setattr_changes_owner_permissions_and_times() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
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
        .expect("setattr failed");
    assert_eq!(attr.perm, 0o4755);
    assert_eq!(attr.file_type, FileType::File);
    assert_eq!((attr.uid, attr.gid), (7, 8));
    assert_eq!(attr.atime, time(1_000, 5));
    assert_eq!(attr.mtime, time(2_000, 6));

    // Clearing setuid leaves setgid alone unless the file is group-executable.
    let attr = fs.setattr(file, &SetAttr { perm: Some(0o6745), ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o6745);
    let attr = fs.setattr(file, &SetAttr { kill_suid_sgid: true, ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o2745);
    let attr = fs.setattr(file, &SetAttr { perm: Some(0o6755), ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o6755);
    let attr = fs.setattr(file, &SetAttr { kill_suid_sgid: true, ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o0755);

    let before = SystemTime::now();
    let attr = fs.setattr(file, &SetAttr { mtime: Some(SetTime::Now), ..Default::default() });
    assert!(attr.unwrap().mtime >= before);

    // Fxfs's times can't be before the epoch.
    let before_epoch = SetTime::At(UNIX_EPOCH - Duration::from_secs(1));
    let set = SetAttr { mtime: Some(before_epoch), ..Default::default() };
    assert_eq!(errno(fs.setattr(file, &set)), Errno::EINVAL);
    close_and_check(fs);
}

/// An open with `O_TRUNC` truncates the file with a setattr, clearing setuid as an unprivileged
/// truncation does.
#[test]
fn truncating_open() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o4755, OWNER).unwrap().inode;
    fs.write(file, 0, b"contents").unwrap();
    let truncate = SetAttr { size: Some(0), kill_suid_sgid: true, ..Default::default() };
    let attr = fs.setattr(file, &truncate).expect("setattr failed");
    assert_eq!((attr.size, attr.perm), (0, 0o755));
    assert_eq!(read_all(&fs, file), b"");
    close_and_check(fs);
}

#[test]
fn mkdir_and_rmdir() {
    let fs = format();
    let dir = fs.mkdir(Inode::ROOT, b"dir", 0o750, OWNER).expect("mkdir failed");
    assert_eq!(dir.file_type, FileType::Directory);
    assert_eq!(dir.perm, 0o750);
    assert_eq!((dir.uid, dir.gid), (1000, 100));
    let file = fs.create(dir.inode, b"file", 0o644, OWNER).unwrap();

    assert_eq!(errno(fs.rmdir(Inode::ROOT, b"dir")), Errno::ENOTEMPTY);
    assert_eq!(errno(fs.rmdir(dir.inode, b"file")), Errno::ENOTDIR);
    fs.unlink(dir.inode, b"file").expect("unlink failed");
    fs.forget(file.inode, 1);
    fs.rmdir(Inode::ROOT, b"dir").expect("rmdir failed");
    assert_eq!(errno(fs.lookup(Inode::ROOT, b"dir")), Errno::ENOENT);
    assert_eq!(errno(fs.rmdir(Inode::ROOT, b"dir")), Errno::ENOENT);
    fs.forget(dir.inode, 1);
    close_and_check(fs);
}

#[test]
fn unlink() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"contents").unwrap();
    fs.unlink(Inode::ROOT, b"file").expect("unlink failed");
    assert_eq!(errno(fs.lookup(Inode::ROOT, b"file")), Errno::ENOENT);
    assert_eq!(errno(fs.unlink(Inode::ROOT, b"file")), Errno::ENOENT);

    // The kernel still holds a lookup on the file, which stays readable until it forgets it.
    assert_eq!(read_all(&fs, file), b"contents");
    assert_eq!(fs.getattr(file).unwrap().nlink, 0);
    fs.forget(file, 1);
    assert_eq!(errno(fs.getattr(file)), Errno::ENOENT);
    close_and_check(fs);
}

#[test]
fn rename_within_and_across_directories() {
    let fs = format();
    let a = fs.mkdir(Inode::ROOT, b"a", 0o755, OWNER).unwrap().inode;
    let b = fs.mkdir(Inode::ROOT, b"b", 0o755, OWNER).unwrap().inode;
    let file = fs.create(a, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"contents").unwrap();

    fs.rename(a, b"file", a, b"renamed", RenameFlags::empty()).expect("rename failed");
    assert_eq!(fs.lookup(a, b"renamed").unwrap().inode, file);
    fs.rename(a, b"renamed", b, b"moved", RenameFlags::empty()).expect("rename failed");
    assert_eq!(errno(fs.lookup(a, b"renamed")), Errno::ENOENT);
    let moved = fs.lookup(b, b"moved").unwrap().inode;
    assert_eq!(moved, file);
    assert_eq!(read_all(&fs, moved), b"contents");
    close_and_check(fs);
}

#[test]
fn rename_replaces_unless_told_not_to() {
    let fs = format();
    let first = fs.create(Inode::ROOT, b"first", 0o644, OWNER).unwrap().inode;
    let second = fs.create(Inode::ROOT, b"second", 0o644, OWNER).unwrap().inode;
    fs.write(first, 0, b"first").unwrap();
    fs.write(second, 0, b"second").unwrap();

    assert_eq!(
        errno(fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"second", RenameFlags::NOREPLACE)),
        Errno::EEXIST
    );
    assert_eq!(
        errno(fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"second", RenameFlags::EXCHANGE)),
        Errno::EINVAL
    );
    fs.rename(Inode::ROOT, b"first", Inode::ROOT, b"third", RenameFlags::NOREPLACE)
        .expect("rename failed");
    fs.rename(Inode::ROOT, b"third", Inode::ROOT, b"second", RenameFlags::empty())
        .expect("rename failed");
    let replaced = fs.lookup(Inode::ROOT, b"second").unwrap().inode;
    assert_eq!(replaced, first);
    assert_eq!(read_all(&fs, replaced), b"first");
    close_and_check(fs);
}

#[test]
fn renamed_directory_has_its_new_parent() {
    let fs = format();
    let a = fs.mkdir(Inode::ROOT, b"a", 0o755, OWNER).unwrap().inode;
    let sub = fs.mkdir(a, b"sub", 0o755, OWNER).unwrap().inode;
    fs.rename(a, b"sub", Inode::ROOT, b"sub", RenameFlags::empty()).expect("rename failed");
    let handle = fs.opendir(sub).expect("opendir failed");
    let entries = fs.readdir(handle, 0).expect("readdir failed");
    assert_eq!(
        entries[..],
        [
            DirEntry { inode: sub, file_type: FileType::Directory, name: b".".to_vec() },
            DirEntry { inode: Inode::ROOT, file_type: FileType::Directory, name: b"..".to_vec() },
        ]
    );
    fs.releasedir(handle);
    close_and_check(fs);
}

#[test]
fn symlink_and_readlink() {
    let fs = format();
    let link = fs.symlink(Inode::ROOT, b"link", b"some/target").expect("symlink failed");
    assert_eq!(link.file_type, FileType::Symlink);
    assert_eq!(link.size, 11);
    assert_eq!(fs.readlink(link.inode).expect("readlink failed"), b"some/target");
    assert_eq!(fs.lookup(Inode::ROOT, b"link").unwrap().file_type, FileType::Symlink);
    assert_eq!(errno(fs.readlink(Inode::ROOT)), Errno::EINVAL);
    assert_eq!(errno(fs.symlink(Inode::ROOT, b"link", b"other")), Errno::EEXIST);
    close_and_check(fs);
}

#[test]
fn link() {
    let fs = format();
    let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).unwrap().inode;
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"contents").unwrap();

    let linked = fs.link(file, dir, b"link").expect("link failed");
    assert_eq!(linked.inode, file);
    assert_eq!(linked.nlink, 2);
    assert_eq!(fs.lookup(dir, b"link").unwrap().inode, file);
    fs.unlink(Inode::ROOT, b"file").unwrap();
    assert_eq!(fs.getattr(file).unwrap().nlink, 1);
    assert_eq!(read_all(&fs, file), b"contents");

    let symlink = fs.symlink(Inode::ROOT, b"symlink", b"target").unwrap().inode;
    assert_eq!(fs.link(symlink, dir, b"symlink").unwrap().nlink, 2);
    assert_eq!(errno(fs.link(dir, Inode::ROOT, b"dir-link")), Errno::EPERM);
    close_and_check(fs);
}

#[test]
fn mknod_makes_special_files() {
    let fs = format();
    let fifo =
        fs.mknod(Inode::ROOT, b"fifo", FileType::Fifo, 0o640, 0, OWNER).expect("mknod failed");
    assert_eq!(fifo.file_type, FileType::Fifo);
    assert_eq!(fifo.perm, 0o640);
    let device =
        fs.mknod(Inode::ROOT, b"device", FileType::CharDevice, 0o600, 0x0103, OWNER).unwrap();
    assert_eq!(device.file_type, FileType::CharDevice);
    assert_eq!(device.rdev, 0x0103);
    let socket = fs.mknod(Inode::ROOT, b"socket", FileType::Socket, 0o755, 0, OWNER).unwrap();
    assert_eq!(socket.file_type, FileType::Socket);
    let file = fs.mknod(Inode::ROOT, b"file", FileType::File, 0o600, 0, OWNER).unwrap();
    assert_eq!(file.file_type, FileType::File);
    assert_eq!(
        errno(fs.mknod(Inode::ROOT, b"dir", FileType::Directory, 0o755, 0, OWNER)),
        Errno::EINVAL
    );

    assert_eq!(fs.lookup(Inode::ROOT, b"fifo").unwrap().file_type, FileType::Fifo);
    assert_eq!(fs.lookup(Inode::ROOT, b"device").unwrap().rdev, 0x0103);
    close_and_check(fs);
}

#[test]
fn readdir() {
    let fs = format();
    let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).unwrap().inode;
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    let link = fs.symlink(Inode::ROOT, b"link", b"file").unwrap().inode;

    let handle = fs.opendir(Inode::ROOT).expect("opendir failed");
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
    fs.create(Inode::ROOT, b"later", 0o644, OWNER).unwrap();
    assert_eq!(fs.readdir(handle, 0).unwrap().len(), 5);
    fs.releasedir(handle);
    assert_eq!(errno(fs.readdir(handle, 0)), Errno::EBADF);
    assert_eq!(errno(fs.opendir(file)), Errno::ENOTDIR);
    close_and_check(fs);
}

#[test]
fn readdir_of_many_entries() {
    let fs = format();
    let names: Vec<Vec<u8>> =
        (0..500).map(|i| format!("file-with-a-long-name-{i:04}").into_bytes()).collect();
    for name in &names {
        fs.create(Inode::ROOT, name, 0o644, OWNER).unwrap();
    }
    let handle = fs.opendir(Inode::ROOT).unwrap();
    let entries = fs.readdir(handle, 2).unwrap();
    assert_eq!(entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), names);
    fs.releasedir(handle);
    close_and_check(fs);
}

#[test]
fn allocate() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.allocate(file, 0, 65536).expect("allocate failed");
    let attr = fs.getattr(file).unwrap();
    assert_eq!(attr.size, 65536);
    assert_eq!(attr.blocks, 128);
    assert_eq!(errno(fs.allocate(Inode::ROOT, 0, 4096)), Errno::EISDIR);
    close_and_check(fs);
}

#[test]
fn fsync() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"contents").unwrap();
    fs.fsync(file).expect("fsync failed");
    fs.fsync(Inode::ROOT).expect("fsync failed");
    close_and_check(fs);
}

#[test]
fn statfs() {
    let fs = format();
    let before = fs.statfs().expect("statfs failed");
    assert_eq!(before.block_size, 4096);
    assert_eq!(before.blocks, STORAGE_SIZE / 4096);
    assert_eq!(before.name_max, 255);
    assert!(before.blocks_free < before.blocks);

    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, &vec![0xaa; 1 << 20]).unwrap();
    fs.fsync(file).unwrap();
    let after = fs.statfs().unwrap();
    assert!(after.blocks_free + 256 <= before.blocks_free, "{after:?} vs {before:?}");
    close_and_check(fs);
}

#[test]
fn contents_survive_remount() {
    let fs = format();
    let dir = fs.mkdir(Inode::ROOT, b"dir", 0o755, OWNER).unwrap().inode;
    let file = fs.create(dir, b"file", 0o600, OWNER).unwrap().inode;
    fs.write(file, 0, b"persistent").unwrap();
    fs.symlink(dir, b"link", b"file").unwrap();
    fs.mknod(dir, b"fifo", FileType::Fifo, 0o644, 0, OWNER).unwrap();
    let storage = close_and_check(fs);

    let fs = Filesystem::mount(storage).expect("mount failed");
    let dir = fs.lookup(Inode::ROOT, b"dir").unwrap().inode;
    let file = fs.lookup(dir, b"file").unwrap();
    assert_eq!((file.perm, file.uid, file.gid), (0o600, 1000, 100));
    assert_eq!(read_all(&fs, file.inode), b"persistent");
    let link = fs.lookup(dir, b"link").unwrap().inode;
    assert_eq!(fs.readlink(link).unwrap(), b"file");
    assert_eq!(fs.lookup(dir, b"fifo").unwrap().file_type, FileType::Fifo);
    close_and_check(fs);
}

#[test]
fn contents_survive_remount_of_an_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image");
    let storage = Storage::create(&path, 64 << 20).expect("create failed");
    assert_eq!(storage.size(), 64 << 20);
    assert!(Storage::create(&path, 64 << 20).is_err(), "created an image over another");

    let fs = Filesystem::format(storage).expect("format failed");
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    fs.write(file, 0, b"on disk").unwrap();
    // Dropping the filesystem unmounts it.
    drop(fs);
    let storage = Storage::open(&path).expect("open failed").check().expect("check failed");

    // The storage a filesystem gives back can be mounted again.
    let fs = Filesystem::mount(storage).expect("mount failed");
    let storage = fs.close().expect("close failed");
    let fs = Filesystem::mount(storage).expect("mount failed");
    let file = fs.lookup(Inode::ROOT, b"file").unwrap().inode;
    assert_eq!(read_all(&fs, file), b"on disk");
    let storage = fs.unmount().expect("unmount failed").expect("was mounted");
    assert!(fs.unmount().expect("unmount failed").is_none());
    assert_eq!(errno(fs.getattr(file)), Errno::ENOTCONN);
    assert_eq!(errno(fs.lookup(Inode::ROOT, b"file")), Errno::ENOTCONN);
    drop(fs);
    storage.check().expect("check failed");
}

#[test]
fn storage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Storage::open(dir.path().join("missing")).expect_err("opened a missing image");
    assert!(format!("{missing:#}").contains("missing"), "{missing:#}");
    let empty = Storage::create(dir.path().join("empty"), 0).expect_err("created an empty image");
    assert_eq!(empty.to_string(), "the image is smaller than a block");
    assert!(!dir.path().join("empty").exists(), "left an empty image behind");
    let unformatted = Filesystem::mount(Storage::memory(STORAGE_SIZE));
    assert!(unformatted.is_err(), "mounted unformatted storage");
}

#[test]
fn vectored_io() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    let written = fs
        .write_vectored(
            file,
            3,
            &[IoSlice::new(b"hello"), IoSlice::new(b""), IoSlice::new(b", world")],
        )
        .expect("write_vectored failed");
    assert_eq!(written, 12);
    assert_eq!(read_all(&fs, file), b"\0\0\0hello, world");

    let (mut first, mut second) = ([0; 4], [0; 32]);
    let read = fs
        .read_vectored(file, 1, &mut [IoSliceMut::new(&mut first), IoSliceMut::new(&mut second)])
        .expect("read_vectored failed");
    assert_eq!(read, 14);
    assert_eq!(&first, b"\0\0he");
    assert_eq!(&second[..10], b"llo, world");
    assert_eq!(errno(fs.write_vectored(Inode::ROOT, 0, &[IoSlice::new(b"x")])), Errno::EISDIR);
    close_and_check(fs);
}

#[test]
fn readdir_entries_from_an_offset_past_the_end() {
    let fs = format();
    let handle = fs.opendir(Inode::ROOT).unwrap();
    assert_eq!(fs.readdir(handle, 2).unwrap().len(), 0);
    assert_eq!(fs.readdir(handle, u64::MAX).unwrap().len(), 0);
    fs.releasedir(handle);
    close_and_check(fs);
}

#[test]
fn errno_names() {
    assert_eq!(Errno::ENOENT.raw(), 2);
    assert_eq!(Errno::from_raw(39), Errno::ENOTEMPTY);
    assert_eq!(Errno::ENOENT.to_string(), "ENOENT (errno 2)");
    assert_eq!(format!("{:?}", Errno::from_raw(4000)), "Errno(4000)");
    assert_eq!(fxdax::Error::from(Errno::EIO).errno(), Some(Errno::EIO));
}

#[test]
fn file_types() {
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
}

#[test]
#[should_panic(expected = "blocking call on an executor thread")]
fn stream_io_on_an_executor_thread_panics() {
    let fs = format();
    let file = fs.create(Inode::ROOT, b"file", 0o644, OWNER).unwrap().inode;
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        fs.read(file, 0, &mut [0; 8]).unwrap();
    });
}

/// `len` bytes of `window` at `offset`, as the guest sees them. The pages must have been faulted
/// in.
#[allow(clippy::mut_from_ref)]
fn window_bytes(window: &DaxWindow, offset: u64, len: usize) -> &mut [u8] {
    // SAFETY: The caller faulted the pages in, and nothing else accesses them while the test does.
    unsafe { std::slice::from_raw_parts_mut(window.host_base().add(offset as usize), len) }
}

/// A file of `len` bytes, each its offset modulo 251.
fn patterned_file(fs: &Filesystem, name: &[u8], len: usize) -> Inode {
    let file = fs.create(Inode::ROOT, name, 0o644, OWNER).unwrap().inode;
    let contents: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    fs.write(file, 0, &contents).unwrap();
    file
}

#[test]
fn dax_mapping_reads_the_file() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", 2 * ps as usize);
    let window = fs.create_dax_window(8 * ps, None).expect("create_dax_window failed");
    assert_eq!(window.size(), 8 * ps);

    fs.setup_mapping(file, 0, 2 * ps, ps, false).expect("setup_mapping failed");
    assert!(!window.is_mapped(0));
    assert!(window.is_mapped(ps));
    assert!(window.is_mapped(2 * ps));
    assert!(!window.is_mapped(3 * ps));

    window.fault(2 * ps, Access::Read).expect("fault failed");
    let expected: Vec<u8> = (ps as usize..2 * ps as usize).map(|i| (i % 251) as u8).collect();
    assert_eq!(window_bytes(&window, 2 * ps, ps as usize), &expected[..]);

    // The mapping is read-only.
    assert_eq!(errno(window.fault(ps, Access::Write)), Errno::EACCES);
    close_and_check(fs);
}

#[test]
fn dax_stores_reach_disk_on_fsync() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", 2 * ps as usize);
    // Write the file back, so that the store through the window is what dirties its page.
    fs.fsync(file).unwrap();
    let window = fs.create_dax_window(4 * ps, None).unwrap();
    fs.setup_mapping(file, 0, 2 * ps, 0, true).expect("setup_mapping failed");

    window.fault(ps, Access::Write).expect("fault failed");
    window_bytes(&window, ps + 10, 25).copy_from_slice(b"stored through the window");
    // The file's stream sees the store, as it shares the file's pages.
    let mut buf = [0; 25];
    fs.read(file, ps + 10, &mut buf).unwrap();
    assert_eq!(&buf, b"stored through the window");

    fs.fsync(file).expect("fsync failed");
    fs.remove_mapping(0, 2 * ps).expect("remove_mapping failed");
    let storage = close_and_check(fs);

    let fs = Filesystem::mount(storage).unwrap();
    let file = fs.lookup(Inode::ROOT, b"file").unwrap().inode;
    let contents = read_all(&fs, file);
    let mut expected: Vec<u8> = (0..2 * ps as usize).map(|i| (i % 251) as u8).collect();
    expected[ps as usize + 10..ps as usize + 35].copy_from_slice(b"stored through the window");
    assert_eq!(contents, expected);
    close_and_check(fs);
}

#[test]
fn dax_mapping_is_upgraded_to_writable() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", ps as usize);
    let window = fs.create_dax_window(4 * ps, None).unwrap();
    fs.setup_mapping(file, 0, ps, 0, false).unwrap();
    window.fault(0, Access::Read).unwrap();
    assert_eq!(errno(window.fault(0, Access::Write)), Errno::EACCES);

    // As Linux's FUSE client does on the first store to a range it mapped read-only.
    fs.setup_mapping(file, 0, ps, 0, true).expect("setup_mapping failed");
    window.fault(0, Access::Write).expect("fault failed");
    window_bytes(&window, 0, 5).copy_from_slice(b"hello");
    let mut buf = [0; 5];
    fs.read(file, 0, &mut buf).unwrap();
    assert_eq!(&buf, b"hello");
    close_and_check(fs);
}

#[test]
fn dax_remove_mapping() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", 4 * ps as usize);
    let window = fs.create_dax_window(4 * ps, None).unwrap();
    fs.setup_mapping(file, 0, 4 * ps, 0, true).unwrap();

    // Removing part of a mapping leaves the rest.
    fs.remove_mapping(ps, 2 * ps).expect("remove_mapping failed");
    assert!(window.is_mapped(0));
    assert!(!window.is_mapped(ps));
    assert!(!window.is_mapped(2 * ps));
    assert!(window.is_mapped(3 * ps));
    assert_eq!(errno(window.fault(ps, Access::Read)), Errno::ENOENT);
    window.fault(3 * ps, Access::Write).expect("fault failed");

    fs.remove_mapping(0, 4 * ps).expect("remove_mapping failed");
    assert!(!window.is_mapped(0));
    assert!(!window.is_mapped(3 * ps));
    close_and_check(fs);
}

#[test]
fn dax_past_the_end_of_the_file() {
    let fs = format();
    let ps = kernel::page_size();
    let size = ps + 100;
    let file = patterned_file(&fs, b"file", size as usize);
    let window = fs.create_dax_window(4 * ps, None).unwrap();
    // As Linux's FUSE client maps whole 2 MiB ranges, which extend past files' ends.
    fs.setup_mapping(file, 0, 3 * ps, 0, true).unwrap();

    // Past the end, the guest reads zeros.
    window.fault(2 * ps, Access::Read).expect("fault failed");
    assert!(window_bytes(&window, 2 * ps, ps as usize).iter().all(|&byte| byte == 0));
    window.fault(ps, Access::Read).expect("fault failed");
    assert_eq!(window_bytes(&window, ps + 99, 1), [((ps + 99) % 251) as u8]);
    assert!(window_bytes(&window, ps + 100, ps as usize - 100).iter().all(|&byte| byte == 0));

    // A store past the end in the last page doesn't reach the file.
    window.fault(ps, Access::Write).expect("fault failed");
    window_bytes(&window, ps + 200, 4).copy_from_slice(b"lost");
    fs.fsync(file).unwrap();
    assert_eq!(fs.getattr(file).unwrap().size, size);
    let storage = close_and_check(fs);

    let fs = Filesystem::mount(storage).unwrap();
    let file = fs.lookup(Inode::ROOT, b"file").unwrap().inode;
    assert_eq!(fs.getattr(file).unwrap().size, size);
    fs.setattr(file, &SetAttr { size: Some(2 * ps), ..Default::default() }).unwrap();
    let mut buf = [0xff; 4];
    fs.read(file, ps + 200, &mut buf).unwrap();
    assert_eq!(buf, [0; 4]);
    close_and_check(fs);
}

#[test]
fn dax_mapping_keeps_an_unlinked_file() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", ps as usize);
    let window = fs.create_dax_window(ps, None).unwrap();
    fs.setup_mapping(file, 0, ps, 0, false).unwrap();
    fs.unlink(Inode::ROOT, b"file").unwrap();
    fs.forget(file, 1);

    window.fault(0, Access::Read).expect("fault failed");
    assert_eq!(window_bytes(&window, 0, 3), [0, 1, 2]);
    fs.remove_mapping(0, ps).unwrap();
    close_and_check(fs);
}

/// The errno a window creation failed with.
fn create_errno(result: std::result::Result<DaxWindow, fxdax::Error>) -> Option<Errno> {
    result.expect_err("succeeded").errno()
}

#[test]
fn dax_errors() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", ps as usize);
    assert_eq!(errno(fs.setup_mapping(file, 0, ps, 0, false)), Errno::ENOSYS);
    assert_eq!(errno(fs.remove_mapping(0, ps)), Errno::ENOSYS);
    assert_eq!(create_errno(fs.create_dax_window(ps + 1, None)), Some(Errno::EINVAL));
    assert_eq!(create_errno(fs.create_dax_window(0, None)), Some(Errno::EINVAL));

    fs.create_dax_window(4 * ps, None).unwrap();
    assert_eq!(create_errno(fs.create_dax_window(4 * ps, None)), Some(Errno::EEXIST));
    assert_eq!(errno(fs.setup_mapping(file, 0, ps, 1, false)), Errno::EINVAL);
    assert_eq!(errno(fs.setup_mapping(file, 1, ps, 0, false)), Errno::EINVAL);
    assert_eq!(errno(fs.setup_mapping(file, 0, ps + 1, 0, false)), Errno::EINVAL);
    assert_eq!(errno(fs.setup_mapping(file, 0, 0, 0, false)), Errno::EINVAL);
    assert_eq!(errno(fs.setup_mapping(file, 0, ps, 4 * ps, false)), Errno::EINVAL);
    assert_eq!(errno(fs.setup_mapping(Inode::ROOT, 0, ps, 0, false)), Errno::EINVAL);
    assert_eq!(errno(fs.remove_mapping(0, 5 * ps)), Errno::EINVAL);
    close_and_check(fs);
}

/// Unmounting unmaps the window's files, so that the guest's faults there fail.
#[test]
fn dax_unmount_unmaps_the_window() {
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", ps as usize);
    let window = fs.create_dax_window(ps, None).unwrap();
    fs.setup_mapping(file, 0, ps, 0, true).unwrap();
    window.fault(0, Access::Write).unwrap();
    window_bytes(&window, 0, 4).copy_from_slice(b"dirt");

    let storage = fs.unmount().unwrap().unwrap();
    assert!(!window.is_mapped(0));
    assert_eq!(errno(window.fault(0, Access::Read)), Errno::ENOENT);
    assert_eq!(errno(fs.setup_mapping(file, 0, ps, 0, false)), Errno::ENOTCONN);
    drop(fs);

    // The store was written back.
    let fs = Filesystem::mount(storage.check().unwrap()).unwrap();
    let file = fs.lookup(Inode::ROOT, b"file").unwrap().inode;
    assert_eq!(&read_all(&fs, file)[..5], b"dirt\x04");
    close_and_check(fs);
}

/// A second stage recording the changes made to it.
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
    fn map(&self, offset: u64, host_addr: *mut u8, len: u64, protection: Protection) {
        let host_addr = host_addr as usize;
        self.0.lock().unwrap().push(SecondStageOp::Map { offset, host_addr, len, protection });
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        self.0.lock().unwrap().push(SecondStageOp::Protect { offset, len, protection });
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.0.lock().unwrap().push(SecondStageOp::Unmap { offset, len });
    }
}

#[test]
fn dax_second_stage_follows_the_window() {
    use Protection::{Read, ReadWrite};
    use SecondStageOp::*;
    let fs = format();
    let ps = kernel::page_size();
    let file = patterned_file(&fs, b"file", 2 * ps as usize);
    fs.fsync(file).unwrap();
    let second_stage = Arc::new(RecordingSecondStage::default());
    let window = fs.create_dax_window(4 * ps, Some(second_stage.clone())).unwrap();
    let map = |offset: u64, protection| Map {
        offset,
        host_addr: window.host_base() as usize + offset as usize,
        len: ps,
        protection,
    };

    // Pages are mapped as the guest faults on them, with the access's permission, together with
    // the pages after them that are in memory.
    fs.setup_mapping(file, 0, 2 * ps, ps, true).unwrap();
    assert_eq!(second_stage.take(), []);
    window.fault(ps, Access::Read).unwrap();
    assert_eq!(second_stage.take(), [map(ps, Read), map(2 * ps, Read)]);
    window.fault(ps, Access::Write).unwrap();
    assert_eq!(second_stage.take(), [Protect { offset: ps, len: ps, protection: ReadWrite }]);
    window.fault(2 * ps, Access::Write).unwrap();
    assert_eq!(second_stage.take(), [Protect { offset: 2 * ps, len: ps, protection: ReadWrite }]);

    // Writing the dirty pages back write-protects them, so that the next store dirties them again.
    window_bytes(&window, ps, 5).copy_from_slice(b"first");
    window_bytes(&window, 2 * ps, 6).copy_from_slice(b"second");
    fs.fsync(file).unwrap();
    assert_eq!(
        second_stage.take(),
        [
            Protect { offset: ps, len: ps, protection: Read },
            Protect { offset: 2 * ps, len: ps, protection: Read }
        ]
    );

    fs.remove_mapping(ps, 2 * ps).unwrap();
    assert_eq!(second_stage.take(), [Unmap { offset: ps, len: 2 * ps }]);
    close_and_check(fs);
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
    let memory = HostMemory::new(4 * kernel::page_size() as usize);
    let fs = format();
    let ps = kernel::page_size() as usize;
    let file = patterned_file(&fs, b"file", 2 * ps);
    fs.fsync(file).unwrap();
    // SAFETY: Only the test uses the memory, which outlives the filesystem.
    let window = unsafe { fs.create_dax_window_in_host_memory(memory.base, memory.size as u64) }
        .expect("create_dax_window_in_host_memory failed");
    assert_eq!(window.size(), memory.size as u64);
    fs.setup_mapping(file, 0, 2 * ps as u64, ps as u64, true).unwrap();

    // Loads fault the file's pages in, and stores to its clean pages fault to dirty them, before
    // and after writeback cleans them again.
    let mut expected: Vec<u8> = (0..2 * ps).map(|i| (i % 251) as u8).collect();
    assert_eq!(memory.bytes(ps, 2 * ps), &expected[..]);
    memory.bytes(ps + 10, 5).copy_from_slice(b"first");
    fs.fsync(file).unwrap();
    memory.bytes(ps + 20, 5).copy_from_slice(b"again");
    memory.bytes(2 * ps, 6).copy_from_slice(b"second");
    expected[10..15].copy_from_slice(b"first");
    expected[20..25].copy_from_slice(b"again");
    expected[ps..ps + 6].copy_from_slice(b"second");
    assert_eq!(read_all(&fs, file), expected);
    fs.fsync(file).unwrap();

    fs.remove_mapping(ps as u64, 2 * ps as u64).unwrap();
    let storage = close_and_check(fs);

    let fs = Filesystem::mount(storage).unwrap();
    let file = fs.lookup(Inode::ROOT, b"file").unwrap().inode;
    assert_eq!(read_all(&fs, file), expected);
    close_and_check(fs);
}

/// A write the window refuses, to a page a read-only mapping maps, is resolved as a read, as KVM's
/// asynchronous page faults fault pages in for writing whatever the guest's access. Here the kernel
/// writes into the memory, as KVM's worker does.
#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_resolves_a_refused_write_as_a_read() {
    use std::os::fd::AsRawFd;
    let ps = kernel::page_size() as usize;
    let memory = HostMemory::new(ps);
    let fs = format();
    let file = patterned_file(&fs, b"file", ps);
    // SAFETY: Only the test uses the memory, which outlives the filesystem.
    unsafe { fs.create_dax_window_in_host_memory(memory.base, ps as u64) }.unwrap();
    fs.setup_mapping(file, 0, ps as u64, 0, false).unwrap();

    // The kernel's write into the page faults it in, and then fails.
    let zero = std::fs::File::open("/dev/zero").unwrap();
    // SAFETY: Reads into the memory, whose faults the filesystem serves.
    let read = unsafe { libc::read(zero.as_raw_fd(), memory.base.as_ptr().cast(), 1) };
    assert_eq!(read, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));
    let expected: Vec<u8> = (0..ps).map(|i| (i % 251) as u8).collect();
    assert_eq!(memory.bytes(0, ps), &expected[..]);

    fs.remove_mapping(0, ps as u64).unwrap();
    close_and_check(fs);
}

#[cfg(target_os = "linux")]
#[test]
fn dax_window_in_host_memory_must_be_whole_pages() {
    let ps = kernel::page_size() as usize;
    let memory = HostMemory::new(2 * ps);
    let fs = format();
    // SAFETY: Fails before using the memory.
    let result = unsafe { fs.create_dax_window_in_host_memory(memory.base, ps as u64 + 1) };
    assert_eq!(result.expect_err("succeeded").to_string(), "DAX window memory must be whole pages");
    close_and_check(fs);
}
