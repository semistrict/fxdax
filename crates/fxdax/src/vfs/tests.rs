use super::*;
use storage_device::fake_device::FakeDevice;

const CREDS: Creds = Creds { uid: 1000, gid: 100 };

fn device() -> DeviceHolder {
    DeviceHolder::new(FakeDevice::new(16384, 512))
}

fn format() -> Vfs {
    Vfs::format(device()).expect("format failed")
}

/// Closes `vfs`, checks the filesystem and returns the device, open again.
fn close_and_check(vfs: Vfs) -> DeviceHolder {
    let device = vfs.close().expect("close failed");
    device.reopen(false);
    let device = Vfs::fsck(device).expect("fsck failed");
    device.reopen(false);
    device
}

fn errno<T: std::fmt::Debug>(result: Result<T>) -> i32 {
    result.expect_err("succeeded").errno()
}

fn names(entries: &[DirEntry]) -> Vec<(&[u8], FileKind)> {
    entries.iter().map(|entry| (entry.name.as_slice(), entry.kind)).collect()
}

fn read_all(vfs: &Vfs, ino: u64) -> Vec<u8> {
    let mut buf = vec![0; vfs.getattr(ino).unwrap().size as usize + 16];
    let len = vfs.read(ino, 0, &mut buf).expect("read failed");
    buf.truncate(len);
    buf
}

#[test]
fn root_is_a_directory() {
    let vfs = format();
    let root = vfs.getattr(ROOT_INO).expect("getattr failed");
    assert_eq!(root.ino, ROOT_INO);
    assert_eq!(root.kind, FileKind::Directory);
    assert_eq!(root.perm, 0o755);
    assert_eq!(root.mode(), 0o040755);
    close_and_check(vfs);
}

#[test]
fn create_then_lookup() {
    let vfs = format();
    let created = vfs.create(CREDS, ROOT_INO, b"file", 0o640).expect("create failed");
    assert_eq!(created.kind, FileKind::File);
    assert_eq!(created.perm, 0o640);
    assert_eq!((created.uid, created.gid), (1000, 100));
    assert_eq!(created.size, 0);
    assert_eq!(created.nlink, 1);
    assert_ne!(created.ino, ROOT_INO);

    let found = vfs.lookup(ROOT_INO, b"file").expect("lookup failed");
    assert_eq!(found, created);
    assert_eq!(errno(vfs.create(CREDS, ROOT_INO, b"file", 0o640)), linux::EEXIST);
    assert_eq!(errno(vfs.lookup(ROOT_INO, b"missing")), linux::ENOENT);
    close_and_check(vfs);
}

#[test]
fn write_then_read() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    assert_eq!(vfs.write(file, 0, b"hello, world").expect("write failed"), 12);
    assert_eq!(vfs.write(file, 7, b"fxdax").expect("write failed"), 5);
    assert_eq!(vfs.getattr(file).unwrap().size, 12);

    let mut buf = [0; 32];
    assert_eq!(vfs.read(file, 0, &mut buf).expect("read failed"), 12);
    assert_eq!(&buf[..12], b"hello, fxdax");
    assert_eq!(vfs.read(file, 7, &mut buf[..3]).expect("read failed"), 3);
    assert_eq!(&buf[..3], b"fxd");
    assert_eq!(vfs.read(file, 100, &mut buf).expect("read failed"), 0);
    assert_eq!(errno(vfs.read(ROOT_INO, 0, &mut buf)), linux::EISDIR);
    close_and_check(vfs);
}

#[test]
fn write_past_the_end_leaves_a_hole() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 100_000, b"end").expect("write failed");
    assert_eq!(vfs.getattr(file).unwrap().size, 100_003);
    let contents = read_all(&vfs, file);
    assert!(contents[..100_000].iter().all(|&byte| byte == 0));
    assert_eq!(&contents[100_000..], b"end");
    close_and_check(vfs);
}

