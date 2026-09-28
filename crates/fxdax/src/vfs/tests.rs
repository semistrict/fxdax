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

/// `len` bytes of `window` at `offset`, as the guest sees them. The pages must have been faulted
/// in.
fn window_bytes(window: &DaxWindow, offset: u64, len: usize) -> &mut [u8] {
    // SAFETY: The caller faulted the pages in, and nothing else accesses them while the test does.
    unsafe {
        std::slice::from_raw_parts_mut((window.host_base() + offset as usize) as *mut u8, len)
    }
}

/// A file of `len` bytes, each its offset modulo 251.
fn patterned_file(vfs: &Vfs, name: &[u8], len: usize) -> u64 {
    let file = vfs.create(CREDS, ROOT_INO, name, 0o644).unwrap().ino;
    let contents: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    vfs.write(file, 0, &contents).unwrap();
    file
}

#[test]
fn dax_mapping_reads_the_file() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", 2 * ps as usize);
    let window = vfs.create_dax_window(8 * ps, None).expect("create_dax_window failed");
    assert_eq!(window.size(), 8 * ps);

    vfs.setup_mapping(file, 0, 2 * ps, false, ps).expect("setup_mapping failed");
    assert!(!window.is_mapped(0));
    assert!(window.is_mapped(ps));
    assert!(window.is_mapped(2 * ps));
    assert!(!window.is_mapped(3 * ps));

    window.fault(2 * ps, false).expect("fault failed");
    let expected: Vec<u8> = (ps as usize..2 * ps as usize).map(|i| (i % 251) as u8).collect();
    assert_eq!(window_bytes(&window, 2 * ps, ps as usize), &expected[..]);

    // The mapping is read-only.
    assert_eq!(errno(window.fault(ps, true)), linux::EACCES);
    close_and_check(vfs);
}

#[test]
fn dax_stores_reach_disk_on_fsync() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", 2 * ps as usize);
    // Write the file back, so that the store through the window is what dirties its page.
    vfs.fsync(file).unwrap();
    let window = vfs.create_dax_window(4 * ps, None).unwrap();
    vfs.setup_mapping(file, 0, 2 * ps, true, 0).expect("setup_mapping failed");

    window.fault(ps, true).expect("fault failed");
    window_bytes(&window, ps + 10, 25).copy_from_slice(b"stored through the window");
    // The file's stream sees the store, as it shares the file's pages.
    let mut buf = [0; 25];
    vfs.read(file, ps + 10, &mut buf).unwrap();
    assert_eq!(&buf, b"stored through the window");

    vfs.fsync(file).expect("fsync failed");
    vfs.remove_mapping(0, 2 * ps).expect("remove_mapping failed");
    let device = close_and_check(vfs);

    let vfs = Vfs::mount(device).unwrap();
    let file = vfs.lookup(ROOT_INO, b"file").unwrap().ino;
    let contents = read_all(&vfs, file);
    let mut expected: Vec<u8> = (0..2 * ps as usize).map(|i| (i % 251) as u8).collect();
    expected[ps as usize + 10..ps as usize + 35].copy_from_slice(b"stored through the window");
    assert_eq!(contents, expected);
    close_and_check(vfs);
}

#[test]
fn dax_mapping_is_upgraded_to_writable() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", ps as usize);
    let window = vfs.create_dax_window(4 * ps, None).unwrap();
    vfs.setup_mapping(file, 0, ps, false, 0).unwrap();
    window.fault(0, false).unwrap();
    assert_eq!(errno(window.fault(0, true)), linux::EACCES);

    // As Linux's FUSE client does on the first store to a range it mapped read-only.
    vfs.setup_mapping(file, 0, ps, true, 0).expect("setup_mapping failed");
    window.fault(0, true).expect("fault failed");
    window_bytes(&window, 0, 5).copy_from_slice(b"hello");
    let mut buf = [0; 5];
    vfs.read(file, 0, &mut buf).unwrap();
    assert_eq!(&buf, b"hello");
    close_and_check(vfs);
}

