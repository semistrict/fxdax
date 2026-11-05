// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of names: links, renames, unnamed files, delayed unlinks, removed directories and
//! file handles.

use super::*;

pub(super) fn test_namespace() {
    fs::create_dir(path("dir")).unwrap();
    fs::write(path("dir/tmp"), b"renamed").unwrap();
    fs::rename(path("dir/tmp"), path("dir/renamed")).unwrap();
    fs::hard_link(path("dir/renamed"), path("dir/link")).unwrap();
    assert_eq!(fs::metadata(path("dir/renamed")).unwrap().nlink(), 2);
    std::os::unix::fs::symlink("written", path("sym")).unwrap();
    assert_eq!(fs::read(path("sym")).unwrap(), WRITTEN);

    fs::write(path("unlinked"), b"gone").unwrap();
    fs::remove_file(path("unlinked")).unwrap();

    let mut names: Vec<String> = fs::read_dir(path("dir"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["link", "renamed"]);

    fs::File::open(path("dir/renamed")).unwrap().sync_all().unwrap();
    fsync_dir(&path("dir"));
}

/// O_TMPFILE: an unnamed file, which a link names, or which goes as it is closed.
pub(super) fn test_tmpfile() {
    let open_unnamed = || {
        let dir = CString::new(MNT).unwrap();
        let fd = unsafe { libc::open(dir.as_ptr(), libc::O_TMPFILE | libc::O_RDWR, 0o600) };
        assert!(fd >= 0, "O_TMPFILE: {}", std::io::Error::last_os_error());
        unsafe { <fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd) }
    };
    let mut file = open_unnamed();
    assert_eq!(file.metadata().unwrap().nlink(), 0);
    file.write_all(b"unnamed, then linked").unwrap();
    let from = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
    let to = CString::new(path("tmpfile-linked")).unwrap();
    let ret = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    assert_eq!(ret, 0, "linkat: {}", std::io::Error::last_os_error());
    assert_eq!(file.metadata().unwrap().nlink(), 1);
    drop(file);
    assert_eq!(fs::read(path("tmpfile-linked")).unwrap(), b"unnamed, then linked");

    // Never named: freed as it is closed, by the graveyard, a batch at a time: the
    // filesystem's objects, as statfs counts them, as they were.
    let objects = || {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let mnt = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::statvfs(mnt.as_ptr(), &mut stat) }, 0);
        stat.f_files
    };
    let before = objects();
    let mut file = open_unnamed();
    file.write_all(&[7; 10000]).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut read = Vec::new();
    file.read_to_end(&mut read).unwrap();
    assert_eq!(read, vec![7; 10000]);
    assert_eq!(objects(), before + 1);
    drop(file);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while objects() != before {
        assert!(std::time::Instant::now() < deadline, "the unnamed file is not freed");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Unlinks the guest's Fxfs makes later, in batches (doc-6): what reads the directory sees them
/// at once, and an fsync of the directory makes them durable.
pub(super) fn test_delayed_unlinks() {
    let dir = path("delayed");
    fs::create_dir(&dir).unwrap();
    // More than a batch, and a directory emptied by them removed.
    for i in 0..100 {
        fs::write(format!("{dir}/f{i}"), b"x").unwrap();
    }
    for i in 0..100 {
        fs::remove_file(format!("{dir}/f{i}")).unwrap();
    }
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    fs::remove_dir(&dir).unwrap();

    let kept = path("delayed-kept");
    fs::create_dir(&kept).unwrap();
    fs::write(format!("{kept}/again"), b"first").unwrap();
    fs::remove_file(format!("{kept}/again")).unwrap();
    // Read right after: the unlink is seen, and the name is free again.
    let names: Vec<_> = fs::read_dir(&kept)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(names.is_empty(), "{names:?}");
    fs::write(format!("{kept}/again"), b"recreated").unwrap();
    // A file's other name keeps it, with one link fewer.
    fs::write(format!("{kept}/gone"), b"two names").unwrap();
    fs::hard_link(format!("{kept}/gone"), format!("{kept}/linked")).unwrap();
    fs::remove_file(format!("{kept}/gone")).unwrap();
    assert_eq!(fs::metadata(format!("{kept}/linked")).unwrap().nlink(), 1);
    assert_eq!(fs::read(format!("{kept}/linked")).unwrap(), b"two names");
    // An open file unlinked stays readable until closed.
    let mut open = fs::File::open(format!("{kept}/again")).unwrap();
    fs::write(format!("{kept}/doomed"), b"open").unwrap();
    let mut doomed = fs::File::open(format!("{kept}/doomed")).unwrap();
    fs::remove_file(format!("{kept}/doomed")).unwrap();
    let mut read = Vec::new();
    doomed.read_to_end(&mut read).unwrap();
    assert_eq!(read, b"open");
    drop(doomed);
    read.clear();
    open.read_to_end(&mut read).unwrap();
    assert_eq!(read, b"recreated");
    fsync_dir(&kept);
    fsync_dir(MNT);
}

/// TASK-50.6.1: a directory removed while the kernel still holds it, with changes not written
/// back, as overlayfs leaves its work directory's: its times, which an unlink in it changed,
/// and an extended attribute (overlayfs's opaque). Fxfs removed it at once: an fsync of it, or
/// of a file unlinked from it, writing back its parent, has nothing of Fxfs's to write to, and
/// fails nothing, nor does closing it; nor does it once changed again (fchmod).
pub(super) fn test_removed_directories() {
    let removed = path("removed");
    fs::create_dir(&removed).unwrap();
    fs::write(format!("{removed}/file"), b"file").unwrap();
    let dir = fs::File::open(&removed).unwrap();
    let file = fs::File::open(format!("{removed}/file")).unwrap();
    fs::remove_file(format!("{removed}/file")).unwrap();
    set_xattr(&removed, "trusted.overlay.opaque", b"y");
    fs::remove_dir(&removed).unwrap();
    dir.sync_all().unwrap();
    file.sync_all().unwrap();
    dir.set_permissions(fs::Permissions::from_mode(0o700)).unwrap();
    dir.sync_all().unwrap();
    drop((dir, file));
    fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
    fsync_dir(MNT);
    let logged: Vec<String> = crate::kernel_log::records()
        .into_iter()
        .filter(|record| {
            record.contains("Filesystem inconsistency") || record.contains("not written back")
        })
        .collect();
    assert_eq!(logged, Vec::<String>::new());
}

/// File handles: name_to_handle_at, and open_by_handle_at with the inode cached and evicted.
pub(super) fn test_file_handles() {
    fs::write(path("handled"), b"by handle").unwrap();
    // struct file_handle: handle_bytes, handle_type, then the handle.
    let mut handle = vec![0u32; 2 + 32];
    handle[0] = 128;
    let mut mount_id = 0i32;
    let name = CString::new(path("handled")).unwrap();
    let ret = unsafe {
        libc::syscall(
            libc::SYS_name_to_handle_at,
            libc::AT_FDCWD,
            name.as_ptr(),
            handle.as_mut_ptr(),
            &mut mount_id,
            0,
        )
    };
    assert_eq!(ret, 0, "name_to_handle_at: {}", std::io::Error::last_os_error());
    let open_by_handle = |handle: &mut Vec<u32>| {
        let mount = fs::File::open(MNT).unwrap();
        let fd = unsafe {
            libc::syscall(
                libc::SYS_open_by_handle_at,
                mount.as_raw_fd(),
                handle.as_mut_ptr(),
                libc::O_RDONLY,
            )
        };
        assert!(fd >= 0, "open_by_handle_at: {}", std::io::Error::last_os_error());
        let mut file =
            unsafe { <fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd as i32) };
        let mut read = Vec::new();
        file.read_to_end(&mut read).unwrap();
        read
    };
    assert_eq!(open_by_handle(&mut handle), b"by handle");
    evict(&path("handled"));
    assert_eq!(open_by_handle(&mut handle), b"by handle");
}