#[test]
fn setattr_changes_size() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"0123456789").unwrap();
    let attr = vfs.setattr(file, SetAttr { size: Some(4), ..Default::default() }).unwrap();
    assert_eq!(attr.size, 4);
    assert_eq!(read_all(&vfs, file), b"0123");
    let attr = vfs.setattr(file, SetAttr { size: Some(8), ..Default::default() }).unwrap();
    assert_eq!(attr.size, 8);
    assert_eq!(read_all(&vfs, file), b"0123\0\0\0\0");
    assert_eq!(
        errno(vfs.setattr(ROOT_INO, SetAttr { size: Some(0), ..Default::default() })),
        linux::EISDIR
    );
    close_and_check(vfs);
}

#[test]
fn setattr_changes_owner_permissions_and_times() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    let attr = vfs
        .setattr(
            file,
            SetAttr {
                perm: Some(0o4755),
                uid: Some(7),
                gid: Some(8),
                atime: Some(SetTime::At(Timespec { secs: 1_000, nanos: 5 })),
                mtime: Some(SetTime::At(Timespec { secs: 2_000, nanos: 6 })),
                ..Default::default()
            },
        )
        .expect("setattr failed");
    assert_eq!(attr.perm, 0o4755);
    assert_eq!(attr.kind, FileKind::File);
    assert_eq!((attr.uid, attr.gid), (7, 8));
    assert_eq!(attr.atime, Timespec { secs: 1_000, nanos: 5 });
    assert_eq!(attr.mtime, Timespec { secs: 2_000, nanos: 6 });

    // Clearing setuid leaves setgid alone unless the file is group-executable.
    let attr = vfs.setattr(file, SetAttr { perm: Some(0o6745), ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o6745);
    let attr = vfs.setattr(file, SetAttr { kill_suid_sgid: true, ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o2745);
    let attr = vfs.setattr(file, SetAttr { perm: Some(0o6755), ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o6755);
    let attr = vfs.setattr(file, SetAttr { kill_suid_sgid: true, ..Default::default() }).unwrap();
    assert_eq!(attr.perm, 0o0755);

    let before = Timespec::now();
    let attr = vfs.setattr(file, SetAttr { mtime: Some(SetTime::Now), ..Default::default() });
    assert!(attr.unwrap().mtime.as_nanos() >= before.as_nanos());
    close_and_check(vfs);
}

#[test]
fn open_truncates() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"contents").unwrap();
    vfs.open(file, false).expect("open failed");
    assert_eq!(vfs.getattr(file).unwrap().size, 8);
    vfs.open(file, true).expect("open failed");
    assert_eq!(vfs.getattr(file).unwrap().size, 0);
    close_and_check(vfs);
}

#[test]
fn mkdir_and_rmdir() {
    let vfs = format();
    let dir = vfs.mkdir(CREDS, ROOT_INO, b"dir", 0o750).expect("mkdir failed");
    assert_eq!(dir.kind, FileKind::Directory);
    assert_eq!(dir.perm, 0o750);
    assert_eq!((dir.uid, dir.gid), (1000, 100));
    let file = vfs.create(CREDS, dir.ino, b"file", 0o644).unwrap();

    assert_eq!(errno(vfs.rmdir(ROOT_INO, b"dir")), linux::ENOTEMPTY);
    assert_eq!(errno(vfs.rmdir(dir.ino, b"file")), linux::ENOTDIR);
    vfs.unlink(dir.ino, b"file").expect("unlink failed");
    vfs.forget(file.ino, 1);
    vfs.rmdir(ROOT_INO, b"dir").expect("rmdir failed");
    assert_eq!(errno(vfs.lookup(ROOT_INO, b"dir")), linux::ENOENT);
    assert_eq!(errno(vfs.rmdir(ROOT_INO, b"dir")), linux::ENOENT);
    vfs.forget(dir.ino, 1);
    close_and_check(vfs);
}

#[test]
fn unlink() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"contents").unwrap();
    vfs.unlink(ROOT_INO, b"file").expect("unlink failed");
    assert_eq!(errno(vfs.lookup(ROOT_INO, b"file")), linux::ENOENT);
    assert_eq!(errno(vfs.unlink(ROOT_INO, b"file")), linux::ENOENT);

    // The kernel still holds a lookup on the file, which stays readable until it forgets it.
    assert_eq!(read_all(&vfs, file), b"contents");
    assert_eq!(vfs.getattr(file).unwrap().nlink, 0);
    vfs.forget(file, 1);
    assert_eq!(errno(vfs.getattr(file)), linux::ENOENT);
    close_and_check(vfs);
}

