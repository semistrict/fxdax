// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Checks of the guest kernel's own Fxfs (fxfs-guest-checks), against what
// fxdax's own Fxfs on the host sees.
//
// The host creates an fxdax image, an extent store, which the VM gets as an
// extent-store device, served by fxdax's extent daemon, and boots fxdax's
// guest kernel (FXDAX_GUEST_KERNEL), which mounts it with its Fxfs;
// fxfs-guest-checks-dax with a DAX window, which Fxfs maps files' data from
// (on hosts whose pages are 4 KiB, Fxfs's blocks). The guest checks whether
// files are DAX-mapped, and exercises writes through mappings, regular writes
// and namespace operations, and evicts a file to write it back. After the VM
// exits the host reopens the image, checks it, and checks what the guest left
// behind.

use macros::{guest, host};

pub struct TestFxfsGuestChecks {
    /// Whether the device has a DAX window, and the guest maps files into it.
    pub dax: bool,
}

/// Offset of the second 2 MiB DAX chunk.
const CHUNK: usize = 2 << 20;
const MAPPED_SIZE: usize = CHUNK + (1 << 20);
const HOST_CONTENT: &[u8] = b"written by the host\n";
const WRITTEN: &[u8] = b"written through FUSE_WRITE\n";
const EVICTED: &[u8] = b"written back when its inode is evicted\n";
/// Size of the image, which is sparse.
#[cfg_attr(feature = "guest", allow(dead_code))]
const IMAGE_SIZE: u64 = 256 << 20;

/// What fxfs-guest-checks' "punched" holds, `len` long: ones, but for the hole punched and zeros
/// from `zeroed` on.
fn punched_contents(len: usize, zeroed: usize) -> Vec<u8> {
    (0..len).map(|i| u8::from(!(1000..9000).contains(&i) && i < zeroed)).collect()
}