#[test]
fn dax_remove_mapping() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", 4 * ps as usize);
    let window = vfs.create_dax_window(4 * ps, None).unwrap();
    vfs.setup_mapping(file, 0, 4 * ps, true, 0).unwrap();

    // Removing part of a mapping leaves the rest.
    vfs.remove_mapping(ps, 2 * ps).expect("remove_mapping failed");
    assert!(window.is_mapped(0));
    assert!(!window.is_mapped(ps));
    assert!(!window.is_mapped(2 * ps));
    assert!(window.is_mapped(3 * ps));
    assert_eq!(errno(window.fault(ps, false)), linux::ENOENT);
    window.fault(3 * ps, true).expect("fault failed");

    vfs.remove_mapping(0, 4 * ps).expect("remove_mapping failed");
    assert!(!window.is_mapped(0));
    assert!(!window.is_mapped(3 * ps));
    close_and_check(vfs);
}

#[test]
fn dax_past_the_end_of_the_file() {
    let vfs = format();
    let ps = page_size();
    let size = ps + 100;
    let file = patterned_file(&vfs, b"file", size as usize);
    let window = vfs.create_dax_window(4 * ps, None).unwrap();
    // As Linux's FUSE client maps whole 2 MiB ranges, which extend past files' ends.
    vfs.setup_mapping(file, 0, 3 * ps, true, 0).unwrap();

    // Past the end, the guest reads zeros.
    window.fault(2 * ps, false).expect("fault failed");
    assert!(window_bytes(&window, 2 * ps, ps as usize).iter().all(|&byte| byte == 0));
    window.fault(ps, false).expect("fault failed");
    assert_eq!(window_bytes(&window, ps + 99, 1), [((ps + 99) % 251) as u8]);
    assert!(window_bytes(&window, ps + 100, ps as usize - 100).iter().all(|&byte| byte == 0));

    // A store past the end in the last page doesn't reach the file.
    window.fault(ps, true).expect("fault failed");
    window_bytes(&window, ps + 200, 4).copy_from_slice(b"lost");
    vfs.fsync(file).unwrap();
    assert_eq!(vfs.getattr(file).unwrap().size, size);
    let device = close_and_check(vfs);

    let vfs = Vfs::mount(device).unwrap();
    let file = vfs.lookup(ROOT_INO, b"file").unwrap().ino;
    assert_eq!(vfs.getattr(file).unwrap().size, size);
    vfs.setattr(file, SetAttr { size: Some(2 * ps), ..Default::default() }).unwrap();
    let mut buf = [0xff; 4];
    vfs.read(file, ps + 200, &mut buf).unwrap();
    assert_eq!(buf, [0; 4]);
    close_and_check(vfs);
}

#[test]
fn dax_mapping_keeps_an_unlinked_file() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", ps as usize);
    let window = vfs.create_dax_window(ps, None).unwrap();
    vfs.setup_mapping(file, 0, ps, false, 0).unwrap();
    vfs.unlink(ROOT_INO, b"file").unwrap();
    vfs.forget(file, 1);

    window.fault(0, false).expect("fault failed");
    assert_eq!(window_bytes(&window, 0, 3), [0, 1, 2]);
    vfs.remove_mapping(0, ps).unwrap();
    close_and_check(vfs);
}

#[test]
fn dax_errors() {
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", ps as usize);
    assert_eq!(errno(vfs.setup_mapping(file, 0, ps, false, 0)), linux::ENOSYS);
    assert_eq!(errno(vfs.remove_mapping(0, ps)), linux::ENOSYS);
    assert_eq!(errno(vfs.create_dax_window(ps + 1, None)), linux::EINVAL);

    vfs.create_dax_window(4 * ps, None).unwrap();
    assert_eq!(errno(vfs.create_dax_window(4 * ps, None)), linux::EEXIST);
    assert_eq!(errno(vfs.setup_mapping(file, 0, ps, false, 1)), linux::EINVAL);
    assert_eq!(errno(vfs.setup_mapping(file, 1, ps, false, 0)), linux::EINVAL);
    assert_eq!(errno(vfs.setup_mapping(file, 0, ps + 1, false, 0)), linux::EINVAL);
    assert_eq!(errno(vfs.setup_mapping(file, 0, 0, false, 0)), linux::EINVAL);
    assert_eq!(errno(vfs.setup_mapping(file, 0, ps, false, 4 * ps)), linux::EINVAL);
    assert_eq!(errno(vfs.setup_mapping(ROOT_INO, 0, ps, false, 0)), linux::EINVAL);
    assert_eq!(errno(vfs.remove_mapping(0, 5 * ps)), linux::EINVAL);
    close_and_check(vfs);
}

