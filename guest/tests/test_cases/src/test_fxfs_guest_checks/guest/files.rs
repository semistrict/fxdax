// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of files' data: written, mapped, truncated, punched, cloned and evicted, read back as
//! written.

use super::*;

pub(super) fn test_host_file() {
    assert_eq!(fs::read(path("from-host")).unwrap(), HOST_CONTENT);
}

pub(super) fn test_mmap_write(dax: bool) {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path("dax-mapped"))
        .unwrap();
    file.set_len(MAPPED_SIZE as u64).unwrap();
    // fxdax: a file created in the mount is a page-cache inode until read in again (doc-13,
    // TASK-164.3); read in, it maps its data from the window.
    assert!(!is_dax(&path("dax-mapped")), "dax-mapped, created here, is DAX");

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

        // Writes to it, through a mapping and with write(2), go through the window, copying
        // on write.
        let file = fs::OpenOptions::new().read(true).write(true).open(path("dax-mapped")).unwrap();
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

pub(super) fn test_regular_write() {
    let mut file = fs::File::create(path("written")).unwrap();
    file.write_all(WRITTEN).unwrap();
    file.sync_all().unwrap();
    assert_eq!(fs::read(path("written")).unwrap(), WRITTEN);
}

pub(super) fn test_truncate_mapped() {
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

pub(super) fn test_eviction_writes_back() {
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

/// fallocate: mode 0 grows the file, FALLOC_FL_KEEP_SIZE doesn't, other modes aren't
/// supported.
pub(super) fn test_fallocate() {
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

/// A file's pages dirtied through a mapping, another file's fsync between (which writes back
/// nothing of this file's), then partly zeroed, by a hole punched and by a truncation: what
/// they hold reads the same through the mapping and through reads.
pub(super) fn test_zero_mapped() {
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

/// SEEK_DATA, SEEK_HOLE and FS_IOC_FIEMAP on a sparse file: data at its start and at 1 MiB.
pub(super) fn test_sparse() {
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

/// fxdax: st_blocks follows what a file holds, the inode in the cache all along, and an unlinked
/// file's too (TASK-138).
pub(super) fn test_blocks_follow_writes() {
    use std::os::unix::fs::MetadataExt;
    let name = path("blocks");
    let mut file = fs::File::create_new(&name).unwrap();
    let bytes = || fs::metadata(&name).unwrap().blocks() * 512;
    // Before any sync: dirty pages count, or a file looks sparser than it is.
    file.write_all(&vec![1; 1 << 20]).unwrap();
    assert_eq!(bytes(), 1 << 20);
    file.sync_all().unwrap();
    assert_eq!(bytes(), 1 << 20);
    file.set_len(0).unwrap();
    file.sync_all().unwrap();
    assert_eq!(bytes(), 0);
    file.write_all(&vec![2; 1 << 20]).unwrap();
    file.sync_all().unwrap();
    fs::remove_file(&name).unwrap();
    assert_eq!(file.metadata().unwrap().blocks() * 512, 1 << 20);
}

/// TASK-26.19.6: reflinks: FICLONE shares a file's extents with another, FICLONERANGE a range
/// of them in place of what another held there, and copy_file_range copies by sharing them
/// too: each reads as it should, taking none of the space a copy would, and writes to any of
/// them, through a mapping too, leave the others as they were, read in again too.
pub(super) fn test_reflinks() {
    use std::os::unix::fs::FileExt;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    const BLOCK: usize = 4096;
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
    clone_range(&ranged_file, &src, 8 * BLOCK as u64, 16 * BLOCK as u64, 4 * BLOCK as u64);
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
    assert_eq!(copied, data.len() as i64, "copy_file_range: {}", std::io::Error::last_os_error());
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
pub(super) fn test_direct_io() {
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

/// TASK-26.19.10: Fxfs's cached records go under memory pressure: the superblock's shrinker
/// counts them (FXFS_IOC_CACHED_OBJECTS) and frees them, as drop_caches (2) has it do.
pub(super) fn test_cache_shrinks() {
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