/// Content of the guest's mmapped file at `offset`.
fn pattern(offset: usize) -> u8 {
    (offset % 251) as u8
}

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::{Attr, Filesystem, Node, Owner, Storage};
    use std::path::Path;

    fn create_image(path: &Path) -> anyhow::Result<()> {
        block_on(async {
            let fs = Filesystem::format(Storage::create(path, IMAGE_SIZE)?).await?;
            let owner = Owner { uid: 0, gid: 0 };
            let (file, _) = fs.root().create(b"from-host", 0o644, owner).await?;
            file.write(0, HOST_CONTENT).await?;
            fs.close().await?;
            Ok(())
        })
    }

    /// Checks the image at `path`, and serves it.
    fn open_image(path: &Path) -> Filesystem {
        let storage = Storage::open(path).expect("open image");
        block_on(async { Filesystem::mount(storage.check().await?).await }).expect("mount image")
    }

    fn lookup_path(fs: &Filesystem, path: &str) -> (Node, Attr) {
        let mut node = fs.root().clone();
        let mut attr = block_on(node.getattr()).unwrap();
        for name in path.split('/') {
            (node, attr) = block_on(node.lookup(name.as_bytes()))
                .unwrap_or_else(|err| panic!("host: lookup {path}: {err}"));
        }
        (node, attr)
    }

    fn read_path(fs: &Filesystem, path: &str) -> Vec<u8> {
        let (file, attr) = lookup_path(fs, path);
        let mut buf = vec![0; attr.size as usize];
        let count = block_on(file.read(0, &mut buf)).unwrap();
        buf.truncate(count);
        buf
    }

    impl Test for TestFxfsGuestChecks {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            // fxdax: the evacuation's and the fault, write and compaction checks run for seconds,
            // and filling the filesystem (`enospc`) most of a minute.
            180
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let image = test_setup.tmp_dir.join("fxdax.img");
            create_image(&image)?;
            let window = if self.dax { 1 << 29 } else { 0 };
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store("fxdax", &image, IMAGE_SIZE, window)?;
            // FXDAX_CHECKS names the checks to run, separated by commas (such as
            // `seal_race,sparse`), for a quick run: all of them otherwise.
            let only = std::env::var("FXDAX_CHECKS").unwrap_or_default();
            vm.env(&std::ffi::CString::new(format!("FXDAX_CHECKS={only}"))?);
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
            }
            // The host's checks are of what all the guest's checks wrote.
            if std::env::var_os("FXDAX_CHECKS").is_some_and(|only| !only.is_empty()) {
                return TestOutcome::Pass;
            }

            let fs = open_image(&test_setup.tmp_dir.join("fxdax.img"));

            let mapped = read_path(&fs, "dax-mapped");
            assert_eq!(mapped.len(), MAPPED_SIZE, "host: dax-mapped size");
            // The guest's Fxfs, with a window, wrote more once the file was read in as DAX.
            let dax_writes = self.dax;
            let after = CHUNK + 4096;
            for (offset, &byte) in mapped.iter().enumerate() {
                let expected = if dax_writes && offset < EVICTED.len() {
                    EVICTED[offset]
                } else if dax_writes && (after..after + WRITTEN.len()).contains(&offset) {
                    WRITTEN[offset - after]
                } else if (CHUNK..CHUNK + WRITTEN.len()).contains(&offset) {
                    WRITTEN[offset - CHUNK]
                } else {
                    pattern(offset)
                };
                assert_eq!(byte, expected, "host: dax-mapped byte {offset}");
            }

            assert_eq!(read_path(&fs, "written"), WRITTEN);
            assert_eq!(read_path(&fs, "dir/renamed"), b"renamed");
            assert_eq!(lookup_path(&fs, "dir/renamed").1.nlink, 2);
            assert_eq!(read_path(&fs, "dir/link"), b"renamed");
            assert_eq!(block_on(lookup_path(&fs, "sym").0.readlink()).unwrap(), b"written");
            assert_eq!(read_path(&fs, "truncated"), vec![b't'; 100]);
            assert!(block_on(fs.root().lookup(b"unlinked")).is_err());
            // Never fsynced: only removing the mapping wrote it back
            assert_eq!(read_path(&fs, "evicted"), EVICTED);
            // Made with O_TMPFILE, then named.
            assert_eq!(read_path(&fs, "tmpfile-linked"), b"unnamed, then linked");
            assert_eq!(lookup_path(&fs, "tmpfile-linked").1.nlink, 1);
            assert_eq!(read_path(&fs, "fallocated"), vec![0; 8192]);
            assert_eq!(read_path(&fs, "punched"), punched_contents(16000, 12000));
            // Unlinked, later, in batches, before the fsync of their directory.
            assert!(block_on(fs.root().lookup(b"delayed")).is_err(), "delayed/ remains");
            assert_eq!(read_path(&fs, "delayed-kept/again"), b"recreated");
            assert_eq!(lookup_path(&fs, "delayed-kept/linked").1.nlink, 1);
            assert!(block_on(lookup_path(&fs, "delayed-kept").0.lookup(b"gone")).is_err());

            TestOutcome::Pass
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    const MNT: &str = "/mnt/fxdax";

    fn path(name: &str) -> String {
        format!("{MNT}/{name}")
    }

    /// Mounts the image with the kernel's Fxfs.
    fn mount() {
        fs::create_dir_all(MNT).unwrap();
        let source = CString::new("fxdax").unwrap();
        let target = CString::new(MNT).unwrap();
        let fstype = CString::new("fxfs").unwrap();
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    /// Evicts `path`'s inode, for it to be read in again: written back, and its pages dropped,
    /// as an inode that has any isn't evicted (dropping every cache would drop the filesystem's
    /// metadata's too).
    fn evict(path: &str) {
        let file = fs::File::open(path).unwrap();
        file.sync_all().unwrap();
        let fd = file.as_raw_fd();
        assert_eq!(unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) }, 0);
        drop(file);
        fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
    }

    /// Unmounts the image.
    fn unmount() {
        let target = CString::new(MNT).unwrap();
        let ret = unsafe { libc::umount(target.as_ptr()) };
        assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
    }

    /// Whether the kernel accesses `path` through the DAX window.
    fn is_dax(path: &str) -> bool {
        const STATX_ATTR_DAX: u64 = 0x200000;
        // struct statx: stx_attributes at byte 8, stx_attributes_mask at 56
        let mut buf = [0u64; 32];
        let c_path = CString::new(path).unwrap();
        let ret = unsafe {
            libc::syscall(
                libc::SYS_statx,
                libc::AT_FDCWD,
                c_path.as_ptr(),
                0,
                0x7ff, // STATX_BASIC_STATS
                buf.as_mut_ptr(),
            )
        };
        assert_eq!(ret, 0, "statx: {}", std::io::Error::last_os_error());
        let (attributes, mask) = (buf[1], buf[7]);
        assert_ne!(mask & STATX_ATTR_DAX, 0, "kernel does not report DAX");
        attributes & STATX_ATTR_DAX != 0
    }

    fn fsync_dir(dir: &str) {
        fs::File::open(dir).unwrap().sync_all().unwrap();
    }

    fn test_host_file() {
        assert_eq!(fs::read(path("from-host")).unwrap(), HOST_CONTENT);
    }

    fn test_mmap_write(dax: bool) {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("dax-mapped"))
            .unwrap();
        file.set_len(MAPPED_SIZE as u64).unwrap();
        // The guest's Fxfs maps a file's data from the window as it is created; its mappings
        // outlive its inode, which is read in again.
        assert_eq!(is_dax(&path("dax-mapped")), dax, "dax-mapped's DAX attribute");

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MAPPED_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), MAPPED_SIZE) };
        for (offset, byte) in bytes.iter_mut().enumerate() {
            *byte = pattern(offset);
        }

        // A regular write while mapped is visible through the mapping
        let mut writer = &file;
        writer.seek(SeekFrom::Start(CHUNK as u64)).unwrap();
        writer.write_all(WRITTEN).unwrap();
        assert_eq!(&bytes[CHUNK..CHUNK + WRITTEN.len()], WRITTEN);

        // And writes through the mapping are visible to regular reads
        let mut reader = &file;
        let mut buf = vec![0; 16];
        reader.seek(SeekFrom::Start(100)).unwrap();
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, bytes[100..116]);

        assert_eq!(unsafe { libc::msync(map, MAPPED_SIZE, libc::MS_SYNC) }, 0);
        assert_eq!(unsafe { libc::munmap(map, MAPPED_SIZE) }, 0);
        file.sync_all().unwrap();
        drop(file);
        if dax {
            evict(&path("dax-mapped"));
            assert!(is_dax(&path("dax-mapped")), "dax-mapped isn't DAX once read in again");
            let data = fs::read(path("dax-mapped")).unwrap();
            assert_eq!(&data[CHUNK..CHUNK + WRITTEN.len()], WRITTEN);

            // Writes to it, through a mapping and with write(2), go through the window.
            let file =
                fs::OpenOptions::new().read(true).write(true).open(path("dax-mapped")).unwrap();
            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    MAPPED_SIZE,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };
            assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), MAPPED_SIZE) };
            bytes[..EVICTED.len()].copy_from_slice(EVICTED);
            let mut writer = &file;
            writer.seek(SeekFrom::Start(CHUNK as u64 + 4096)).unwrap();
            writer.write_all(WRITTEN).unwrap();
            assert_eq!(&bytes[CHUNK + 4096..CHUNK + 4096 + WRITTEN.len()], WRITTEN);
            assert_eq!(unsafe { libc::msync(map, MAPPED_SIZE, libc::MS_SYNC) }, 0);
            assert_eq!(unsafe { libc::munmap(map, MAPPED_SIZE) }, 0);
            file.sync_all().unwrap();
            drop(file);
            let data = fs::read(path("dax-mapped")).unwrap();
            assert_eq!(&data[..EVICTED.len()], EVICTED);
            assert_eq!(&data[CHUNK + 4096..CHUNK + 4096 + WRITTEN.len()], WRITTEN);
        }
    }

    /// A shared mapping read as one PMD, then written one page at a time, leaves no DAX entry
    /// behind when its file is unlinked.
    fn test_pmd_write_unlink() {
        let name = path("pmd-write-unlinked");
        let data: Vec<u8> = (0..CHUNK).map(|offset| pattern(offset)).collect();
        fs::write(&name, data).unwrap();
        evict(&name);

        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                CHUNK,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let bytes = map.cast::<u8>();
        assert_eq!(unsafe { bytes.read_volatile() }, pattern(0));
        unsafe { bytes.write_volatile(b'X') };
        assert_eq!(unsafe { libc::msync(map, CHUNK, libc::MS_SYNC) }, 0);
        file.sync_all().unwrap();
        assert_eq!(unsafe { libc::munmap(map, CHUNK) }, 0);
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    fn test_regular_write() {
        let mut file = fs::File::create(path("written")).unwrap();
        file.write_all(WRITTEN).unwrap();
        file.sync_all().unwrap();
        assert_eq!(fs::read(path("written")).unwrap(), WRITTEN);
    }

    fn test_namespace() {
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

    fn test_truncate_mapped() {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("truncated"))
            .unwrap();
        (&file).write_all(&[b't'; 10000]).unwrap();

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED);
        file.set_len(100).unwrap();
        let page = unsafe { std::slice::from_raw_parts(map.cast::<u8>(), 4096) };
        assert_eq!(page[99], b't');
        assert!(page[100..].iter().all(|&b| b == 0), "tail not zeroed");
        assert_eq!(unsafe { libc::munmap(map, 4096) }, 0);
        file.sync_all().unwrap();
    }

    fn test_eviction_writes_back() {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("evicted"))
            .unwrap();
        file.set_len(EVICTED.len() as u64).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                EVICTED.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED);
        unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), EVICTED.len()) }
            .copy_from_slice(EVICTED);
        assert_eq!(unsafe { libc::munmap(map, EVICTED.len()) }, 0);
        drop(file);

        // Evicting the inode writes it back: with DAX, by removing its mappings
        fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
        assert_eq!(fs::read(path("evicted")).unwrap(), EVICTED);
    }

    /// O_TMPFILE: an unnamed file, which a link names, or which goes as it is closed.
    fn test_tmpfile() {
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

    /// fallocate: mode 0 grows the file, FALLOC_FL_KEEP_SIZE doesn't, other modes aren't
    /// supported.
    fn test_fallocate() {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("fallocated"))
            .unwrap();
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::fallocate(fd, 0, 0, 8192) };
        assert_eq!(ret, 0, "fallocate: {}", std::io::Error::last_os_error());
        assert_eq!(file.metadata().unwrap().len(), 8192);
        assert_eq!(unsafe { libc::fallocate(fd, libc::FALLOC_FL_KEEP_SIZE, 0, 1 << 20) }, 0);
        assert_eq!(file.metadata().unwrap().len(), 8192);
        assert_eq!(fs::read(path("fallocated")).unwrap(), vec![0; 8192]);
        let collapse = libc::FALLOC_FL_COLLAPSE_RANGE;
        assert_eq!(unsafe { libc::fallocate(fd, collapse, 0, 4096) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EOPNOTSUPP));
        file.sync_all().unwrap();

        // A hole punched through parts of three pages, then a range zeroed past the end.
        let mut punched = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("punched"))
            .unwrap();
        punched.write_all(&[1; 12288]).unwrap();
        let fd = punched.as_raw_fd();
        let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
        let ret = unsafe { libc::fallocate(fd, punch, 1000, 8000) };
        assert_eq!(ret, 0, "punch: {}", std::io::Error::last_os_error());
        assert_eq!(punched.metadata().unwrap().len(), 12288);
        assert_eq!(fs::read(path("punched")).unwrap(), punched_contents(12288, 12288));
        // The page wholly within the hole is one.
        assert_eq!(unsafe { libc::lseek(fd, 0, libc::SEEK_HOLE) }, 4096);
        let ret = unsafe { libc::fallocate(fd, libc::FALLOC_FL_ZERO_RANGE, 12000, 4000) };
        assert_eq!(ret, 0, "zero range: {}", std::io::Error::last_os_error());
        assert_eq!(fs::read(path("punched")).unwrap(), punched_contents(16000, 12000));
        punched.sync_all().unwrap();
    }

    /// A file's pages dirtied through a mapping, their extent sealed by another file's fsync (which
    /// writes back nothing of this file's), then partly zeroed, by a hole punched and by a
    /// truncation, which move them: what they hold reads the same through the mapping and through
    /// reads.
    fn test_zero_mapped() {
        const SIZE: usize = 8192;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("zero-mapped"))
            .unwrap();
        (&file).write_all(&[1; SIZE]).unwrap();
        file.sync_all().unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), SIZE) };
        let seal = || {
            fs::write(path("zero-mapped-other"), b"other").unwrap();
            fs::File::open(path("zero-mapped-other")).unwrap().sync_all().unwrap();
        };
        let mut expected = vec![1; SIZE];

        bytes[100] = 2;
        bytes[5000] = 3;
        (expected[100], expected[5000]) = (2, 3);
        seal();
        let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
        let ret = unsafe { libc::fallocate(file.as_raw_fd(), punch, 1000, 1000) };
        assert_eq!(ret, 0, "punch: {}", std::io::Error::last_os_error());
        expected[1000..2000].fill(0);
        assert_eq!(&bytes[..], &expected[..]);
        assert_eq!(fs::read(path("zero-mapped")).unwrap(), expected);

        bytes[5500] = 4;
        expected[5500] = 4;
        seal();
        file.set_len(6000).unwrap();
        expected.truncate(6000);
        assert_eq!(&bytes[..6000], &expected[..]);
        assert_eq!(fs::read(path("zero-mapped")).unwrap(), expected);

        assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
        file.sync_all().unwrap();
    }

    /// fxdax: what /proc/fs/fxfs/<tag> counts (TASK-91.7), by name.
    fn stats() -> std::collections::BTreeMap<String, u64> {
        let text = fs::read_to_string("/proc/fs/fxfs/fxdax").unwrap();
        (text.lines())
            .map(|line| {
                let (name, value) = line.split_once(' ').unwrap();
                (name.to_owned(), value.parse().unwrap())
            })
            .collect()
    }

    /// fxdax: /proc/fs/fxfs/<tag> says what the filesystem waited on and its device's requests
    /// took, which the guest counts as idle (TASK-91.7): an fsync's commit and its seal among them.
    fn test_stats() {
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
        fs::remove_file(path("stats")).unwrap();
    }

    /// fxdax: writes of several of the device's requests each, written back as fsyncs elsewhere
    /// seal: the device applied a write in flight while a seal's barrier was on both sides of it,
    /// sealing the extent its first request opened, and the rest failed: "extent … is sealed", and
    /// EIO (TASK-91.1, the sandbox team's pgbench).
    fn test_writes_across_seals() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        const FILES: usize = 4;
        let stop = Arc::new(AtomicBool::new(false));
        let sealer = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut file = fs::File::create(path("sealing")).unwrap();
                while !stop.load(Ordering::Relaxed) {
                    file.write_all(b"s").unwrap();
                    file.sync_data().unwrap();
                }
            })
        };
        let block = vec![0x5a; 4 << 20];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut written = 0;
        while Instant::now() < deadline {
            let mut file = fs::File::create(path(&format!("across-{}", written % FILES))).unwrap();
            file.write_all(&block).unwrap();
            file.sync_data().unwrap();
            written += 1;
        }
        stop.store(true, Ordering::Relaxed);
        sealer.join().unwrap();
        for n in 0..FILES.min(written) {
            assert!(fs::read(path(&format!("across-{n}"))).unwrap() == block, "across-{n}");
            fs::remove_file(path(&format!("across-{n}"))).unwrap();
        }
        fs::remove_file(path("sealing")).unwrap();
    }

    /// fxdax: as a database's log: files written whole, then written again 8 KiB at a time, each
    /// write fdatasynced, by several threads at once, while another fsyncs: on DAX, a writable MAP
    /// of an extent the guest took for open was refused (EPERM) under pgbench (TASK-91.9).
    fn test_log_writes() {
        use std::os::unix::fs::FileExt;
        use std::time::{Duration, Instant};

        const WRITERS: usize = 4;
        const SIZE: usize = 16 << 20;
        const WRITE: usize = 8 << 10;
        let deadline = Instant::now() + Duration::from_secs(10);
        let writers: Vec<_> = (0..WRITERS)
            .map(|n| {
                std::thread::spawn(move || {
                    let name = path(&format!("log-{n}"));
                    let file = fs::File::create(&name).unwrap();
                    file.write_all_at(&vec![0; SIZE], 0).unwrap();
                    file.sync_all().unwrap();
                    let block = vec![n as u8 + 1; WRITE];
                    let mut offset = 0;
                    while Instant::now() < deadline {
                        file.write_all_at(&block, offset as u64).unwrap();
                        file.sync_data().unwrap();
                        offset = (offset + WRITE) % SIZE;
                    }
                    name
                })
            })
            .collect();
        let mut sealing = fs::File::create(path("log-sealing")).unwrap();
        while Instant::now() < deadline {
            sealing.write_all(b"s").unwrap();
            sealing.sync_all().unwrap();
        }
        for writer in writers {
            fs::remove_file(writer.join().unwrap()).unwrap();
        }
        fs::remove_file(path("log-sealing")).unwrap();
    }

    /// write(2)s and stores through a mapping to the same pages of a file, while another file's
    /// fsyncs seal its extents again and again: as the pages move, a write(2) in the device's gate
    /// invalidates their DAX entries, which a fault holding an entry must not wait for the gate
    /// with (the seal waits for the write(2), the write(2) for the fault): a deadlock, which the
    /// test's timeout catches. Then each byte holds what was written last, by each in turn alone.
    fn test_seal_race() {
        use std::os::unix::fs::FileExt;
        use std::sync::atomic::{AtomicBool, Ordering};

        const SIZE: usize = 64 << 10;
        const PAGE: usize = 4096;
        const ROUNDS: usize = 200;
        /// At most: each seal starts a staging extent, which stays while any of its blocks lives
        /// (partly used ones aren't compacted yet), and the image is small.
        const SEALS: usize = 300;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("seal-race"))
            .unwrap();
        (&file).write_all(&[0; SIZE]).unwrap();
        file.sync_all().unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let mapped = map as usize;
        let store = move |round: usize| {
            for page in (0..SIZE).step_by(PAGE) {
                unsafe { ((mapped + page + 1) as *mut u8).write_volatile(round as u8) };
            }
        };
        let write = |round: usize| {
            for page in (0..SIZE).step_by(PAGE) {
                file.write_all_at(&[round as u8], page as u64).unwrap();
            }
        };
        let sealing = AtomicBool::new(true);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..SEALS {
                    if !sealing.load(Ordering::Relaxed) {
                        break;
                    }
                    fs::write(path("seal-race-other"), b"other").unwrap();
                    fs::File::open(path("seal-race-other")).unwrap().sync_all().unwrap();
                }
            });
            let storer = scope.spawn(move || (0..ROUNDS).for_each(store));
            (0..ROUNDS).for_each(write);
            storer.join().unwrap();
            sealing.store(false, Ordering::Relaxed);
        });

        write(ROUNDS);
        store(ROUNDS);
        let mut expected = vec![0; SIZE];
        for page in (0..SIZE).step_by(PAGE) {
            (expected[page], expected[page + 1]) = (ROUNDS as u8, ROUNDS as u8);
        }
        let bytes = unsafe { std::slice::from_raw_parts(map.cast::<u8>(), SIZE) };
        assert_eq!(bytes, &expected[..]);
        assert_eq!(fs::read(path("seal-race")).unwrap(), expected);
        assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
        file.sync_all().unwrap();
    }

    /// A POSIX ACL xattr's value: its version, then each entry's tag, permissions and id.
    fn acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut value = 2u32.to_le_bytes().to_vec();
        for &(tag, perm, id) in entries {
            value.extend(tag.to_le_bytes());
            value.extend(perm.to_le_bytes());
            value.extend(id.to_le_bytes());
        }
        value
    }

    fn set_xattr(path: &str, name: &str, value: &[u8]) {
        let (path, name) = (CString::new(path).unwrap(), CString::new(name).unwrap());
        let ret = unsafe {
            libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
        };
        assert_eq!(ret, 0, "setxattr: {}", std::io::Error::last_os_error());
    }

    fn get_xattr(path: &str, name: &str) -> Vec<u8> {
        let (path, name) = (CString::new(path).unwrap(), CString::new(name).unwrap());
        let mut value = vec![0u8; 256];
        let len = unsafe {
            libc::getxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
        };
        assert!(len >= 0, "getxattr: {}", std::io::Error::last_os_error());
        value.truncate(len as usize);
        value
    }

    /// POSIX ACLs: set and read back, the mode following them, and a directory's default ACL
    /// inherited, in place of the umask, which applies without one.
    fn test_acls() {
        const USER_OBJ: u16 = 0x01;
        const USER: u16 = 0x02;
        const GROUP_OBJ: u16 = 0x04;
        const MASK: u16 = 0x10;
        const OTHER: u16 = 0x20;
        const UNDEFINED: u32 = u32::MAX;

        fs::write(path("acl-file"), b"acl").unwrap();
        let access = acl(&[
            (USER_OBJ, 6, UNDEFINED),
            (USER, 6, 1234),
            (GROUP_OBJ, 4, UNDEFINED),
            (MASK, 6, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        set_xattr(&path("acl-file"), "system.posix_acl_access", &access);
        assert_eq!(get_xattr(&path("acl-file"), "system.posix_acl_access"), access);
        // The group's bits are the mask's.
        assert_eq!(fs::metadata(path("acl-file")).unwrap().mode() & 0o777, 0o660);
        // The ACL is enforced: user 1234, which it names, may read; user 1236 may not.
        let may_open = |uid: u32, flags: libc::c_int| {
            let file = CString::new(path("acl-file")).unwrap();
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
            if pid == 0 {
                // In the child: drop to `uid`, try, and say how it went.
                let status = unsafe {
                    if libc::setgroups(0, std::ptr::null()) != 0
                        || libc::setresgid(uid, uid, uid) != 0
                        || libc::setresuid(uid, uid, uid) != 0
                    {
                        2
                    } else {
                        let fd = libc::open(file.as_ptr(), flags);
                        if fd >= 0 { 0 } else { 1 }
                    }
                };
                unsafe { libc::_exit(status) };
            }
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            assert!(libc::WIFEXITED(status), "child {status:#x}");
            libc::WEXITSTATUS(status)
        };
        assert_eq!(may_open(1234, libc::O_RDONLY), 0, "the user the ACL names can't read");
        assert_eq!(may_open(1236, libc::O_RDONLY), 1, "another user could read");
        assert_eq!(may_open(1234, libc::O_WRONLY), 0, "the user the ACL names can't write");
        // fxdax: a chmod takes the group's write away, and the mask's with it, so the user the
        // ACL names may no longer write (TASK-79.96): chmod left the ACL as it was.
        fs::set_permissions(path("acl-file"), fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(may_open(1234, libc::O_WRONLY), 1, "the user the ACL names can still write");
        assert_eq!(may_open(1234, libc::O_RDONLY), 0, "the user the ACL names can't read");
        let masked = acl(&[
            (USER_OBJ, 6, UNDEFINED),
            (USER, 6, 1234),
            (GROUP_OBJ, 4, UNDEFINED),
            (MASK, 4, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        assert_eq!(get_xattr(&path("acl-file"), "system.posix_acl_access"), masked);

        let old_umask = unsafe { libc::umask(0o077) };
        fs::create_dir(path("acl-dir")).unwrap();
        let default =
            acl(&[(USER_OBJ, 7, UNDEFINED), (GROUP_OBJ, 5, UNDEFINED), (OTHER, 5, UNDEFINED)]);
        set_xattr(&path("acl-dir"), "system.posix_acl_default", &default);
        fs::write(path("acl-dir/inherits"), b"x").unwrap();
        fs::write(path("umasked"), b"x").unwrap();
        unsafe { libc::umask(old_umask) };
        // The default ACL, not the umask, limits the new file's mode.
        assert_eq!(fs::metadata(path("acl-dir/inherits")).unwrap().mode() & 0o777, 0o644);
        // Without one, the umask does.
        assert_eq!(fs::metadata(path("umasked")).unwrap().mode() & 0o777, 0o600);
    }

    /// Freezing the filesystem and thawing it (FIFREEZE, FITHAW).
    fn test_freeze() {
        const FIFREEZE: libc::c_ulong = 0xc0045877;
        const FITHAW: libc::c_ulong = 0xc0045878;
        let dir = fs::File::open(MNT).unwrap();
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), FIFREEZE as _, 0) };
        assert_eq!(ret, 0, "FIFREEZE: {}", std::io::Error::last_os_error());
        // A writer waits while it is frozen, and goes on as it thaws.
        let (wrote, written) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            fs::write(path("frozen"), b"written once thawed").unwrap();
            wrote.send(()).unwrap();
        });
        let waited = written.recv_timeout(std::time::Duration::from_millis(500));
        assert_eq!(waited, Err(std::sync::mpsc::RecvTimeoutError::Timeout), "wrote while frozen");
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), FITHAW as _, 0) };
        assert_eq!(ret, 0, "FITHAW: {}", std::io::Error::last_os_error());
        writer.join().unwrap();
        assert_eq!(fs::read(path("frozen")).unwrap(), b"written once thawed");
        fs::write(path("thawed"), b"writes go on").unwrap();
        assert_eq!(fs::read(path("thawed")).unwrap(), b"writes go on");
    }

    /// /proc/mounts shows the mount as it is: files mapping from the DAX window, if it has one.
    fn test_show_options(dax: bool) {
        let mounts = fs::read_to_string("/proc/mounts").unwrap();
        let line = mounts
            .lines()
            .find(|line| line.split(' ').nth(1) == Some(MNT))
            .unwrap_or_else(|| panic!("{MNT} not in /proc/mounts: {mounts}"));
        let options: Vec<&str> = line.split(' ').nth(3).unwrap().split(',').collect();
        assert_eq!(options.contains(&"dax"), dax, "{line}");
        assert!(options.contains(&"rw") && !options.contains(&"follow"), "{line}");
    }

    /// SB_NOSEC: a file's first write skips looking for what a write removes, until it has some;
    /// then a write removes it: security.capability, and the set-user-ID bit, for a writer without
    /// CAP_FSETID.
    fn test_write_removes_privileges() {
        let file = path("privileged");
        fs::write(&file, b"before").unwrap();
        // A v2 capability: CAP_NET_RAW, effective.
        let mut capability = vec![0u8; 20];
        capability[..4].copy_from_slice(&0x0200_0001u32.to_le_bytes());
        capability[4..8].copy_from_slice(&(1u32 << 13).to_le_bytes());
        let (c_file, name) = (CString::new(file.as_str()).unwrap(), c"security.capability");
        let set = unsafe {
            libc::setxattr(c_file.as_ptr(), name.as_ptr(), capability.as_ptr().cast(), 20, 0)
        };
        assert_eq!(set, 0, "setxattr: {}", std::io::Error::last_os_error());
        fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b", after").unwrap();
        let got =
            unsafe { libc::getxattr(c_file.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        let error = std::io::Error::last_os_error().raw_os_error();
        assert_eq!((got, error), (-1, Some(libc::ENODATA)), "security.capability kept");

        // Set-user-ID, writable by anyone: written by another user, it is not any more.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o4777)).unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
        if pid == 0 {
            let dropped = unsafe { libc::setgid(1000) == 0 && libc::setuid(1000) == 0 };
            let wrote = dropped
                && fs::OpenOptions::new()
                    .append(true)
                    .open(&file)
                    .and_then(|mut file| file.write_all(b", by another"))
                    .is_ok();
            unsafe { libc::_exit(if wrote { 0 } else { 1 }) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "{status:#x}");
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, 0o777, "set-user-ID kept");
        assert_eq!(fs::read(&file).unwrap(), b"before, after, by another");
    }

    /// Unlinks the guest's Fxfs makes later, in batches (doc-6): what reads the directory sees them
    /// at once, and an fsync of the directory makes them durable.
    fn test_delayed_unlinks() {
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
    fn test_removed_directories() {
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

    /// SEEK_DATA, SEEK_HOLE and FS_IOC_FIEMAP on a sparse file: data at its start and at 1 MiB.
    fn test_sparse() {
        const MIB: u64 = 1 << 20;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path("sparse"))
            .unwrap();
        file.write_all(&[1; 4096]).unwrap();
        file.seek(SeekFrom::Start(MIB)).unwrap();
        file.write_all(&[2; 4096]).unwrap();
        file.set_len(2 * MIB).unwrap();
        let fd = file.as_raw_fd();
        let seek = |offset: u64, whence| unsafe { libc::lseek(fd, offset as i64, whence) };
        assert_eq!(seek(0, libc::SEEK_DATA), 0);
        assert_eq!(seek(0, libc::SEEK_HOLE), 4096);
        assert_eq!(seek(4096, libc::SEEK_DATA), MIB as i64);
        assert_eq!(seek(MIB, libc::SEEK_HOLE), (MIB + 4096) as i64);
        assert_eq!(seek(MIB + 4096, libc::SEEK_DATA), -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENXIO));

        // struct fiemap, then its extents: 56 bytes each.
        const FS_IOC_FIEMAP: libc::c_ulong = 0xc020660b;
        const FIEMAP_EXTENT_LAST: u32 = 0x1;
        const EXTENTS: usize = 8;
        let mut buf = vec![0u64; 4 + EXTENTS * 7];
        buf[0] = 0; // fm_start
        buf[1] = u64::MAX; // fm_length
        // fm_flags 0, fm_mapped_extents 0; fm_extent_count, fm_reserved.
        buf[3] = EXTENTS as u64;
        let ret = unsafe { libc::ioctl(fd, FS_IOC_FIEMAP as _, buf.as_mut_ptr()) };
        assert_eq!(ret, 0, "FS_IOC_FIEMAP: {}", std::io::Error::last_os_error());
        let mapped = (buf[2] >> 32) as usize;
        let extents: Vec<(u64, u64, u32)> = (0..mapped)
            .map(|i| {
                let e = &buf[4 + i * 7..4 + (i + 1) * 7];
                (e[0], e[2], e[5] as u32)
            })
            .collect();
        let logical: Vec<(u64, u64)> = extents.iter().map(|&(l, len, _)| (l, len)).collect();
        assert_eq!(logical, [(0, 4096), (MIB, 4096)]);
        assert_eq!(extents[1].2 & FIEMAP_EXTENT_LAST, FIEMAP_EXTENT_LAST);
        file.sync_all().unwrap();
    }

    /// TASK-26.19: what programs see of what Fxfs does not do (README, "Guest Fxfs: what it does
    /// not do"): FIDEDUPERANGE's status is EOPNOTSUPP, and a MAP_SYNC mapping fails with it.
    fn test_unsupported() {
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

    /// TASK-26.19.6: reflinks: FICLONE shares a file's extents with another, FICLONERANGE a range
    /// of them in place of what another held there, and copy_file_range copies by sharing them
    /// too: each reads as it should, taking none of the space a copy would, and writes to any of
    /// them, through a mapping too, leave the others as they were, read in again too.
    fn test_reflinks() {
        use std::os::unix::fs::FileExt;
        const FICLONE: libc::c_ulong = 0x4004_9409;
        /// `_IOW(0x94, 13, struct file_clone_range)`.
        const FICLONERANGE: libc::c_ulong = 0x4020_940d;
        const BLOCK: usize = 4096;
        /// `struct file_clone_range`.
        #[repr(C)]
        struct CloneRange {
            src_fd: i64,
            src_offset: u64,
            src_length: u64,
            dest_offset: u64,
        }
        let check = |ret: libc::c_long, what: &str| {
            assert_eq!(ret, 0, "{what}: {}", std::io::Error::last_os_error());
        };
        let free = || {
            let mnt = CString::new(MNT).unwrap();
            let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::statvfs(mnt.as_ptr(), &mut stat) }, 0);
            stat.f_bfree as usize * stat.f_frsize as usize
        };
        // Of 64 blocks, and a partial one at the end.
        let data: Vec<u8> = (0..64 * BLOCK + 100).map(|i| (i % 241) as u8).collect();
        let (source, clone, ranged, copy) =
            (path("reflinked"), path("reflink-clone"), path("reflink-range"), path("reflink-copy"));
        fs::write(&source, &data).unwrap();
        fs::write(&ranged, vec![0xee; 32 * BLOCK]).unwrap();
        fsync_dir(MNT);
        fs::File::open(&source).unwrap().sync_all().unwrap();
        let before = free();

        let src = fs::File::open(&source).unwrap();
        let cloned = fs::File::create(&clone).unwrap();
        check(
            unsafe { libc::ioctl(cloned.as_raw_fd(), FICLONE as _, src.as_raw_fd()) } as _,
            "FICLONE",
        );
        let ranged_file = fs::OpenOptions::new().write(true).open(&ranged).unwrap();
        let range = CloneRange {
            src_fd: src.as_raw_fd() as i64,
            src_offset: 8 * BLOCK as u64,
            src_length: 16 * BLOCK as u64,
            dest_offset: 4 * BLOCK as u64,
        };
        check(
            unsafe { libc::ioctl(ranged_file.as_raw_fd(), FICLONERANGE as _, &range) } as _,
            "FICLONERANGE",
        );
        let copied_file = fs::File::create(&copy).unwrap();
        let null = std::ptr::null_mut::<libc::loff_t>();
        let copied = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                src.as_raw_fd(),
                null,
                copied_file.as_raw_fd(),
                null,
                data.len(),
                0,
            )
        };
        assert_eq!(
            copied,
            data.len() as i64,
            "copy_file_range: {}",
            std::io::Error::last_os_error()
        );
        drop((cloned, ranged_file, copied_file));

        let mut ranged_expected = vec![0xee; 32 * BLOCK];
        ranged_expected[4 * BLOCK..20 * BLOCK].copy_from_slice(&data[8 * BLOCK..24 * BLOCK]);
        let (mut source_expected, mut clone_expected) = (data.clone(), data.clone());
        let read_all = |what: &str, source_expected: &[u8], clone_expected: &[u8]| {
            assert_eq!(fs::read(&source).unwrap(), source_expected, "{what}: the source");
            assert_eq!(fs::read(&clone).unwrap(), clone_expected, "{what}: the clone");
            assert_eq!(fs::read(&ranged).unwrap(), ranged_expected, "{what}: the range's");
            assert_eq!(fs::read(&copy).unwrap(), data, "{what}: the copy");
        };
        read_all("cloned", &source_expected, &clone_expected);
        // Three copies' worth shared: less than one copy's taken (metadata's).
        let taken = before.saturating_sub(free());
        assert!(taken < data.len(), "the clones took {taken} bytes, of a {}-byte file", data.len());

        // Writes to the source, and through a mapping of the clone.
        src.sync_all().unwrap();
        let writer = fs::OpenOptions::new().write(true).open(&source).unwrap();
        writer.write_all_at(&[0x11; BLOCK], 0).unwrap();
        source_expected[..BLOCK].fill(0x11);
        let mapped = fs::OpenOptions::new().read(true).write(true).open(&clone).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * BLOCK,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                mapped.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        for i in BLOCK..2 * BLOCK {
            unsafe { map.cast::<u8>().add(i).write_volatile(0x22) };
        }
        clone_expected[BLOCK..2 * BLOCK].fill(0x22);
        assert_eq!(unsafe { libc::msync(map, 2 * BLOCK, libc::MS_SYNC) }, 0);
        assert_eq!(unsafe { libc::munmap(map, 2 * BLOCK) }, 0);
        drop((writer, mapped, src));
        read_all("written", &source_expected, &clone_expected);
        for file in [&source, &clone, &ranged, &copy] {
            evict(file);
        }
        read_all("read in again", &source_expected, &clone_expected);
        for file in [&source, &clone, &ranged, &copy] {
            fs::remove_file(file).unwrap();
        }
    }

    /// TASK-26.19.11: O_DIRECT's reads and writes, aligned or not, past the end too, see what
    /// buffered I/O wrote, and it sees theirs.
    fn test_direct_io() {
        use std::os::unix::fs::{FileExt, OpenOptionsExt};
        let file = path("direct");
        let buffered: Vec<u8> = (0..3 * 4096).map(|i| (i % 249) as u8).collect();
        fs::write(&file, &buffered).unwrap();
        let direct = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_DIRECT)
            .open(&file)
            .unwrap();
        let mut read = vec![0; 3 * 4096];
        assert_eq!(direct.read_at(&mut read, 0).unwrap(), 3 * 4096);
        assert_eq!(read, buffered, "a direct read of what buffered I/O wrote");
        // Aligned, unaligned, and growing the file.
        direct.write_all_at(&[0xaa; 4096], 4096).unwrap();
        direct.write_all_at(b"unaligned", 100).unwrap();
        direct.write_all_at(&[0xbb; 5000], 3 * 4096 - 1000).unwrap();
        let mut expected = buffered.clone();
        expected[4096..2 * 4096].fill(0xaa);
        expected[100..109].copy_from_slice(b"unaligned");
        expected.resize(3 * 4096 + 4000, 0);
        expected[3 * 4096 - 1000..].fill(0xbb);
        assert_eq!(fs::read(&file).unwrap(), expected, "a buffered read of what O_DIRECT wrote");
        let mut read = vec![0; expected.len() + 100];
        assert_eq!(direct.read_at(&mut read, 0).unwrap(), expected.len(), "a read past the end");
        assert_eq!(&read[..expected.len()], &expected[..]);
        drop(direct);
        fs::remove_file(&file).unwrap();
    }

    const FS_IOC_GETFLAGS: libc::c_ulong = 0x80086601;
    const FS_IOC_SETFLAGS: libc::c_ulong = 0x40086602;
    const FS_IMMUTABLE_FL: libc::c_int = 0x10;
    const FS_APPEND_FL: libc::c_int = 0x20;
    const FS_NODUMP_FL: libc::c_int = 0x40;
    const FS_NOATIME_FL: libc::c_int = 0x80;

    /// `path`'s attribute flags (lsattr).
    fn get_flags(path: &str) -> libc::c_int {
        let file = fs::File::open(path).unwrap();
        let mut flags: libc::c_int = 0;
        let ret = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS as _, &mut flags) };
        assert_eq!(ret, 0, "FS_IOC_GETFLAGS {path}: {}", std::io::Error::last_os_error());
        flags
    }

    /// Sets `path`'s attribute flags (chattr).
    fn set_flags(path: &str, flags: libc::c_int) {
        let file = fs::File::open(path).unwrap();
        let ret = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_SETFLAGS as _, &flags) };
        assert_eq!(ret, 0, "FS_IOC_SETFLAGS {path}: {}", std::io::Error::last_os_error());
    }

    /// TASK-26.19.4: chattr's flags, immutable, append-only, no-dump and no-atime: lsattr reads
    /// them back, the kernel holds files to them, and Fxfs keeps them: as they are once the inode
    /// is evicted and read in again, and once the filesystem is mounted again.
    fn test_file_attributes() {
        let (immutable, append) = (path("immutable"), path("append-only"));
        fs::write(&immutable, b"kept").unwrap();
        fs::write(&append, b"first").unwrap();
        set_flags(&immutable, FS_IMMUTABLE_FL | FS_NODUMP_FL);
        set_flags(&append, FS_APPEND_FL | FS_NOATIME_FL);
        let refused = |result: std::io::Result<()>| result.err().and_then(|err| err.raw_os_error());
        let write = |path: &str| fs::OpenOptions::new().write(true).open(path).map(drop);
        assert_eq!(refused(write(&immutable)), Some(libc::EPERM), "write to immutable");
        assert_eq!(refused(fs::remove_file(&immutable)), Some(libc::EPERM), "unlink immutable");
        let renamed = fs::rename(&immutable, path("immutable-renamed"));
        assert_eq!(refused(renamed), Some(libc::EPERM), "rename immutable");
        assert_eq!(refused(write(&append)), Some(libc::EPERM), "write to append-only");
        let mut appending = fs::OpenOptions::new().append(true).open(&append).unwrap();
        appending.write_all(b", then appended").unwrap();
        assert_eq!(refused(appending.set_len(0)), Some(libc::EPERM), "truncate append-only");
        drop(appending);
        let flags = || (get_flags(&immutable), get_flags(&append));
        let set = (FS_IMMUTABLE_FL | FS_NODUMP_FL, FS_APPEND_FL | FS_NOATIME_FL);
        assert_eq!(flags(), set);
        evict(&immutable);
        evict(&append);
        assert_eq!(flags(), set, "read in again");
        unmount();
        mount();
        assert_eq!(flags(), set, "mounted again");
        assert_eq!(fs::read(&immutable).unwrap(), b"kept");
        assert_eq!(fs::read(&append).unwrap(), b"first, then appended");
        // Cleared, they are files as any other.
        set_flags(&immutable, 0);
        set_flags(&append, 0);
        fs::write(&immutable, b"changed").unwrap();
        fs::remove_file(&immutable).unwrap();
        fs::remove_file(&append).unwrap();
    }

    /// TASK-26.19.8: an idmapped mount of the filesystem (FS_ALLOW_IDMAP), mapping IDs 0 to 999 to
    /// 10000 to 10999: a file root owns is 10000's through it; one a caller of 10005 makes through
    /// it is 5's in the filesystem; and root, whom it maps to nothing, makes none (EOVERFLOW).
    fn test_idmapped_mount() {
        const OPEN_TREE_CLONE: libc::c_uint = 1;
        const MOUNT_ATTR_IDMAP: u64 = 0x0010_0000;
        const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 4;
        /// `struct mount_attr`.
        #[repr(C)]
        struct MountAttr {
            attr_set: u64,
            attr_clr: u64,
            propagation: u64,
            userns_fd: u64,
        }
        let check = |ret: libc::c_long, what: &str| {
            assert!(ret >= 0, "{what}: {}", std::io::Error::last_os_error());
            ret
        };
        let dir = path("idmapped");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::write(format!("{dir}/root's"), b"root's").unwrap();

        // A user namespace mapping 0 to 999 to 10000 to 10999: a child's, which waits in it.
        let (mut ready, mut go) = ([0; 2], [0; 2]);
        assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(go.as_mut_ptr()) }, 0);
        let child = unsafe { libc::fork() };
        if child == 0 {
            unsafe {
                let mut byte = 0u8;
                if libc::unshare(libc::CLONE_NEWUSER) != 0
                    || libc::write(ready[1], (&raw const byte).cast(), 1) != 1
                    || libc::read(go[0], (&raw mut byte).cast(), 1) != 1
                {
                    libc::_exit(1);
                }
                libc::_exit(0);
            }
        }
        let mut byte = 0u8;
        assert_eq!(unsafe { libc::read(ready[0], (&raw mut byte).cast(), 1) }, 1);
        for map in ["uid_map", "gid_map"] {
            fs::write(format!("/proc/{child}/{map}"), b"0 10000 1000\n").unwrap();
        }
        let userns = fs::File::open(format!("/proc/{child}/ns/user")).unwrap();
        let mnt = CString::new(MNT).unwrap();
        let tree = check(
            unsafe {
                libc::syscall(
                    libc::SYS_open_tree,
                    libc::AT_FDCWD,
                    mnt.as_ptr(),
                    OPEN_TREE_CLONE | libc::O_CLOEXEC as libc::c_uint,
                )
            },
            "open_tree",
        ) as libc::c_int;
        let attr = MountAttr {
            attr_set: MOUNT_ATTR_IDMAP,
            attr_clr: 0,
            propagation: 0,
            userns_fd: userns.as_raw_fd() as u64,
        };
        let empty = c"";
        check(
            unsafe {
                libc::syscall(
                    libc::SYS_mount_setattr,
                    tree,
                    empty.as_ptr(),
                    libc::AT_EMPTY_PATH,
                    &raw const attr,
                    std::mem::size_of::<MountAttr>(),
                )
            },
            "mount_setattr",
        );
        let target = "/mnt/idmapped";
        fs::create_dir_all(target).unwrap();
        let target_c = CString::new(target).unwrap();
        check(
            unsafe {
                libc::syscall(
                    libc::SYS_move_mount,
                    tree,
                    empty.as_ptr(),
                    libc::AT_FDCWD,
                    target_c.as_ptr(),
                    MOVE_MOUNT_F_EMPTY_PATH,
                )
            },
            "move_mount",
        );
        unsafe {
            libc::close(tree);
            assert_eq!(libc::write(go[1], (&raw const byte).cast(), 1), 1);
            let mut status = 0;
            assert_eq!(libc::waitpid(child, &mut status, 0), child);
            assert_eq!(status, 0, "the user namespace's child");
        }

        let through = format!("{target}/idmapped");
        assert_eq!(fs::metadata(format!("{through}/root's")).unwrap().uid(), 10000);
        let made = format!("{through}/5's");
        let maker = unsafe { libc::fork() };
        if maker == 0 {
            unsafe {
                libc::setfsgid(10005);
                libc::setfsuid(10005);
            }
            let code = match fs::write(&made, b"5's") {
                Ok(()) => 0,
                Err(err) => err.raw_os_error().unwrap_or(255),
            };
            unsafe { libc::_exit(code) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(maker, &mut status, 0) }, maker);
        assert_eq!(libc::WEXITSTATUS(status), 0, "10005's create through the idmapped mount");
        let owner = |path: &str| {
            let metadata = fs::metadata(path).unwrap();
            (metadata.uid(), metadata.gid())
        };
        assert_eq!(owner(&format!("{dir}/5's")), (5, 5), "in the filesystem");
        assert_eq!(owner(&made), (10005, 10005), "through the idmapped mount");
        let unmapped = fs::write(format!("{through}/root-made"), b"");
        assert_eq!(unmapped.err().and_then(|err| err.raw_os_error()), Some(libc::EOVERFLOW));

        let ret = unsafe { libc::umount2(target_c.as_ptr(), libc::MNT_DETACH) };
        assert_eq!(ret, 0, "umount {target}: {}", std::io::Error::last_os_error());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// TASK-26.19.8: multigrain timestamps (FS_MGTIME): a change after stat read the ctime before
    /// has a later ctime, however quickly the changes follow one another, where a clock tick's
    /// granularity would leave many the same.
    fn test_multigrain_times() {
        let file = path("multigrain");
        fs::write(&file, b"").unwrap();
        let ctime = || {
            let metadata = fs::metadata(&file).unwrap();
            (metadata.ctime(), metadata.ctime_nsec())
        };
        let mut before = ctime();
        for change in 0..200 {
            let mode = if change % 2 == 0 { 0o600 } else { 0o644 };
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
            let after = ctime();
            assert!(after > before, "change {change}: ctime {after:?}, before it {before:?}");
            before = after;
        }
        fs::remove_file(&file).unwrap();
    }

    /// TASK-26.19.10: Fxfs's cached records go under memory pressure: the superblock's shrinker
    /// counts them (FXFS_IOC_CACHED_OBJECTS) and frees them, as drop_caches (2) has it do.
    fn test_cache_shrinks() {
        /// `_IOR('x', 0x44, __u64)`.
        const FXFS_IOC_CACHED_OBJECTS: libc::c_ulong = 0x8008_7844;
        const FILES: u64 = 2000;
        let cached = || {
            let dir = fs::File::open(MNT).unwrap();
            let mut cached = 0u64;
            let ret =
                unsafe { libc::ioctl(dir.as_raw_fd(), FXFS_IOC_CACHED_OBJECTS as _, &mut cached) };
            assert_eq!(ret, 0, "FXFS_IOC_CACHED_OBJECTS: {}", std::io::Error::last_os_error());
            cached
        };
        let dir = path("cached");
        fs::create_dir(&dir).unwrap();
        for i in 0..FILES {
            fs::write(format!("{dir}/f{i}"), b"x").unwrap();
        }
        fsync_dir(&dir);
        fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
        // Each stat looks up the name and the object.
        for i in 0..FILES {
            fs::metadata(format!("{dir}/f{i}")).unwrap();
        }
        let full = cached();
        assert!(full >= 2 * FILES, "{full} records cached after {FILES} stats");
        fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
        let shrunk = cached();
        assert!(shrunk <= full / 10, "{shrunk} records cached of {full} after drop_caches");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// TASK-26.19.11: a file in the page cache is cached in large folios: of a large file read
    /// through once, most of the pages its mapping maps are compound pages' tails
    /// (/proc/kpageflags' KPF_COMPOUND_TAIL).
    fn test_large_folios() {
        use std::os::unix::fs::FileExt;
        const SIZE: usize = 8 << 20;
        const PAGE: usize = 4096;
        const PRESENT: u64 = 1 << 63;
        const PFN: u64 = (1 << 55) - 1;
        const KPF_COMPOUND_TAIL: u64 = 1 << 16;
        let file = path("large-folios");
        fs::write(&file, vec![0x5a; SIZE]).unwrap();
        evict(&file);
        assert_eq!(fs::read(&file).unwrap(), vec![0x5a; SIZE]);
        let opened = fs::File::open(&file).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                opened.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let pagemap = fs::File::open("/proc/self/pagemap").unwrap();
        let kpageflags = fs::File::open("/proc/kpageflags").unwrap();
        let read_u64 = |file: &fs::File, at: u64| {
            let mut bytes = [0; 8];
            file.read_exact_at(&mut bytes, at).unwrap();
            u64::from_le_bytes(bytes)
        };
        let mut tails = 0;
        for page in 0..SIZE / PAGE {
            assert_eq!(unsafe { map.cast::<u8>().add(page * PAGE).read_volatile() }, 0x5a);
            let entry = read_u64(&pagemap, ((map as usize / PAGE + page) * 8) as u64);
            assert_ne!(entry & PRESENT, 0, "page {page} is not mapped");
            if read_u64(&kpageflags, (entry & PFN) * 8) & KPF_COMPOUND_TAIL != 0 {
                tails += 1;
            }
        }
        assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
        assert!(
            tails >= SIZE / PAGE / 2,
            "{tails} of {} pages are large folios' tails",
            SIZE / PAGE
        );
        fs::remove_file(&file).unwrap();
    }

    /// fxdax: pages scattered across a file, written back together: every other page of a file
    /// of 8 MiB, each written by a write(2) of its own once the file is out of the page cache, so
    /// that each is a folio of its own, and then part of a page past the file's end. Writeback writes
    /// many such folios with one call to Fxfs (fxfs_write_many, TASK-92.10). Read again from the
    /// device, the file holds what each wrote.
    fn test_scattered_writeback() {
        use std::os::unix::fs::FileExt;
        const SIZE: usize = 8 << 20;
        const PAGE: usize = 4096;
        let name = path("scattered");
        fs::write(&name, vec![1; SIZE]).unwrap();
        evict(&name);
        let file = fs::OpenOptions::new().write(true).open(&name).unwrap();
        let mut expected = vec![1; SIZE];
        for page in (0..SIZE / PAGE).step_by(2) {
            let data = vec![(page % 251) as u8 + 2; PAGE];
            file.write_all_at(&data, (page * PAGE) as u64).unwrap();
            expected[page * PAGE..(page + 1) * PAGE].copy_from_slice(&data);
        }
        let tail = [0xee; 100];
        file.write_all_at(&tail, (SIZE + PAGE) as u64).unwrap();
        expected.resize(SIZE + PAGE, 0);
        expected.extend_from_slice(&tail);
        drop(file);
        evict(&name);
        assert!(fs::read(&name).unwrap() == expected, "the file differs from what was written");
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: of a large folio, writeback writes the pages written alone (TASK-91.12): a database's
    /// pages, 8 KiB each, written into a file read into large folios, which writeback wrote whole,
    /// a megabyte each. Then a store through a mapping into the same folios, which dirties them
    /// whole. Read again from the device, the file holds what each wrote.
    fn test_partial_writeback() {
        use std::os::unix::fs::FileExt;
        const SIZE: usize = 8 << 20;
        const WRITE: usize = 8 << 10;
        const AT: [usize; 3] = [(1 << 20) + (64 << 10), (3 << 20) - (4 << 10), 6 << 20];
        let name = path("partial-writeback");
        fs::write(&name, vec![1; SIZE]).unwrap();
        evict(&name);
        assert!(fs::read(&name).unwrap() == vec![1; SIZE]);
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let mut expected = vec![1; SIZE];
        let before = stats();
        for (i, at) in AT.into_iter().enumerate() {
            let data = vec![i as u8 + 2; WRITE];
            file.write_all_at(&data, at as u64).unwrap();
            expected[at..at + WRITE].copy_from_slice(&data);
        }
        file.sync_data().unwrap();
        let written = stats()["device_write_bytes"] - before["device_write_bytes"];
        assert!(written < 1 << 20, "{written} bytes written for {} written", AT.len() * WRITE);
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let stored = AT[0] + (256 << 10);
        unsafe { std::ptr::write_bytes(map.cast::<u8>().add(stored), 0x77, WRITE) };
        expected[stored..stored + WRITE].fill(0x77);
        assert_eq!(unsafe { libc::msync(map, SIZE, libc::MS_SYNC) }, 0);
        assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
        drop(file);
        evict(&name);
        assert!(fs::read(&name).unwrap() == expected, "the file differs from what was written");
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: a file's pages written again and fsynced, again and again, as a database's are: each
    /// write moves its page to the staging extent's region, a new one after every seal, and the
    /// file's range held every region its pages were ever in, so that the window filled with
    /// them (TASK-91.10). It lets go of those its pages left: as many regions as pages, at most.
    fn test_regions_pruned() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        const PAGES: usize = 16;
        const ROUNDS: usize = 400;
        let name = path("regions-pruned");
        fs::write(&name, vec![1; 1 << 20]).unwrap();
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        file.sync_all().unwrap();
        let before = stats()["window_regions"];
        let mut expected = vec![1; 1 << 20];
        for round in 0..ROUNDS {
            let at = (round % PAGES) * 2 * PAGE;
            let data = vec![(round % 251) as u8 + 2; PAGE];
            file.write_all_at(&data, at as u64).unwrap();
            expected[at..at + PAGE].copy_from_slice(&data);
            file.sync_data().unwrap();
        }
        let after = stats();
        let regions = after["window_regions"] as i64 - before as i64;
        assert!(
            regions <= 2 * PAGES as i64,
            "{regions} regions more after {ROUNDS} fsyncs: {after:?}"
        );
        drop(file);
        assert!(fs::read(&name).unwrap() == expected, "the file differs from what was written");
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: a file's pages each written once and fsynced, as a database's rows a run updates
    /// once and its history it appends: each write moves its page to the staging extent's region,
    /// a new one after every seal, which the page, written no more, kept, until the window's
    /// regions filled and every write mapped its page alone (TASK-91.10). After each seal its
    /// pages move to the file's range's own space, and the regions they emptied go.
    fn test_regions_evacuated() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        const ROUNDS: usize = 400;
        let name = path("regions-evacuated");
        fs::write(&name, vec![1; 4 << 20]).unwrap();
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        file.sync_all().unwrap();
        let before = stats();
        let mut expected = vec![1; 4 << 20];
        for round in 0..ROUNDS {
            let at = round * 2 * PAGE;
            let data = vec![(round % 251) as u8 + 2; PAGE];
            file.write_all_at(&data, at as u64).unwrap();
            expected[at..at + PAGE].copy_from_slice(&data);
            file.sync_data().unwrap();
        }
        let after = stats();
        let regions = after["window_regions"] as i64 - before["window_regions"] as i64;
        let evacuated = after["window_maps_evacuate"] - before["window_maps_evacuate"];
        let pruned = after["window_regions_pruned"] - before["window_regions_pruned"];
        assert!(
            regions <= 32,
            "{regions} regions more after {ROUNDS} fsyncs, {evacuated} runs evacuated, {pruned} \
             regions pruned: {after:?}"
        );
        assert!(evacuated > 0, "no pages evacuated: {after:?}");
        drop(file);
        evict(&name);
        assert!(fs::read(&name).unwrap() == expected, "the file differs from what was written");
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: files written and fsynced, then truncated or removed at once, again and again: each
    /// seal's evacuation holds the files' ranges while it maps their pages, which an unlinked
    /// file's eviction, or a truncate, dropped under it (a GPF in fxfs_dax_relocate, TASK-91.10).
    /// The filesystem stays whole: the last file reads back.
    fn test_evacuate_dropped() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        for round in 0..300 {
            let name = path(&format!("evacuate-dropped-{}", round % 4));
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&name)
                .unwrap();
            file.write_all_at(&vec![1; 64 * PAGE], 0).unwrap();
            file.sync_data().unwrap();
            file.write_all_at(&vec![(round % 251) as u8 + 2; PAGE], (round % 64 * PAGE) as u64)
                .unwrap();
            file.sync_data().unwrap();
            if round % 2 == 0 {
                file.set_len(PAGE as u64).unwrap();
            } else {
                drop(file);
                fs::remove_file(&name).unwrap();
            }
        }
        let name = path("evacuate-dropped-last");
        let data = vec![0x3c; 64 * PAGE];
        fs::write(&name, &data).unwrap();
        evict(&name);
        assert!(fs::read(&name).unwrap() == data, "the last file differs from what was written");
        // The even rounds' files, truncated, are left: the odd rounds' were removed.
        for n in [0, 2] {
            fs::remove_file(path(&format!("evacuate-dropped-{n}"))).unwrap();
        }
        fs::remove_file(&name).unwrap();
    }

    /// A write(2) of a whole page over a file's data not cached, from a buffer that faults after
    /// its first 100 bytes: it writes those, and the rest of the page keeps the file's data. The
    /// page, never read as the write was to cover it, took zeros where the copy fell short
    /// (TASK-79.14).
    fn test_short_copy() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        let name = path("short-copy");
        fs::write(&name, vec![b'A'; 2 * PAGE]).unwrap();
        evict(&name);
        // Two pages, the second inaccessible: the buffer is the first's last 100 bytes, and on.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let map = map.cast::<u8>();
        unsafe { std::ptr::write_bytes(map, b'B', PAGE) };
        let second = unsafe { map.add(PAGE) }.cast();
        assert_eq!(unsafe { libc::mprotect(second, PAGE, libc::PROT_NONE) }, 0);
        let file = fs::OpenOptions::new().write(true).open(&name).unwrap();
        let buf = unsafe { map.add(PAGE - 100) };
        let written = unsafe { libc::pwrite(file.as_raw_fd(), buf.cast(), PAGE, 0) };
        assert_eq!(written, 100, "pwrite: {}", std::io::Error::last_os_error());
        assert_eq!(unsafe { libc::munmap(map.cast(), 2 * PAGE) }, 0);
        file.sync_all().unwrap();
        drop(file);
        evict(&name);
        let mut expected = vec![b'A'; 2 * PAGE];
        expected[..100].fill(b'B');
        let mut read = vec![0; 2 * PAGE];
        fs::File::open(&name).unwrap().read_exact_at(&mut read, 0).unwrap();
        assert!(read == expected, "the page's first bytes past the copy: {:?}", &read[96..110]);
        fs::remove_file(&name).unwrap();
    }

    /// An O_DIRECT read(2) of a page from a file of 100 bytes: it reads the 100. The iterator cut
    /// at the file's end was reverted past its start, a BUG() (TASK-79.15).
    fn test_direct_read_past_end() {
        use std::os::unix::fs::OpenOptionsExt;
        const PAGE: usize = 4096;
        let name = path("direct-past-end");
        fs::write(&name, vec![b'C'; 100]).unwrap();
        let file =
            fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECT).open(&name).unwrap();
        // Aligned, as O_DIRECT's buffers are.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let read = unsafe { libc::read(file.as_raw_fd(), map, PAGE) };
        assert_eq!(read, 100, "read: {}", std::io::Error::last_os_error());
        let got = unsafe { std::slice::from_raw_parts(map.cast::<u8>(), 100) }.to_vec();
        assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
        assert!(got == vec![b'C'; 100], "read {got:?}");
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: as `faults_writes_compaction`, with every fault's copy of the page it holds failing to
    /// allocate (the fxfs.dax_fail_held fault injection), as when memory is short: Fxfs keeps the
    /// page's contents through the device, whose write puts the new block's extent in the store
    /// before the guest takes it for open, and a reader mapped the extent's region read-only while
    /// a writer made it writable; the host refused the writer's stores, and the VM stopped with
    /// only KVM_RUN's EFAULT to say so (TASK-99, the sandbox team's 2 GiB DAX VM).
    fn test_faults_unheld() {
        const FAIL_HELD: &str = "/sys/module/fxfs/parameters/dax_fail_held";
        fs::write(FAIL_HELD, "Y").unwrap();
        test_faults_writes_compaction();
        fs::write(FAIL_HELD, "N").unwrap();
    }

    /// fxdax: a file's random operations, as fsx makes them (TASK-44.15): write(2)s, stores
    /// through a mapping, reads, truncations, holes punched and fsyncs, at random offsets and
    /// lengths, from a seed (FXDAX_RANDOM_OPS_SEED, or 1), each read, and every 256th operation
    /// the whole file, checked against what a model of its bytes says it holds. A mismatch names
    /// the seed, the operation and those before it, which replay it.
    fn test_random_ops() {
        const MAX: u64 = 1 << 20;
        const OPS: usize = 3000;
        const PAGE: u64 = 4096;
        let seed: u64 =
            std::env::var("FXDAX_RANDOM_OPS_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        // xorshift64*, as the operations need no more.
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |bound: u64| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D) % bound.max(1)
        };
        let name = path("random-ops");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        let fd = file.as_raw_fd();
        let mut model: Vec<u8> = Vec::new();
        let mut done: Vec<String> = Vec::new();
        let failed = |done: &[String], what: String| -> ! {
            let last = &done[done.len().saturating_sub(24)..];
            panic!(
                "seed {seed}, operation {}: {what}; the last before it:\n{}",
                done.len(),
                last.join("\n")
            );
        };
        let check = |model: &[u8], done: &[String], at: u64, len: u64| {
            let mut got = vec![0u8; len as usize];
            let n = unsafe { libc::pread(fd, got.as_mut_ptr().cast(), got.len(), at as i64) };
            if n < 0 {
                failed(done, format!("pread at {at:#x}: {}", std::io::Error::last_os_error()));
            }
            got.truncate(n as usize);
            let end = (at as usize + len as usize).min(model.len()).max(at as usize);
            let want = model.get(at as usize..end).unwrap_or(&[]);
            if got != want {
                let first = got
                    .iter()
                    .zip(want)
                    .position(|(a, b)| a != b)
                    .unwrap_or(got.len().min(want.len()));
                failed(
                    done,
                    format!(
                        "{len:#x} bytes read at {at:#x}: {} bytes, differing from the model at {:#x} ({} bytes there)",
                        got.len(),
                        at as usize + first,
                        want.len()
                    ),
                );
            }
        };
        for op in 0..OPS {
            let at = next(MAX);
            let len = 1 + next(16 << 10);
            match next(16) {
                // write(2)
                0..=4 => {
                    let data: Vec<u8> = (0..len).map(|_| next(256) as u8).collect();
                    done.push(format!("write {len:#x} at {at:#x}"));
                    let n =
                        unsafe { libc::pwrite(fd, data.as_ptr().cast(), data.len(), at as i64) };
                    if n != data.len() as isize {
                        failed(&done, format!("pwrite: {n}: {}", std::io::Error::last_os_error()));
                    }
                    let end = (at + len) as usize;
                    if model.len() < end {
                        model.resize(end, 0);
                    }
                    model[at as usize..end].copy_from_slice(&data);
                }
                // Stores through a mapping of the file, within its size.
                5..=8 if !model.is_empty() => {
                    let size = model.len() as u64;
                    let at = at % size;
                    let len = len.min(size - at);
                    done.push(format!("store {len:#x} at {at:#x}"));
                    let mapped = size.div_ceil(PAGE) * PAGE;
                    let map = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            mapped as usize,
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_SHARED,
                            fd,
                            0,
                        )
                    };
                    if map == libc::MAP_FAILED {
                        failed(&done, format!("mmap: {}", std::io::Error::last_os_error()));
                    }
                    for i in 0..len {
                        let byte = next(256) as u8;
                        unsafe { map.cast::<u8>().add((at + i) as usize).write_volatile(byte) };
                        model[(at + i) as usize] = byte;
                    }
                    unsafe { libc::munmap(map, mapped as usize) };
                }
                // Reads.
                9..=11 => {
                    done.push(format!("read {len:#x} at {at:#x}"));
                    check(&model, &done, at, len);
                }
                // A truncation, shorter or longer.
                12 => {
                    let size = next(MAX);
                    done.push(format!("truncate to {size:#x}"));
                    file.set_len(size).unwrap();
                    model.resize(size as usize, 0);
                }
                // A hole punched, the size kept.
                13 => {
                    done.push(format!("punch {len:#x} at {at:#x}"));
                    let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
                    if unsafe { libc::fallocate(fd, punch, at as i64, len as i64) } != 0 {
                        failed(&done, format!("punch: {}", std::io::Error::last_os_error()));
                    }
                    let end = ((at + len) as usize).min(model.len());
                    if (at as usize) < end {
                        model[at as usize..end].fill(0);
                    }
                }
                // fsync, a seal and a commit.
                _ => {
                    done.push("fsync".into());
                    file.sync_all().unwrap();
                }
            }
            if op % 256 == 255 {
                done.push("read the whole file".into());
                let size = file.metadata().unwrap().len();
                if size != model.len() as u64 {
                    failed(&done, format!("size {size:#x}, the model's {:#x}", model.len()));
                }
                check(&model, &done, 0, MAX + PAGE);
            }
        }
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: a file's pages rewritten and fsynced, so that they move to new blocks and the
    /// regions they were in are let go of (fxfs_dax_prune), while read(2)s of it, whose iomaps use
    /// its ranges, copy from where its pages are; another file's pages rewritten likewise beside
    /// it. A region let go of while an iomap still used it (prune's in-use check removed, a hand
    /// mutant no test killed, TASK-44.19) gives its window space to another file's extent, whose
    /// bytes the read then returns. The file's bytes are odd, the other's even: a read finds odd
    /// bytes alone.
    fn test_prune_while_read() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        const PAGES: usize = 256;
        let (odd, even) = (path("prune-odd"), path("prune-even"));
        let create = |name: &str, byte: u8| {
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(name)
                .unwrap();
            file.write_all_at(&vec![byte; PAGE * PAGES], 0).unwrap();
            file.sync_all().unwrap();
            file
        };
        let (odd_file, even_file) = (create(&odd, 1), create(&even, 2));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        std::thread::scope(|scope| {
            // Each file's pages written again, a page at a time, fsynced now and then.
            for (file, first) in [(&odd_file, 1u8), (&even_file, 2u8)] {
                scope.spawn(move || {
                    let mut n = 0usize;
                    while std::time::Instant::now() < deadline {
                        let page = (n * 37) % PAGES;
                        let byte = first.wrapping_add(2 * (n % 100) as u8);
                        file.write_all_at(&[byte; PAGE], (page * PAGE) as u64).unwrap();
                        if n % 8 == 0 {
                            file.sync_data().unwrap();
                        }
                        n += 1;
                    }
                });
            }
            // read(2)s of the odd file, which find its bytes alone.
            let readers: Vec<_> = (0..3)
                .map(|reader| {
                    let file = &odd_file;
                    scope.spawn(move || {
                        // Single-page cached reads race the same relocations as longer copies.
                        let pages = if reader == 2 { 1 } else { 16 };
                        let mut buf = vec![0u8; PAGE * pages];
                        let mut n = reader;
                        while std::time::Instant::now() < deadline {
                            let at = (n * 16) % PAGES * PAGE;
                            let len = buf.len().min(PAGE * PAGES - at);
                            file.read_exact_at(&mut buf[..len], at as u64).unwrap();
                            if let Some(i) = buf[..len].iter().position(|byte| byte % 2 == 0) {
                                return Err(format!(
                                    "byte {:#x} of the odd file read {:#x}, not the file's",
                                    at + i,
                                    buf[i]
                                ));
                            }
                            n += 1;
                        }
                        Ok(())
                    })
                })
                .collect();
            for reader in readers {
                assert_eq!(reader.join().unwrap(), Ok(()));
            }
        });
        fs::remove_file(&odd).unwrap();
        fs::remove_file(&even).unwrap();
    }

    /// fxdax: stores through a mapping of a file, write(2)s into its holes, and other files made,
    /// written and removed meanwhile, which make the journal compact, for a few seconds: a write
    /// in the gate waited for a fault's DAX entry, the fault for Fxfs, Fxfs for compaction, and
    /// compaction's seal for the write, and the filesystem hung (TASK-79.12). Each finishes.
    fn test_faults_writes_compaction() {
        use std::os::unix::fs::FileExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        const PAGE: usize = 4096;
        const SIZE: usize = 4 << 20;
        let name = path("faults-writes");
        let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
        file.set_len(SIZE as u64).unwrap();
        let stop = AtomicBool::new(false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        std::thread::scope(|scope| {
            // Stores through a mapping, a page at a time, holes and pages mapped already.
            scope.spawn(|| {
                let map = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        SIZE,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        file.as_raw_fd(),
                        0,
                    )
                };
                assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
                let mut n = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let page = (n * 7919) % (SIZE / PAGE);
                    unsafe { map.cast::<u8>().add(page * PAGE + n % PAGE).write_volatile(n as u8) };
                    n += 1;
                }
                assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
            });
            // write(2)s into the file's holes, fsynced now and then: a seal each.
            scope.spawn(|| {
                let mut n = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let page = (n * 104_729 + 3) % (SIZE / PAGE);
                    file.write_all_at(&[n as u8; PAGE], (page * PAGE) as u64).unwrap();
                    if n % 16 == 0 {
                        file.sync_data().unwrap();
                    }
                    n += 1;
                }
            });
            // Files made, written, synced and removed: the journal fills, and compacts.
            scope.spawn(|| {
                let mut n = 0usize;
                while std::time::Instant::now() < deadline {
                    let other = path(&format!("faults-writes-{}", n % 32));
                    fs::write(&other, [n as u8; 512]).unwrap();
                    if n % 8 == 0 {
                        fs::File::open(&other).unwrap().sync_all().unwrap();
                    }
                    fs::remove_file(&other).unwrap();
                    n += 1;
                }
                stop.store(true, Ordering::Relaxed);
            });
        });
        file.sync_all().unwrap();
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: a write(2) into a file's holes, in the gate, waits for the DAX entry of a read fault
    /// on the page it writes; the read fault for the file's semaphore, which a write fault holds
    /// while it seals, as Fxfs's map_write may wait for compaction's seal (the fxfs.dax_fault_seals
    /// fault injection); and the seal for the write(2): the filesystem hung (TASK-79.12). Each
    /// finishes, and the file holds what the write(2)s wrote.
    fn test_faults_seal_holding_the_semaphore() {
        use std::os::unix::fs::FileExt;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        const FAULT_SEALS: &str = "/sys/module/fxfs/parameters/dax_fault_seals";
        const PAGE: usize = 4096;
        const SIZE: usize = 32 << 20;
        // The pages the stores go to, ahead of those the write(2)s go to.
        const STORED: usize = 64;
        let name = path("faults-seal");
        let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
        file.set_len(SIZE as u64).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let map = map as usize;
        let writing = AtomicUsize::new(STORED);
        let stop = AtomicBool::new(false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        fs::write(FAULT_SEALS, "Y").unwrap();
        let written = std::thread::scope(|scope| {
            // Stores, each page's first since a seal a write fault, which seals.
            scope.spawn(|| {
                let mut n = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let at = map + (n % STORED) * PAGE + n % PAGE;
                    unsafe { (at as *mut u8).write_volatile(n as u8) };
                    n += 1;
                }
            });
            // Loads of the page the write(2)s write, faulted on anew each time.
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let at = map + writing.load(Ordering::Relaxed) * PAGE;
                    let advised = unsafe {
                        libc::madvise(at as *mut libc::c_void, PAGE, libc::MADV_DONTNEED)
                    };
                    assert_eq!(advised, 0, "madvise: {}", std::io::Error::last_os_error());
                    unsafe { (at as *const u8).read_volatile() };
                }
            });
            // write(2)s into the file's holes, a page each.
            let writer = scope.spawn(|| {
                let mut page = STORED;
                while std::time::Instant::now() < deadline && page < SIZE / PAGE {
                    writing.store(page, Ordering::Relaxed);
                    file.write_all_at(&[page as u8; PAGE], (page * PAGE) as u64).unwrap();
                    page += 1;
                }
                stop.store(true, Ordering::Relaxed);
                page
            });
            writer.join().unwrap()
        });
        fs::write(FAULT_SEALS, "N").unwrap();
        assert_eq!(unsafe { libc::munmap(map as *mut libc::c_void, SIZE) }, 0);
        let mut read = vec![0; PAGE];
        for page in STORED..written {
            file.read_exact_at(&mut read, (page * PAGE) as u64).unwrap();
            assert!(read == [page as u8; PAGE], "page {page} holds other data");
        }
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// A store through a mapping past a file's end, then the file grown over it: it reads zeros
    /// there. Writeback wrote no more than the file held, and the folio's tail, never zeroed,
    /// became the file's data (TASK-79.53).
    fn test_store_past_end() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        let name = path("store-past-end");
        fs::write(&name, vec![b'D'; 100]).unwrap();
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        unsafe { map.cast::<u8>().add(1000).write_volatile(b'X') };
        assert_eq!(unsafe { libc::msync(map, PAGE, libc::MS_SYNC) }, 0);
        file.set_len(2 * PAGE as u64).unwrap();
        let mut read = vec![0xff; 2 * PAGE];
        file.read_exact_at(&mut read, 0).unwrap();
        let mut expected = vec![0; 2 * PAGE];
        expected[..100].fill(b'D');
        assert!(read == expected, "past the old end: {:?}", &read[996..1004]);
        assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// fxdax: sub-page appends into a block already writable take no Fxfs transaction of their own
    /// (TASK-92.28). fdatasync writes the deferred size, and eviction finds all the appended data.
    fn test_append_writable() {
        use std::os::unix::fs::FileExt;
        const APPENDS: usize = 32;
        let name = path("append-writable");
        let mut expected = vec![1; 100];
        fs::write(&name, &expected).unwrap();
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        file.sync_data().unwrap();
        // The first append after the seal makes its page writable; the rest reuse it.
        file.write_all_at(&[2], expected.len() as u64).unwrap();
        expected.push(2);
        let before = stats()["dax_map_writes"];
        for n in 0..APPENDS {
            file.write_all_at(&[n as u8 + 3], expected.len() as u64).unwrap();
            expected.push(n as u8 + 3);
        }
        let after = stats();
        assert_eq!(after["dax_map_writes"], before, "appends mapped writes: {after:?}");
        file.sync_data().unwrap();
        drop(file);
        evict(&name);
        assert_eq!(fs::read(&name).unwrap(), expected);
        fs::remove_file(&name).unwrap();
    }

    /// Sustained page-sized appends allocate in batches, without publishing the reserved tail as
    /// file data. Sync, eviction, cloning, growing and shrinking preserve the written bytes.
    fn test_append_pages() {
        use std::os::unix::fs::FileExt;
        const PAGE: usize = 4096;
        const APPENDS: usize = 48;
        let name = path("append-pages");
        let file =
            fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap();
        let mut expected = vec![0x55; 100];
        file.write_all_at(&expected, 0).unwrap();
        file.sync_data().unwrap();
        // Establish an unsealed append stream, as SQLite's WAL does between barriers.
        for _ in 0..2 {
            file.write_all_at(&vec![0x33; PAGE], expected.len() as u64).unwrap();
            expected.extend_from_slice(&vec![0x33; PAGE]);
        }
        let before = stats();
        for n in 0..APPENDS {
            let bytes = vec![n as u8 + 1; PAGE];
            file.write_all_at(&bytes, expected.len() as u64).unwrap();
            expected.extend_from_slice(&bytes);
        }
        let after = stats();
        assert!(
            after["dax_map_writes"] - before["dax_map_writes"] <= APPENDS as u64 / 4,
            "page appends mapped {} writes",
            after["dax_map_writes"] - before["dax_map_writes"]
        );
        let allocated = after["dax_extent_alloc_bytes"] - before["dax_extent_alloc_bytes"];
        assert!(allocated <= 65536, "small appends allocated {allocated} bytes of extent buffers");
        assert_eq!(file.metadata().unwrap().len(), expected.len() as u64);
        assert_eq!(fs::read(&name).unwrap(), expected);
        file.sync_data().unwrap();
        drop(file);
        evict(&name);
        assert_eq!(fs::read(&name).unwrap(), expected);

        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let copy_name = path("append-pages-copy");
        let copy = fs::File::create(&copy_name).unwrap();
        const FICLONE: libc::c_ulong = 0x40049409;
        assert_eq!(unsafe { libc::ioctl(copy.as_raw_fd(), FICLONE as _, file.as_raw_fd()) }, 0);
        copy.sync_data().unwrap();
        drop(copy);
        evict(&copy_name);
        assert_eq!(fs::metadata(&copy_name).unwrap().len(), expected.len() as u64);
        assert_eq!(fs::read(&copy_name).unwrap(), expected);
        fs::remove_file(copy_name).unwrap();

        expected.resize(expected.len() + 65536, 0);
        file.set_len(expected.len() as u64).unwrap();
        assert_eq!(fs::read(&name).unwrap(), expected);
        file.set_len((PAGE + 3) as u64).unwrap();
        expected[PAGE + 3..].fill(0);
        file.set_len(expected.len() as u64).unwrap();
        file.sync_data().unwrap();
        drop(file);
        evict(&name);
        assert_eq!(fs::read(&name).unwrap(), expected);
        fs::remove_file(name).unwrap();
    }

    /// Deferred appends survive COW after another file seals the extent, and an ordinary grow
    /// or shrink uses the appended size rather than zeroing data past Fxfs's former end.
    fn test_append_size() {
        use std::os::unix::fs::FileExt;

        let seal = fs::File::create(path("append-size-seal")).unwrap();
        for operation in ["overwrite", "grow", "shrink", "clone"] {
            let name = path(&format!("append-size-{operation}"));
            let file =
                fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap();
            let mut expected = vec![0x11; 100];
            file.write_all_at(&expected, 0).unwrap();
            file.sync_data().unwrap();
            // One transaction makes the page writable, then the appends defer their size.
            for n in 0..32 {
                file.write_all_at(&[n + 1], expected.len() as u64).unwrap();
                expected.push(n + 1);
            }
            match operation {
                "overwrite" => {
                    seal.sync_all().unwrap();
                    file.write_all_at(&[0x55], 0).unwrap();
                    expected[0] = 0x55;
                }
                "grow" => {
                    file.set_len(200).unwrap();
                    expected.resize(200, 0);
                }
                "shrink" => {
                    file.set_len(120).unwrap();
                    expected.truncate(120);
                }
                "clone" => {
                    let clone_name = path("append-size-clone-copy");
                    let cloned = fs::File::create(&clone_name).unwrap();
                    const FICLONE: libc::c_ulong = 0x40049409;
                    assert_eq!(
                        unsafe { libc::ioctl(cloned.as_raw_fd(), FICLONE as _, file.as_raw_fd()) },
                        0
                    );
                    cloned.sync_data().unwrap();
                    drop(cloned);
                    evict(&clone_name);
                    assert_eq!(fs::read(&clone_name).unwrap(), expected, "cloned append");
                    fs::remove_file(clone_name).unwrap();
                }
                _ => unreachable!(),
            }
            // Read before fsync too: writing the size later cannot repair bytes zeroed by COW.
            assert_eq!(fs::read(&name).unwrap(), expected, "{operation}");
            file.sync_data().unwrap();
            drop(file);
            evict(&name);
            assert_eq!(fs::read(&name).unwrap(), expected, "{operation} after eviction");
            fs::remove_file(name).unwrap();
        }
        drop(seal);
        fs::remove_file(path("append-size-seal")).unwrap();
    }

    /// Background inode writeback must neither restore a size from before truncate nor lose
    /// the last deferred append when fdatasync returns (TASK-92.28).
    fn test_append_writeback() {
        use std::os::unix::fs::FileExt;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let name = path("append-writeback");
        let file = Arc::new(
            fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let syncer = {
            let (file, stop) = (file.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    assert_eq!(unsafe { libc::syncfs(file.as_raw_fd()) }, 0);
                }
            })
        };
        for round in 0..64 {
            file.set_len(0).unwrap();
            let mut expected = vec![round as u8; 100];
            file.write_all_at(&expected, 0).unwrap();
            for _ in 0..32 {
                file.write_all_at(&[0xa5], expected.len() as u64).unwrap();
                expected.push(0xa5);
            }
            file.set_len(120).unwrap();
            expected.truncate(120);
            file.sync_data().unwrap();
            assert_eq!(fs::read(&name).unwrap(), expected);
        }
        stop.store(true, Ordering::Relaxed);
        syncer.join().unwrap();
        drop(file);
        evict(&name);
        assert_eq!(fs::metadata(&name).unwrap().len(), 120);
        fs::remove_file(name).unwrap();
    }

    /// fxdax: a file appended to across its chunks while another thread maps it and reads it
    /// all, again and again, for a few seconds: each append writes. A fault mapped the chunk again
    /// as small as the file was, between the write making room for it and writing, which the write
    /// found too small: EIO, and a WARN (TASK-79.28).
    fn test_append_while_mapped() {
        use std::sync::atomic::{AtomicBool, Ordering};
        const STEP: usize = 192 << 10;
        let name = path("append-mapped");
        let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
        let stop = AtomicBool::new(false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let len = file.metadata().unwrap().len() as usize;
                    if len == 0 {
                        continue;
                    }
                    let map = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            len,
                            libc::PROT_READ,
                            libc::MAP_SHARED,
                            file.as_raw_fd(),
                            0,
                        )
                    };
                    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
                    for page in (0..len).step_by(4096) {
                        unsafe { map.cast::<u8>().add(page).read_volatile() };
                    }
                    assert_eq!(unsafe { libc::munmap(map, len) }, 0);
                }
            });
            // Up to 32 MiB, of the image's 256.
            let mut appender = &file;
            let mut n = 0u8;
            while std::time::Instant::now() < deadline && (n as usize) * STEP < 32 << 20 {
                appender.write_all(&vec![n; STEP]).unwrap();
                n += 1;
            }
            stop.store(true, Ordering::Relaxed);
        });
        drop(file);
        fs::remove_file(&name).unwrap();
    }

    /// File handles: name_to_handle_at, and open_by_handle_at with the inode cached and evicted.
    fn test_file_handles() {
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

    /// fxdax: the filesystem filled, each way a guest fills it, fails what doesn't fit with
    /// ENOSPC, and stays mounted and whole: a panic took the guest's kernel down with it (TASK-60,
    /// the sandbox team's pgbench, TASK-58). What fitted is written back, and read back as it was
    /// written: in the page cache, a write reserves room as it dirties pages, and fails then if
    /// the filesystem has none, where it never did and its writeback, failing, dropped the data
    /// (TASK-80). Then what filled it removed, a file is written and read back.
    fn test_enospc() {
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
            std::os::unix::fs::FileExt::read_exact_at(&read, &mut data, 2 * n as u64 * 4096)
                .unwrap();
            assert!(data == block[..4096], "block {n} of {blocks}");
        }
        drop(read);
        // Full, a directory doesn't fit either. (Nor is fallocate checked: it reserves no space,
        // which an extent store can't, fxfs_vfs_fallocate.)
        enospc(
            fill(IMAGE_MIB * 1024, &mut |n| fs::create_dir(format!("{dir}/d{n}"))),
            "directories",
        );
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

    impl Test for TestFxfsGuestChecks {
        fn in_guest(self: Box<Self>) {
            let dax = self.dax;
            mount();

            let mmap_write = || test_mmap_write(dax);
            let show_options = || test_show_options(dax);
            let mut tests: Vec<(&str, &dyn Fn())> = vec![
                ("host_file", &test_host_file),
                ("mmap_write", &mmap_write),
                ("regular_write", &test_regular_write),
                ("namespace", &test_namespace),
                ("truncate_mapped", &test_truncate_mapped),
                ("eviction_writes_back", &test_eviction_writes_back),
                ("tmpfile", &test_tmpfile),
                ("fallocate", &test_fallocate),
                ("zero_mapped", &test_zero_mapped),
                ("seal_race", &test_seal_race),
                ("writes_across_seals", &test_writes_across_seals),
                ("stats", &test_stats),
                ("log_writes", &test_log_writes),
                ("acls", &test_acls),
                ("freeze", &test_freeze),
                ("show_options", &show_options),
                ("write_removes_privileges", &test_write_removes_privileges),
                ("delayed_unlinks", &test_delayed_unlinks),
                ("sparse", &test_sparse),
                ("file_handles", &test_file_handles),
                ("unsupported", &test_unsupported),
                ("reflinks", &test_reflinks),
                ("removed_directories", &test_removed_directories),
                ("file_attributes", &test_file_attributes),
                ("idmapped_mount", &test_idmapped_mount),
                ("multigrain_times", &test_multigrain_times),
                ("cache_shrinks", &test_cache_shrinks),
                ("direct_io", &test_direct_io),
                ("random_ops", &test_random_ops),
            ];
            if dax {
                tests.push(("pmd_write_unlink", &test_pmd_write_unlink));
                tests.push(("regions_pruned", &test_regions_pruned));
                tests.push(("regions_evacuated", &test_regions_evacuated));
                tests.push(("evacuate_dropped", &test_evacuate_dropped));
                tests.push(("faults_writes_compaction", &test_faults_writes_compaction));
                tests.push(("append_writable", &test_append_writable));
                tests.push(("append_pages", &test_append_pages));
                tests.push(("append_size", &test_append_size));
                tests.push(("append_writeback", &test_append_writeback));
                tests.push(("faults_unheld", &test_faults_unheld));
                tests.push((
                    "faults_seal_holding_the_semaphore",
                    &test_faults_seal_holding_the_semaphore,
                ));
                tests.push(("append_while_mapped", &test_append_while_mapped));
                tests.push(("prune_while_read", &test_prune_while_read));
            }
            // Files on DAX are in no page cache.
            if !dax {
                tests.push(("large_folios", &test_large_folios));
                tests.push(("scattered_writeback", &test_scattered_writeback));
                tests.push(("partial_writeback", &test_partial_writeback));
                tests.push(("short_copy", &test_short_copy));
                tests.push(("direct_read_past_end", &test_direct_read_past_end));
                tests.push(("store_past_end", &test_store_past_end));
            }
            // Last: it fills the filesystem, then removes what it filled it with.
            tests.push(("enospc", &test_enospc));
            let only = std::env::var("FXDAX_CHECKS").unwrap_or_default();
            let only: Vec<&str> = only.split(',').filter(|name| !name.is_empty()).collect();
            let mut failed = Vec::new();
            for (name, test) in &tests {
                if !only.is_empty() && !only.contains(name) {
                    continue;
                }
                if catch_unwind(AssertUnwindSafe(test)).is_err() {
                    eprintln!("FAIL: {name}");
                    failed.push(*name);
                } else {
                    eprintln!("PASS: {name}");
                }
            }
            fsync_dir(MNT);

            if failed.is_empty() {
                println!("OK");
            } else {
                // What the guest's Fxfs logged.
                crate::kernel_log::print();
                println!("FAILED: {}", failed.join(", "));
            }
        }
    }
}