#[test]
fn rename_within_and_across_directories() {
    let vfs = format();
    let a = vfs.mkdir(CREDS, ROOT_INO, b"a", 0o755).unwrap().ino;
    let b = vfs.mkdir(CREDS, ROOT_INO, b"b", 0o755).unwrap().ino;
    let file = vfs.create(CREDS, a, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"contents").unwrap();

    vfs.rename(a, b"file", a, b"renamed", 0).expect("rename failed");
    assert_eq!(vfs.lookup(a, b"renamed").unwrap().ino, file);
    vfs.rename(a, b"renamed", b, b"moved", 0).expect("rename failed");
    assert_eq!(errno(vfs.lookup(a, b"renamed")), linux::ENOENT);
    let moved = vfs.lookup(b, b"moved").unwrap().ino;
    assert_eq!(moved, file);
    assert_eq!(read_all(&vfs, moved), b"contents");
    close_and_check(vfs);
}

#[test]
fn rename_replaces_unless_told_not_to() {
    let vfs = format();
    let first = vfs.create(CREDS, ROOT_INO, b"first", 0o644).unwrap().ino;
    let second = vfs.create(CREDS, ROOT_INO, b"second", 0o644).unwrap().ino;
    vfs.write(first, 0, b"first").unwrap();
    vfs.write(second, 0, b"second").unwrap();

    assert_eq!(
        errno(vfs.rename(ROOT_INO, b"first", ROOT_INO, b"second", RENAME_NOREPLACE)),
        linux::EEXIST
    );
    assert_eq!(
        errno(vfs.rename(ROOT_INO, b"first", ROOT_INO, b"second", RENAME_EXCHANGE)),
        linux::EINVAL
    );
    vfs.rename(ROOT_INO, b"first", ROOT_INO, b"third", RENAME_NOREPLACE).expect("rename failed");
    vfs.rename(ROOT_INO, b"third", ROOT_INO, b"second", 0).expect("rename failed");
    let replaced = vfs.lookup(ROOT_INO, b"second").unwrap().ino;
    assert_eq!(replaced, first);
    assert_eq!(read_all(&vfs, replaced), b"first");
    close_and_check(vfs);
}

#[test]
fn renamed_directory_has_its_new_parent() {
    let vfs = format();
    let a = vfs.mkdir(CREDS, ROOT_INO, b"a", 0o755).unwrap().ino;
    let sub = vfs.mkdir(CREDS, a, b"sub", 0o755).unwrap().ino;
    vfs.rename(a, b"sub", ROOT_INO, b"sub", 0).expect("rename failed");
    let handle = vfs.opendir(sub).expect("opendir failed");
    let entries = vfs.readdir(handle, 0).expect("readdir failed");
    assert_eq!(
        entries[..],
        [
            DirEntry { ino: sub, kind: FileKind::Directory, name: b".".to_vec() },
            DirEntry { ino: ROOT_INO, kind: FileKind::Directory, name: b"..".to_vec() },
        ]
    );
    vfs.releasedir(handle);
    close_and_check(vfs);
}

#[test]
fn symlink_and_readlink() {
    let vfs = format();
    let link = vfs.symlink(ROOT_INO, b"link", b"some/target").expect("symlink failed");
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.size, 11);
    assert_eq!(vfs.readlink(link.ino).expect("readlink failed"), b"some/target");
    assert_eq!(vfs.lookup(ROOT_INO, b"link").unwrap().kind, FileKind::Symlink);
    assert_eq!(errno(vfs.readlink(ROOT_INO)), linux::EINVAL);
    assert_eq!(errno(vfs.symlink(ROOT_INO, b"link", b"other")), linux::EEXIST);
    close_and_check(vfs);
}

