// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of the filesystem as a whole: its counts, options, freezing, unbinding, lockups, what
//! it does not do, and running out of space.

use super::*;

/// fxdax: /proc/fs/fxfs/<tag> says what the filesystem waited on and its device's requests
/// took, which the guest counts as idle (TASK-91.7): an fsync's commit and its seal among them.
pub(super) fn test_stats() {
    let before = stats();
    let mut file = fs::File::create(path("stats")).unwrap();
    file.write_all(b"counted").unwrap();
    file.sync_all().unwrap();
    let after = stats();
    let grew = |name: &str| after[name] > before[name];
    assert_eq!(
        ["device_commits", "seals", "seal_waits", "device_writes"].map(grew),
        [true; 4],
        "{before:?} then {after:?}"
    );
    // And their histograms (TASK-111.6).
    assert_eq!(
        ["device_commits_hist", "seals_hist", "seal_waits_hist"].map(grew),
        [true; 3],
        "{before:?} then {after:?}"
    );
    fs::remove_file(path("stats")).unwrap();
}

/// Freezing the filesystem and thawing it (FIFREEZE, FITHAW).
pub(super) fn test_freeze() {
    freeze_at(MNT, FIFREEZE).expect("FIFREEZE");
    // A writer waits while it is frozen, and goes on as it thaws.
    let writer = writer_waits("frozen", b"written once thawed", "frozen");
    freeze_at(MNT, FITHAW).expect("FITHAW");
    writer.join().unwrap();
    assert_eq!(fs::read(path("frozen")).unwrap(), b"written once thawed");
    fs::write(path("thawed"), b"writes go on").unwrap();
    assert_eq!(fs::read(path("thawed")).unwrap(), b"writes go on");
}

/// A writer of `data` to the file `name`, which waits, the filesystem frozen (`what` says how):
/// to be joined once it thaws.
fn writer_waits(name: &str, data: &'static [u8], what: &str) -> std::thread::JoinHandle<()> {
    let (wrote, written) = std::sync::mpsc::channel();
    let name = path(name);
    let writer = std::thread::spawn(move || {
        fs::write(name, data).unwrap();
        // Waited for no longer, once it waited.
        let _ = wrote.send(());
    });
    let waited = written.recv_timeout(std::time::Duration::from_millis(500));
    assert_eq!(waited, Err(std::sync::mpsc::RecvTimeoutError::Timeout), "wrote while {what}");
    writer
}

/// fxdax: a thaw that fails leaves the filesystem frozen, as the VFS leaves the superblock, and
/// the next goes on from the step it stopped at, never taking one twice (TASK-200): failing
/// before it tells the device it thaws (fxfs.fail_thaw_at 1), and before it renews the journal's
/// tail, the device told, and open again (2). Writers wait meanwhile, and write once it thaws,
/// durably. The device is told of the freeze, and of the thaw, once each. Fxfs took a thaw that
/// failed for done: the next let every writer in, to wait for ever at the device it held closed.
pub(super) fn test_failed_thaw_thaws_on_retry() {
    for step in [1, 2] {
        let told = device_requests("freeze");
        freeze_at(MNT, FIFREEZE).expect("FIFREEZE");
        assert_eq!(device_requests("freeze"), told + 1, "frozen, step {step}: the device told");
        {
            let _fail = Param::set("fail_thaw_at", &step.to_string());
            let failed = freeze_at(MNT, FITHAW).expect_err("a thaw made to fail");
            assert_eq!(failed.raw_os_error(), Some(libc::EIO), "step {step}: {failed}");
            assert_injected("fail_thaw_at");
        }
        let thawed_told = told + 1 + u64::from(step > 1);
        assert_eq!(device_requests("freeze"), thawed_told, "step {step}: the device told");
        let name = format!("thawed-on-retry-{step}");
        let writer = writer_waits(&name, b"written once thawed", "a thaw failed");
        freeze_at(MNT, FITHAW).expect("FITHAW after one failed");
        writer.join().unwrap();
        assert_eq!(device_requests("freeze"), told + 2, "step {step}: told of the thaw once");
        fs::File::open(path(&name)).unwrap().sync_all().unwrap();
        assert_eq!(fs::read(path(&name)).unwrap(), b"written once thawed");
        fs::remove_file(path(&name)).unwrap();
    }
}

