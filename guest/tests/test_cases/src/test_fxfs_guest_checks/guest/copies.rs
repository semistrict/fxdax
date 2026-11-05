// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of copies between a file and user memory that is the file's own mapping, ends short,
//! or is past the file's end.

use super::*;

/// fxdax: a write whose source is a mapping of the file it grows: faulting that source in, it
/// mapped the chunk only as large as the file, which the write's range then couldn't reach,
/// again and again, until it stopped short: the write's end lost, or EIO (xfstests generic/638,
/// TASK-147).
pub(super) fn test_write_from_own_mapping() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("write-from-own-mapping");
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
    let contents: Vec<u8> = (0..2 * PAGE).map(|i| (i % 251) as u8).collect();
    file.write_all_at(&contents, 0).unwrap();
    file.sync_all().unwrap();
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            2 * PAGE,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let wrote = unsafe { libc::pwrite(file.as_raw_fd(), map, 64, 2 * PAGE as i64) };
    assert_eq!(wrote, 64, "pwrite: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, 2 * PAGE) }, 0);
    let mut read = vec![0; 64];
    file.read_exact_at(&mut read, 2 * PAGE as u64).unwrap();
    assert_eq!(read, contents[..64], "what the write copied from its own mapping");
    fs::remove_file(&name).unwrap();
}

/// A write of pages from a shared mapping of those same pages copies what they held. On
/// writable DAX a write moved whole pages to new blocks, keeping none of what it covered, and
/// read its source from them: the first write here left zeros and said it succeeded; and in the
/// second, each page's source a page on, the page written first was read as zeros for the next
/// (TASK-150.8).
pub(super) fn test_write_from_same_page() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("write-from-same-page");
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
    let mut expected: Vec<u8> = (0..3 * PAGE).map(|i| (i % 251) as u8 + 1).collect();
    file.write_all_at(&expected, 0).unwrap();
    file.sync_all().unwrap();
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            3 * PAGE,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let wrote = unsafe { libc::pwrite(file.as_raw_fd(), map, PAGE, 0) };
    assert_eq!(wrote, PAGE as isize, "pwrite: {}", std::io::Error::last_os_error());
    let mut read = vec![0; 3 * PAGE];
    file.read_exact_at(&mut read, 0).unwrap();
    assert!(read == expected, "a page written from itself: {:?}", &read[..8]);
    file.sync_all().unwrap();
    let next = unsafe { map.cast::<u8>().add(PAGE) }.cast();
    let wrote = unsafe { libc::pwrite(file.as_raw_fd(), next, 2 * PAGE, 0) };
    assert_eq!(wrote, 2 * PAGE as isize, "pwrite: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, 3 * PAGE) }, 0);
    expected.copy_within(PAGE.., 0);
    file.read_exact_at(&mut read, 0).unwrap();
    assert!(read == expected, "pages written from the pages after: {:?}", &read[..8]);
    file.sync_all().unwrap();
    // In one page, apart: the iomap of the write unmapped what it was to copy, each time.
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            PAGE,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let source = unsafe { map.cast::<u8>().add(2000) }.cast();
    let wrote = unsafe { libc::pwrite(file.as_raw_fd(), source, 100, 100) };
    assert_eq!(wrote, 100, "pwrite: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
    expected.copy_within(2000..2100, 100);
    file.read_exact_at(&mut read, 0).unwrap();
    assert!(read == expected, "bytes written from their page: {:?}", &read[100..108]);
    drop(file);
    evict(&name);
    assert!(fs::read(&name).unwrap() == expected, "the file evicted");
    fs::remove_file(&name).unwrap();
}

/// A write over a hole a mapping maps zeros for takes those zeros away, whatever its pieces. Page
/// 1 is a hole between pages 0 and 2, written first, and a load maps the zero page for it. On
/// writable DAX, a write of pages 0 and 1 then took two iomaps, as page 0 stayed where it was
/// and page 1 went after page 2 in the staging extent; only the first said its pages were new,
/// so that page 1's mapping could stay on the zero page, and a store from it write zeros for
/// good (found by guest/sim, TASK-149.14).
pub(super) fn test_write_over_mapped_hole() {
    const PAGE: usize = 4096;
    let name = path("write-over-mapped-hole");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&name)
        .unwrap();
    let fd = file.as_raw_fd();
    let write = |at: usize, data: &[u8]| {
        let written = unsafe { libc::pwrite(fd, data.as_ptr().cast(), data.len(), at as i64) };
        assert_eq!(written, data.len() as isize, "{}", std::io::Error::last_os_error());
    };
    write(0, &[b'a'; PAGE]);
    write(2 * PAGE, &[b'a'; PAGE]);
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            3 * PAGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let mapped = map.cast::<u8>();
    assert_eq!(unsafe { mapped.add(PAGE).read_volatile() }, 0, "the hole, through the mapping");

    write(0, &[b'b'; 2 * PAGE]);
    let mut read = vec![0u8; PAGE];
    let got = unsafe { libc::pread(fd, read.as_mut_ptr().cast(), PAGE, PAGE as i64) };
    assert_eq!(got, PAGE as isize, "{}", std::io::Error::last_os_error());
    assert_eq!(read, vec![b'b'; PAGE], "page 1, through read(2)");
    let through = unsafe { std::slice::from_raw_parts(mapped.add(PAGE), PAGE) };
    assert!(through.iter().all(|byte| *byte == b'b'), "page 1 through the mapping is stale");
    unsafe { libc::munmap(map, 3 * PAGE) };
    fs::remove_file(&name).unwrap();
}