#[test]
fn link() {
    let vfs = format();
    let dir = vfs.mkdir(CREDS, ROOT_INO, b"dir", 0o755).unwrap().ino;
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"contents").unwrap();

    let linked = vfs.link(file, dir, b"link").expect("link failed");
    assert_eq!(linked.ino, file);
    assert_eq!(linked.nlink, 2);
    assert_eq!(vfs.lookup(dir, b"link").unwrap().ino, file);
    vfs.unlink(ROOT_INO, b"file").unwrap();
    assert_eq!(vfs.getattr(file).unwrap().nlink, 1);
    assert_eq!(read_all(&vfs, file), b"contents");

    let symlink = vfs.symlink(ROOT_INO, b"symlink", b"target").unwrap().ino;
    assert_eq!(vfs.link(symlink, dir, b"symlink").unwrap().nlink, 2);
    assert_eq!(errno(vfs.link(dir, ROOT_INO, b"dir-link")), linux::EPERM);
    close_and_check(vfs);
}

#[test]
fn mknod_makes_special_files() {
    let vfs = format();
    let fifo = vfs.mknod(CREDS, ROOT_INO, b"fifo", 0o010640, 0).expect("mknod failed");
    assert_eq!(fifo.kind, FileKind::Fifo);
    assert_eq!(fifo.perm, 0o640);
    let device = vfs.mknod(CREDS, ROOT_INO, b"device", 0o020600, 0x0103).unwrap();
    assert_eq!(device.kind, FileKind::CharDevice);
    assert_eq!(device.rdev, 0x0103);
    let socket = vfs.mknod(CREDS, ROOT_INO, b"socket", 0o140755, 0).unwrap();
    assert_eq!(socket.kind, FileKind::Socket);
    let file = vfs.mknod(CREDS, ROOT_INO, b"file", 0o100600, 0).unwrap();
    assert_eq!(file.kind, FileKind::File);
    assert_eq!(errno(vfs.mknod(CREDS, ROOT_INO, b"dir", 0o040755, 0)), linux::EINVAL);

    assert_eq!(vfs.lookup(ROOT_INO, b"fifo").unwrap().kind, FileKind::Fifo);
    assert_eq!(vfs.lookup(ROOT_INO, b"device").unwrap().rdev, 0x0103);
    close_and_check(vfs);
}

#[test]
fn readdir() {
    let vfs = format();
    let dir = vfs.mkdir(CREDS, ROOT_INO, b"dir", 0o755).unwrap().ino;
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    let link = vfs.symlink(ROOT_INO, b"link", b"file").unwrap().ino;

    let handle = vfs.opendir(ROOT_INO).expect("opendir failed");
    let entries = vfs.readdir(handle, 0).expect("readdir failed");
    assert_eq!(
        names(&entries),
        [
            (&b"."[..], FileKind::Directory),
            (b"..", FileKind::Directory),
            (b"dir", FileKind::Directory),
            (b"file", FileKind::File),
            (b"link", FileKind::Symlink),
        ]
    );
    assert_eq!(
        entries.iter().map(|entry| entry.ino).collect::<Vec<_>>(),
        [ROOT_INO, ROOT_INO, dir, file, link]
    );
    assert_eq!(
        names(&vfs.readdir(handle, 3).unwrap()),
        [(&b"file"[..], FileKind::File), (b"link", FileKind::Symlink)]
    );
    assert_eq!(vfs.readdir(handle, 5).unwrap().len(), 0);

    // An open directory lists what it had when opened.
    vfs.create(CREDS, ROOT_INO, b"later", 0o644).unwrap();
    assert_eq!(vfs.readdir(handle, 0).unwrap().len(), 5);
    vfs.releasedir(handle);
    assert_eq!(errno(vfs.readdir(handle, 0)), linux::EBADF);
    assert_eq!(errno(vfs.opendir(file)), linux::ENOTDIR);
    close_and_check(vfs);
}

#[test]
fn readdir_of_many_entries() {
    let vfs = format();
    let names: Vec<Vec<u8>> =
        (0..500).map(|i| format!("file-with-a-long-name-{i:04}").into_bytes()).collect();
    for name in &names {
        vfs.create(CREDS, ROOT_INO, name, 0o644).unwrap();
    }
    let handle = vfs.opendir(ROOT_INO).unwrap();
    let entries = vfs.readdir(handle, 2).unwrap();
    assert_eq!(entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), names);
    vfs.releasedir(handle);
    close_and_check(vfs);
}