/// The image mounted writable again as this goes, if a check that mounts it read-only failed: the
/// checks after it write.
struct MountedWritable;

impl Drop for MountedWritable {
    fn drop(&mut self) {
        let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
        let read_only = mounts.lines().any(|mount| {
            let fields: Vec<&str> = mount.split_whitespace().collect();
            fields.get(1) == Some(&MNT) && fields.get(3).is_some_and(|o| o.starts_with("ro"))
        });
        if read_only {
            // Not a panic, which in a panic's unwinding would abort before the first said why.
            let _ = freeze_at(MNT, FITHAW);
            let unmounted = unsafe { libc::umount(CString::new(MNT).unwrap().as_ptr()) };
            if unmounted != 0 || mount_with(0).is_err() {
                eprintln!("mounting {MNT} writable again: {}", std::io::Error::last_os_error());
            }
        }
    }
}

/// fxdax: a volume remounted read-only, or mounted so, freezes its filesystem as a writable one
/// does, which tells the device, and thaws it (TASK-202): the VFS freezes a read-only superblock
/// without a call to its filesystem, which waited for it for ever, and its VM was never
/// snapshotted, though fsfreeze returned 0. Remounted writable, it takes writes again; what was
/// written before reads back mounted read-only.
pub(super) fn test_read_only_freezes() {
    let freezes_and_thaws = |what: &str| {
        let told = device_requests("freeze");
        freeze_at(MNT, FIFREEZE).expect("FIFREEZE");
        assert_eq!(device_requests("freeze"), told + 1, "{what}: frozen, the device told");
        freeze_at(MNT, FITHAW).expect("FITHAW");
        assert_eq!(device_requests("freeze"), told + 2, "{what}: thawed, the device told");
    };
    let _writable = MountedWritable;
    let name = path("read-only");
    fs::write(&name, b"written before").unwrap();
    // Its inode dirty, which the remount writes back.
    fs::set_permissions(&name, fs::Permissions::from_mode(0o600)).unwrap();
    mount_with(libc::MS_REMOUNT | libc::MS_RDONLY).expect("remounting read-only");
    freezes_and_thaws("remounted read-only");
    mount_with(libc::MS_REMOUNT).expect("remounting writable");
    fs::write(&name, b"written again").unwrap();
    // The filesystem too, which writes nothing.
    unmount();
    mount_with(libc::MS_RDONLY).expect("mounting read-only");
    freezes_and_thaws("mounted read-only");
    assert_eq!(fs::read(&name).unwrap(), b"written again");
    assert_eq!(fs::metadata(&name).unwrap().permissions().mode() & 0o777, 0o600);
    unmount();
    mount();
    fs::remove_file(&name).unwrap();
}

/// Unbinding the driver from a mounted device waits for it to unmount, the filesystem working
/// meanwhile, rather than freeing what it uses; binding it again gives it back (TASK-79.48).
pub(super) fn test_unbind_waits_for_unmount() {
    use crate::extent_driver;

    let device = extent_driver::device();
    let (unbound, unbinding) = std::sync::mpsc::channel();
    let unbinder = std::thread::spawn({
        let device = device.clone();
        move || {
            extent_driver::unbind(&device);
            unbound.send(()).unwrap();
        }
    });
    let waited = unbinding.recv_timeout(std::time::Duration::from_millis(500));
    assert_eq!(waited, Err(std::sync::mpsc::RecvTimeoutError::Timeout), "unbound while mounted");
    fs::write(path("unbinding"), b"written while unbinding").unwrap();
    fsync_dir(MNT);
    assert_eq!(fs::read(path("unbinding")).unwrap(), b"written while unbinding");
    // Nothing mounts a device going away.
    unmount();
    unbinder.join().unwrap();
    let refused = try_mount().unwrap_err();
    assert_eq!(refused.raw_os_error(), Some(libc::ENOENT), "{refused}");
    extent_driver::bind(&device);
    mount();
    assert_eq!(fs::read(path("unbinding")).unwrap(), b"written while unbinding");
    fs::write(path("bound"), b"written once bound again").unwrap();
    fsync_dir(MNT);
    assert_eq!(fs::read(path("bound")).unwrap(), b"written once bound again");
}