/// fxdax: a read into a mapping of the file it reads, whose pages the window maps read-only:
/// the read's copy faults, the fault can't store to a slice, and dropping the slice waited for
/// the slice's users, the read among them, for ever.
pub(super) fn test_read_into_own_mapping() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("read-into-own-mapping");
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
    let contents: Vec<u8> = (0..2 * PAGE).map(|i| (i % 251) as u8).collect();
    file.write_all_at(&contents, 0).unwrap();
    file.sync_all().unwrap();
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            2 * PAGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let dest = unsafe { map.cast::<u8>().add(PAGE) }.cast();
    let read = unsafe { libc::pread(file.as_raw_fd(), dest, PAGE, 0) };
    assert_eq!(read, PAGE as isize, "pread: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, 2 * PAGE) }, 0);
    let mut after = vec![0; PAGE];
    file.read_exact_at(&mut after, PAGE as u64).unwrap();
    assert_eq!(after, contents[..PAGE], "what the read copied into its own mapping");
    fs::remove_file(&name).unwrap();
}

/// A write(2) of a whole page over a file's data not cached, from a buffer that faults after
/// its first 100 bytes: it writes those, and the rest of the page keeps the file's data. The
/// page, never read as the write was to cover it, took zeros where the copy fell short
/// (TASK-79.14).
pub(super) fn test_short_copy() {
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
pub(super) fn test_direct_read_past_end() {
    use std::os::unix::fs::OpenOptionsExt;
    const PAGE: usize = 4096;
    let name = path("direct-past-end");
    fs::write(&name, vec![b'C'; 100]).unwrap();
    let file = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECT).open(&name).unwrap();
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

/// A store through a mapping past a file's end, then the file grown over it: it reads zeros
/// there. Writeback wrote no more than the file held, and the folio's tail, never zeroed,
/// became the file's data (TASK-79.53).
pub(super) fn test_store_past_end() {
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

/// As `test_store_past_end`, the file grown by a write past its end, which leaves what is
/// between the old end and the write for reads to find: it reads zeros there too.
pub(super) fn test_store_past_end_then_written() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("store-past-end-written");
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
    file.write_all_at(&[b'E'; 8], 2 * PAGE as u64).unwrap();
    let mut read = vec![0xff; 2 * PAGE + 8];
    file.read_exact_at(&mut read, 0).unwrap();
    let mut expected = vec![0; 2 * PAGE + 8];
    expected[..100].fill(b'D');
    expected[2 * PAGE..].fill(b'E');
    assert!(read == expected, "past the old end: {:?}", &read[996..1004]);
    assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// As `test_store_past_end`, the file grown by a clone past its end, which grew the inode
/// without zeroing what the cached page held past the old end, and Fxfs's file without zeroing
/// what its last block held there (TASK-153.4).
pub(super) fn test_store_past_end_then_cloned() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let (name, source_name) = (path("store-past-end-cloned"), path("store-past-end-source"));
    fs::write(&source_name, vec![b'E'; PAGE]).unwrap();
    fs::write(&name, vec![b'D'; 100]).unwrap();
    let source = fs::File::open(&source_name).unwrap();
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
    clone_range(&file, &source, 0, PAGE as u64, 2 * PAGE as u64);
    let mut read = vec![0xff; 3 * PAGE];
    file.read_exact_at(&mut read, 0).unwrap();
    let mut expected = vec![0; 3 * PAGE];
    expected[..100].fill(b'D');
    expected[2 * PAGE..].fill(b'E');
    assert!(read == expected, "past the old end: {:?}", &read[996..1004]);
    assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
    drop((file, source));
    evict(&name);
    let read = fs::read(&name).unwrap();
    assert!(read == expected, "read in again, past the old end: {:?}", &read[996..1004]);
    fs::remove_file(&name).unwrap();
    fs::remove_file(&source_name).unwrap();
}

/// A write past the end from memory that isn't there says EFAULT, and leaves the file as long
/// as it was. Cutting the file back to what a write stopped short wrote, DAX grew it to the
/// write's start.
pub(super) fn test_write_past_end_copies_nothing() {
    const PAGE: usize = 4096;
    let name = path("write-past-end-copies-nothing");
    fs::write(&name, vec![b'D'; 100]).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&name).unwrap();
    let wrote = unsafe { libc::pwrite(file.as_raw_fd(), 1 as *const _, 8, 2 * PAGE as i64) };
    assert_eq!(wrote, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));
    assert_eq!(fs::metadata(&name).unwrap().len(), 100);
    drop(file);
    evict(&name);
    assert_eq!(fs::read(&name).unwrap(), vec![b'D'; 100]);
    fs::remove_file(&name).unwrap();
}