/// A second stage recording the changes made to it.
#[derive(Default)]
struct RecordingSecondStage(std::sync::Mutex<Vec<SecondStageOp>>);

#[derive(Debug, PartialEq, Eq)]
enum SecondStageOp {
    Map { offset: u64, host_addr: usize, len: u64, prot: i32 },
    Protect { offset: u64, len: u64, prot: i32 },
    Unmap { offset: u64, len: u64 },
}

impl RecordingSecondStage {
    fn take(&self) -> Vec<SecondStageOp> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl SecondStage for RecordingSecondStage {
    fn map(&self, vaddr: usize, host_addr: usize, len: usize, prot: i32) {
        let (offset, len) = (vaddr as u64, len as u64);
        self.0.lock().unwrap().push(SecondStageOp::Map { offset, host_addr, len, prot });
    }

    fn protect(&self, vaddr: usize, len: usize, prot: i32) {
        let (offset, len) = (vaddr as u64, len as u64);
        self.0.lock().unwrap().push(SecondStageOp::Protect { offset, len, prot });
    }

    fn unmap(&self, vaddr: usize, len: usize) {
        let (offset, len) = (vaddr as u64, len as u64);
        self.0.lock().unwrap().push(SecondStageOp::Unmap { offset, len });
    }
}

#[test]
fn dax_second_stage_follows_the_window() {
    use SecondStageOp::*;
    const R: i32 = libc::PROT_READ;
    const RW: i32 = libc::PROT_READ | libc::PROT_WRITE;
    let vfs = format();
    let ps = page_size();
    let file = patterned_file(&vfs, b"file", 2 * ps as usize);
    vfs.fsync(file).unwrap();
    let second_stage = Arc::new(RecordingSecondStage::default());
    let window = vfs.create_dax_window(4 * ps, Some(second_stage.clone())).unwrap();
    let map = |offset: u64, prot| Map {
        offset,
        host_addr: window.host_base() + offset as usize,
        len: ps,
        prot,
    };

    // Pages are mapped as the guest faults on them, with the access's permission, together with
    // the pages after them that are in memory.
    vfs.setup_mapping(file, 0, 2 * ps, true, ps).unwrap();
    assert_eq!(second_stage.take(), []);
    window.fault(ps, false).unwrap();
    assert_eq!(second_stage.take(), [map(ps, R), map(2 * ps, R)]);
    window.fault(ps, true).unwrap();
    assert_eq!(second_stage.take(), [Protect { offset: ps, len: ps, prot: RW }]);
    window.fault(2 * ps, true).unwrap();
    assert_eq!(second_stage.take(), [Protect { offset: 2 * ps, len: ps, prot: RW }]);

    // Writing the dirty pages back write-protects them, so that the next store dirties them again.
    window_bytes(&window, ps, 5).copy_from_slice(b"first");
    window_bytes(&window, 2 * ps, 6).copy_from_slice(b"second");
    vfs.fsync(file).unwrap();
    assert_eq!(
        second_stage.take(),
        [Protect { offset: ps, len: ps, prot: R }, Protect { offset: 2 * ps, len: ps, prot: R }]
    );

    vfs.remove_mapping(ps, 2 * ps).unwrap();
    assert_eq!(second_stage.take(), [Unmap { offset: ps, len: 2 * ps }]);
    close_and_check(vfs);
}