/// /proc/mounts shows the mount as it is: files mapping from the DAX window, if it has one.
pub(super) fn test_show_options(dax: bool) {
    let mounts = fs::read_to_string("/proc/mounts").unwrap();
    let line = mounts
        .lines()
        .find(|line| line.split(' ').nth(1) == Some(MNT))
        .unwrap_or_else(|| panic!("{MNT} not in /proc/mounts: {mounts}"));
    let options: Vec<&str> = line.split(' ').nth(3).unwrap().split(',').collect();
    assert_eq!(options.contains(&"dax"), dax, "{line}");
    assert!(options.contains(&"rw") && !options.contains(&"follow"), "{line}");
}

/// TASK-26.19: what programs see of what Fxfs does not do (README, "Guest Fxfs: what it does
/// not do"): FIDEDUPERANGE's status is EOPNOTSUPP, and a MAP_SYNC mapping fails with it.
pub(super) fn test_unsupported() {
    /// `_IOWR(0x94, 54, struct file_dedupe_range)`.
    const FIDEDUPERANGE: libc::c_ulong = 0xc018_9436;
    const MAP_SHARED_VALIDATE: libc::c_int = 0x03;
    const MAP_SYNC: libc::c_int = 0x80000;
    /// `struct file_dedupe_range`, with one `struct file_dedupe_range_info`.
    #[repr(C)]
    struct DedupeRange {
        src_offset: u64,
        src_length: u64,
        dest_count: u16,
        reserved1: u16,
        reserved2: u32,
        dest_fd: i64,
        dest_offset: u64,
        bytes_deduped: u64,
        status: i32,
        reserved: u32,
    }
    fs::write(path("plain"), vec![7; 4096]).unwrap();
    fs::write(path("same"), vec![7; 4096]).unwrap();
    let file = fs::File::open(path("plain")).unwrap();
    let same = fs::OpenOptions::new().read(true).write(true).open(path("same")).unwrap();
    let mut range = DedupeRange {
        src_offset: 0,
        src_length: 4096,
        dest_count: 1,
        reserved1: 0,
        reserved2: 0,
        dest_fd: same.as_raw_fd() as i64,
        dest_offset: 0,
        bytes_deduped: 0,
        status: 0,
        reserved: 0,
    };
    let ret = unsafe { libc::ioctl(file.as_raw_fd(), FIDEDUPERANGE as _, &mut range) };
    assert_eq!(ret, 0, "FIDEDUPERANGE: {}", std::io::Error::last_os_error());
    assert_eq!(range.status, -libc::EOPNOTSUPP, "FIDEDUPERANGE's status");

    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            MAP_SHARED_VALIDATE | MAP_SYNC,
            same.as_raw_fd(),
            0,
        )
    };
    assert_eq!(map, libc::MAP_FAILED);
    let refused = std::io::Error::last_os_error().raw_os_error();
    assert_eq!(refused, Some(libc::EOPNOTSUPP), "MAP_SYNC");
}

/// fxdax: a soft lockup dumps every CPU's stack, as the guest's command line asks (vm::CMDLINE),
/// which a kernel without the setting would ignore but for a line in its log (TASK-133).
pub(super) fn test_lockups_show_every_cpu() {
    let setting = fs::read_to_string("/proc/sys/kernel/softlockup_all_cpu_backtrace").unwrap();
    assert_eq!(setting, "1\n");
}