#[test]
fn fallocate() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.fallocate(file, 0, 0, 65536).expect("fallocate failed");
    let attr = vfs.getattr(file).unwrap();
    assert_eq!(attr.size, 65536);
    assert_eq!(attr.blocks, 128);
    // FALLOC_FL_KEEP_SIZE
    assert_eq!(errno(vfs.fallocate(file, 1, 0, 4096)), linux::ENOTSUP);
    assert_eq!(errno(vfs.fallocate(ROOT_INO, 0, 0, 4096)), linux::EISDIR);
    close_and_check(vfs);
}

#[test]
fn fsync() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"contents").unwrap();
    vfs.fsync(file).expect("fsync failed");
    vfs.fsync(ROOT_INO).expect("fsync failed");
    close_and_check(vfs);
}

#[test]
fn statfs() {
    let vfs = format();
    let before = vfs.statfs().expect("statfs failed");
    assert_eq!(before.block_size, 4096);
    assert_eq!(before.blocks, 16384 * 512 / 4096);
    assert_eq!(before.name_max, 255);
    assert!(before.blocks_free < before.blocks);

    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, &vec![0xaa; 1 << 20]).unwrap();
    vfs.fsync(file).unwrap();
    let after = vfs.statfs().unwrap();
    assert!(after.blocks_free + 256 <= before.blocks_free, "{after:?} vs {before:?}");
    close_and_check(vfs);
}

#[test]
fn contents_survive_remount() {
    let vfs = format();
    let dir = vfs.mkdir(CREDS, ROOT_INO, b"dir", 0o755).unwrap().ino;
    let file = vfs.create(CREDS, dir, b"file", 0o600).unwrap().ino;
    vfs.write(file, 0, b"persistent").unwrap();
    vfs.symlink(dir, b"link", b"file").unwrap();
    vfs.mknod(CREDS, dir, b"fifo", 0o010644, 0).unwrap();
    let device = close_and_check(vfs);

    let vfs = Vfs::mount(device).expect("mount failed");
    let dir = vfs.lookup(ROOT_INO, b"dir").unwrap().ino;
    let file = vfs.lookup(dir, b"file").unwrap();
    assert_eq!((file.perm, file.uid, file.gid), (0o600, 1000, 100));
    assert_eq!(read_all(&vfs, file.ino), b"persistent");
    let link = vfs.lookup(dir, b"link").unwrap().ino;
    assert_eq!(vfs.readlink(link).unwrap(), b"file");
    assert_eq!(vfs.lookup(dir, b"fifo").unwrap().kind, FileKind::Fifo);
    close_and_check(vfs);
}

#[test]
fn contents_survive_remount_of_an_image() {
    let image = tempfile::NamedTempFile::new().unwrap();
    image.as_file().set_len(64 << 20).unwrap();

    let vfs = Vfs::format_image(image.path()).expect("format failed");
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    vfs.write(file, 0, b"on disk").unwrap();
    // Dropping the volume unmounts it.
    drop(vfs);
    Vfs::fsck(image_device(image.path()).unwrap()).expect("fsck failed");

    let vfs = Vfs::mount_image(image.path()).expect("mount failed");
    let file = vfs.lookup(ROOT_INO, b"file").unwrap().ino;
    assert_eq!(read_all(&vfs, file), b"on disk");
    vfs.unmount().expect("unmount failed");
    assert_eq!(errno(vfs.getattr(file)), linux::ENOENT);
    drop(vfs);
    Vfs::fsck(image_device(image.path()).unwrap()).expect("fsck failed");
}

#[test]
#[should_panic(expected = "blocking call on an executor thread")]
fn stream_io_on_an_executor_thread_panics() {
    let vfs = format();
    let file = vfs.create(CREDS, ROOT_INO, b"file", 0o644).unwrap().ino;
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        vfs.read(file, 0, &mut [0; 8]).unwrap();
    });
}