/// fxdax: the filesystem filled, each way a guest fills it, fails what doesn't fit with
/// ENOSPC, and stays mounted and whole: a panic took the guest's kernel down with it (TASK-60,
/// the sandbox team's pgbench, TASK-58). What fitted is written back, and read back as it was
/// written: in the page cache, a write reserves room as it dirties pages, and fails then if
/// the filesystem has none, where it never did and its writeback, failing, dropped the data
/// (TASK-80). Then what filled it removed, a file is written and read back.
pub(super) fn test_enospc() {
    let dir = path("full");
    fs::create_dir_all(&dir).unwrap();
    let enospc = |result: std::io::Result<()>, what: &str| {
        let err = result.expect_err(what);
        assert_eq!(err.raw_os_error(), Some(libc::ENOSPC), "{what}: {err}");
    };
    // Until a write doesn't fit, as `write` says: Err then; Ok if `write` fitted `limit`
    // times, many more than the filesystem has room for.
    let fill = |limit: usize, write: &mut dyn FnMut(usize) -> std::io::Result<()>| {
        (0..limit).try_for_each(|n| write(n))
    };
    let sync = || unsafe { libc::sync() };
    let block = vec![0x5a; 1 << 20];
    const IMAGE_MIB: usize = (IMAGE_SIZE >> 20) as usize;
    // `path`, opened to read its data as the filesystem has it, its pages written back
    // dropped from the cache: those writeback failed to write are dropped too.
    let stored = |path: &str| {
        let file = fs::File::open(path).unwrap();
        let fd = file.as_raw_fd();
        assert_eq!(unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) }, 0);
        file
    };
    let read_all = |mut file: fs::File| {
        let mut read = Vec::new();
        file.read_to_end(&mut read).unwrap();
        read
    };

    // A file written a MiB at a time, whose writes that fitted fsync, and read back.
    let big = format!("{dir}/big");
    let mut file = fs::File::create(&big).unwrap();
    enospc(fill(IMAGE_MIB * 4, &mut |_| file.write_all(&block)), "a file's writes");
    let fitted = file.metadata().unwrap().len();
    assert!(fitted > (IMAGE_SIZE / 2), "{fitted} bytes fitted");
    file.sync_all().unwrap();
    drop(file);
    let mut read = stored(&big);
    let mut at = 0;
    let mut chunk = vec![0; block.len()];
    loop {
        let n = read.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        assert!(chunk[..n] == block[..n], "the file's data at {at} as written");
        at += n as u64;
    }
    assert_eq!(at, fitted);
    // Open, it would keep its room, unlinked.
    drop(read);
    fs::remove_file(&big).unwrap();
    sync();

    // Files of a block each, as many as fit.
    let mut files = 0;
    enospc(
        fill(IMAGE_MIB * 1024, &mut |n| {
            fs::write(format!("{dir}/f{n}"), &block[..4096])?;
            files = n + 1;
            Ok(())
        }),
        "small files",
    );
    assert!(files > 1000, "{files} small files fitted");
    sync();
    for n in 0..files {
        assert!(read_all(stored(&format!("{dir}/f{n}"))) == block[..4096], "small file {n}");
    }
    // Room for a file of many extents, then truncated while full: as a truncate of
    // PostgreSQL's relations, which panicked (TASK-58).
    for n in 0..files / 2 {
        fs::remove_file(format!("{dir}/f{n}")).unwrap();
    }
    sync();
    let extents = format!("{dir}/extents");
    let file = fs::OpenOptions::new().create(true).write(true).open(&extents).unwrap();
    let mut blocks = 0;
    enospc(
        fill(IMAGE_MIB * 1024, &mut |n| {
            std::os::unix::fs::FileExt::write_all_at(&file, &block[..4096], 2 * n as u64 * 4096)?;
            blocks = n + 1;
            Ok(())
        }),
        "a file of many extents",
    );
    file.sync_all().unwrap();
    let read = stored(&extents);
    let mut data = vec![0; 4096];
    for n in 0..blocks {
        std::os::unix::fs::FileExt::read_exact_at(&read, &mut data, 2 * n as u64 * 4096).unwrap();
        assert!(data == block[..4096], "block {n} of {blocks}");
    }
    drop(read);
    // Full, a directory doesn't fit either. (Nor is fallocate checked: it reserves no space,
    // which an extent store can't, fxfs_vfs_fallocate.)
    enospc(fill(IMAGE_MIB * 1024, &mut |n| fs::create_dir(format!("{dir}/d{n}"))), "directories");
    file.set_len(0).unwrap();
    drop(file);
    sync();

    // All of it gone, the filesystem is as it was.
    fs::remove_dir_all(&dir).unwrap();
    sync();
    let again = vec![0xa5; 8 << 20];
    fs::write(path("after-full"), &again).unwrap();
    evict(&path("after-full"));
    assert!(fs::read(path("after-full")).unwrap() == again, "read back after full");
    fs::remove_file(path("after-full")).unwrap();
}
