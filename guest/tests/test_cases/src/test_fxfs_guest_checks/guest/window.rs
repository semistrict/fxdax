// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the checks of the DAX window: what it maps, PMDs, a full window, MAPs failing, and
//! writers killed.

use super::*;

/// A shared mapping read as one PMD, then written one page at a time, leaves no DAX entry
/// behind when its file is unlinked.
pub(super) fn test_pmd_write_unlink() {
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

/// fxdax: the checks before, on a 32 MiB window, filled its ranges' space, which other ranges
/// were reclaimed for: the paths a full window takes, as the sandbox team runs it, which a large
/// window never reaches (TASK-119.12, TASK-101). Since the filesystem was mounted last
/// (file_attributes remounts it).
pub(super) fn test_window_filled() {
    let stats = stats();
    assert!(stats["window_reclaims"] > 0, "the window's ranges never filled: {stats:?}");
}

/// fxdax: a vmsplice holding more of a file's pages than the window has room for, which a pipe
/// takes page by page: no chunk it holds a page of can go, so each of its faults finds no room,
/// and waits for some: SIGKILL ends it, whose fault looped on for ever without looking
/// (TASK-79.49). Its pages go with it, and the file reads back.
pub(super) fn test_window_held_killed() {
    const SIZE: usize = 48 << 20;
    let name = path("window-held");
    let data: Vec<u8> = (0..SIZE).map(|at| (at >> 21) as u8 + 1).collect();
    // Read in again, DAX: created here, it is a page-cache inode (doc-13, TASK-164.3).
    fs::write(&name, &data).unwrap();
    read_in_sealed(&name);
    let file = fs::File::open(&name).unwrap();
    let before = stats()["window_full"];
    let child = unsafe { libc::fork() };
    assert!(child >= 0, "fork: {}", std::io::Error::last_os_error());
    if child == 0 {
        unsafe {
            let map = libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            );
            let mut pipe = [0; 2];
            if map == libc::MAP_FAILED
                || libc::pipe(pipe.as_mut_ptr()) != 0
                || libc::fcntl(pipe[1], libc::F_SETPIPE_SZ, 2 * SIZE as libc::c_int) < 0
            {
                libc::_exit(1);
            }
            let spliced = libc::iovec { iov_base: map, iov_len: SIZE };
            libc::vmsplice(pipe[1], &spliced, 1, 0);
            libc::_exit(2);
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while stats()["window_full"] < before + 16 {
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) },
            0,
            "the splice ended, status {status:#x}, without filling the window"
        );
        assert!(std::time::Instant::now() < deadline, "the window never filled: {:?}", stats());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(child, libc::SIGKILL) }, 0);
    let status = wait_for_child(child, std::time::Duration::from_secs(20));
    assert_eq!(
        (libc::WIFSIGNALED(status), libc::WTERMSIG(status)),
        (true, libc::SIGKILL),
        "status {status:#x}"
    );
    drop(file);
    assert!(fs::read(&name).unwrap() == data, "the file differs from what was written");
    fs::remove_file(&name).unwrap();
}

/// fxdax: two mappings of a slice's page, of a sealed extent's region, loading it in turn, as
/// processes running one binary do: a fault
/// each, as the read of a page its entry names replaces nothing. A read that replaced the entry
/// took the other mapping's page away, for a fault for each load, as a build's processes queued
/// on the entries of their binary's hot pages for minutes on end (TASK-152). Then processes
/// reading all of the region's pages at once: a fault for each page of each, at most, and tries
/// of a PMD where no page table covers their mapping yet, which dax_faults_hist counts too.
/// The file, created here, is a page-cache inode until read in again (doc-13, TASK-164.3),
/// whose writeback packs it in the staging extent: read in again, from that extent sealed, it
/// is a slice of its region, read-only.
pub(super) fn test_reads_stay_mapped() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const PAGES: usize = 256;
    const SIZE: usize = PAGES * PAGE;
    const TURNS: u64 = 64;
    const READERS: usize = 4;
    const PASSES: usize = 64;
    let name = path("reads-stay-mapped");
    fs::write(&name, vec![1; SIZE]).unwrap();
    let open = || fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    let file = open();
    file.sync_all().unwrap();
    // Written again, its writeback's blocks in the staging extent, which the fsync seals.
    file.write_all_at(&[2; SIZE], 0).unwrap();
    file.sync_data().unwrap();
    drop(file);
    read_in_sealed(&name);
    let file = open();
    let map = |at: *mut libc::c_void, flags| {
        let map = unsafe {
            libc::mmap(at, SIZE, libc::PROT_READ, libc::MAP_SHARED | flags, file.as_raw_fd(), 0)
        };
        assert_ne!(map, libc::MAP_FAILED);
        map as *const u8
    };
    let faults = || stats()["dax_faults_hist"];

    // One 2 MiB, within what is reserved, for both mappings, so that one page table has them.
    let reserved = 2 * CHUNK;
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
    let spare =
        unsafe { libc::mmap(std::ptr::null_mut(), reserved, libc::PROT_NONE, flags, -1, 0) };
    assert_ne!(spare, libc::MAP_FAILED);
    let aligned = (spare as usize).next_multiple_of(CHUNK);
    let first = map(aligned as *mut libc::c_void, libc::MAP_FIXED);
    let second = map((aligned + SIZE) as *mut libc::c_void, libc::MAP_FIXED);
    // Its page table made, by another page's load: else tries of a PMD fall back, and count.
    assert_eq!(unsafe { std::ptr::read_volatile(first.add(PAGE)) }, 2);
    let before = faults();
    for _ in 0..TURNS {
        assert_eq!(unsafe { std::ptr::read_volatile(first) }, 2);
        assert_eq!(unsafe { std::ptr::read_volatile(second) }, 2);
    }
    let turns = faults() - before;
    assert_eq!(unsafe { libc::munmap(spare, reserved) }, 0);

    let before = faults();
    let began = std::time::Instant::now();
    let readers: Vec<_> = (0..READERS)
        .map(|_| match unsafe { libc::fork() } {
            0 => {
                let mapped = map(std::ptr::null_mut(), 0);
                let same = (0..PASSES).all(|_| {
                    (0..PAGES).all(|page| {
                        let byte = unsafe { std::ptr::read_volatile(mapped.add(page * PAGE)) };
                        byte == 2
                    })
                });
                unsafe { libc::_exit(i32::from(!same)) }
            }
            pid => {
                assert!(pid > 0, "fork: {}", std::io::Error::last_os_error());
                pid
            }
        })
        .collect();
    for pid in readers {
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0, "reader {pid}: status {status:#x}");
    }
    let read = faults() - before;
    eprintln!(
        "reads_stay_mapped: {turns} faults in {TURNS} turns, {read} of {READERS} readers of \
         {PAGES} pages in {:?}",
        began.elapsed()
    );
    assert_eq!(turns, 2, "two mappings took {turns} faults in {TURNS} turns");
    assert!(
        read <= (READERS * (PAGES + 4)) as u64,
        "{READERS} readers of {PAGES} pages took {read} faults"
    );
    fs::remove_file(&name).unwrap();
}

/// fxdax: write(2)s of pages scattered over a file read in from sealed extents take no MAP
/// each: a write makes its page private where the window maps it, a page more each, by one
/// PRIVATE each, its iomap's, before its store (doc-13, TASK-164.9). A MAP of its own took
/// 200 us of a write's 230 on writable DAX: rand-write fell from 18,151 to 3,534 IOPS. Fails if
/// a write's iomap made more than its page private, or mapped its chunk again.
pub(super) fn test_writes_private() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const SIZE: usize = 4 << 20;
    const PAGES: usize = SIZE / PAGE;
    const WRITES: usize = 256;
    const STRIDE: usize = 677;
    let name = path("writes-private");
    let mut expected = vec![1; SIZE];
    fs::write(&name, &expected).unwrap();
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    let requests = || stats()["window_map_requests"];
    // The pages private, and the PRIVATE requests sent.
    let counts = || {
        let stats = stats();
        (stats["dax_private_pages"], stats["dax_privates"])
    };
    let (before, private_before) = (requests(), counts());
    for write in 0..WRITES {
        let at = (write * STRIDE % PAGES) * PAGE;
        let data = vec![(write % 251) as u8 + 2; PAGE];
        file.write_all_at(&data, at as u64).unwrap();
        expected[at..at + PAGE].copy_from_slice(&data);
    }
    let written = requests() - before;
    let private_after = counts();
    let before = requests();
    assert!(fs::read(&name).unwrap() == expected, "the file differs from what was written");
    let read = requests() - before;
    eprintln!("writes_private: {written} MAPs for {WRITES} writes, then {read}");
    let private = private_before.0 + WRITES as u64;
    assert_eq!(private_after.0, private, "private pages after {WRITES} writes of a page");
    let privates = private_before.1 + WRITES as u64;
    assert_eq!(private_after.1, privates, "PRIVATE requests for writes of a page");
    assert!(written < (WRITES / 8) as u64, "{WRITES} writes took {written} MAPs");
    assert!(read < (WRITES / 8) as u64, "the read after them took {read} MAPs");
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// fxdax: a mapped file's page stored to, its DAX entry dirty; another file's fsync seals; then
/// msync writes the entry back, and the page holds what was stored, through the mapping and the
/// file (TASK-128). Of a file read in from sealed extents, DAX (doc-13).
pub(super) fn test_stale_entry_flushed() {
    use std::os::unix::io::AsRawFd;
    const SIZE: usize = 1 << 20;
    let (name, other) = (path("stale-entry"), path("stale-entry-other"));
    fs::write(&name, vec![1; SIZE]).unwrap();
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
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
    assert_ne!(map, libc::MAP_FAILED);
    let map = map as *mut u8;
    // The store makes the page private, its entry dirty.
    unsafe { std::ptr::write_volatile(map, 2) };
    // Another file's fsync seals, of its creation alone.
    let other_file =
        fs::OpenOptions::new().create(true).truncate(true).write(true).open(&other).unwrap();
    other_file.sync_all().unwrap();
    assert_eq!(unsafe { libc::msync(map as *mut libc::c_void, SIZE, libc::MS_SYNC) }, 0);
    let stored = unsafe { std::ptr::read_volatile(map) };
    assert_eq!(unsafe { libc::munmap(map as *mut libc::c_void, SIZE) }, 0);
    drop(file);
    assert_eq!((stored, fs::read(&name).unwrap()[0]), (2, 2));
    fs::remove_file(&name).unwrap();
    fs::remove_file(&other).unwrap();
}

/// fxdax: a write whose range's MAP fails after its first piece (the fxfs.dax_fail_maps fault
/// injection, as a device's MAP error): the chunk in pieces, written back as the page cache's
/// (created here, a page-cache inode until read in again, doc-13, TASK-164.3) whole, then pages
/// 0 and 2 again, and read in again, no range mapping it yet. The write fails (EIO), and leaves
/// the file as it was; the next writes it, as read(2) finds it, and the file evicted. Fails if
/// the write went on into a range mapped in part (EIO lost, or the pages past the first piece
/// other data), or if the failure left the range so that the next write or read failed too.
/// On writable DAX the range mapped a page where it was, which Fxfs freed (TASK-79.106).
pub(super) fn test_map_failed() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const SIZE: usize = 4 << 20;
    let name = path("map-failed");
    let open = || fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    let mut expected: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8 + 1).collect();
    fs::write(&name, &expected).unwrap();
    let file = open();
    file.sync_all().unwrap();
    file.write_all_at(&[0xa0; PAGE], 0).unwrap();
    file.write_all_at(&[0xa2; PAGE], 2 * PAGE as u64).unwrap();
    expected[..PAGE].fill(0xa0);
    expected[2 * PAGE..3 * PAGE].fill(0xa2);
    file.sync_all().unwrap();
    drop(file);
    read_in_sealed(&name);
    let file = open();
    {
        let _fail = Param::set("dax_fail_maps", "1");
        let failed = file.write_at(&[0xc0; 3 * PAGE], 0).unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EIO), "{failed}");
        assert_injected("dax_fail_maps");
    }
    assert!(fs::read(&name).unwrap() == expected, "the file, read after the write failed");
    file.write_all_at(&[0xc0; 3 * PAGE], 0).unwrap();
    expected[..3 * PAGE].fill(0xc0);
    assert!(fs::read(&name).unwrap() == expected, "the file, read after the write");
    drop(file);
    evict(&name);
    assert!(fs::read(&name).unwrap() == expected, "the file evicted");
    fs::remove_file(&name).unwrap();
}

/// fxdax: a writer killed before its copy, its pages made private and dirty, of a file read in
/// again from sealed extents (created here, it is a page-cache inode until then, doc-13,
/// TASK-164.3): the kernel's copy stops at the signal, and the pages hold what they held,
/// written back as it was. Played out with a pause point. On writable DAX, a writer killed
/// once its blocks moved left zeros where the file's data was (TASK-153.5). Fails if the copy
/// went on after the signal, or if the private pages read as anything but the file's (zeros, a
/// page not copied from its extent's).
pub(super) fn test_write_killed() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("write-killed");
    fs::write(&name, vec![1; 4 * PAGE]).unwrap();
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    let (pauses, only) = Pauses::begin_of(Pauses::WRITE_MAPPED, &name);
    let writer = unsafe { libc::fork() };
    assert!(writer >= 0, "fork: {}", std::io::Error::last_os_error());
    if writer == 0 {
        file.write_all_at(&[2; 4 * PAGE], 0).unwrap();
        unsafe { libc::_exit(0) };
    }
    Pauses::wait_or(Pauses::WRITE_MAPPED, Some(writer));
    assert_eq!(unsafe { libc::kill(writer, libc::SIGKILL) }, 0);
    drop(pauses);
    drop(only);
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(writer, &mut status, 0) }, writer);
    assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    let mut read = vec![0; 4 * PAGE];
    file.read_exact_at(&mut read, 0).unwrap();
    assert!(read.iter().all(|byte| *byte == 1), "the file: {:?}", &read[..4]);
    drop(file);
    evict(&name);
    assert!(fs::read(&name).unwrap().iter().all(|byte| *byte == 1), "the file evicted");
    fs::remove_file(&name).unwrap();
}

/// fxdax: a fault injection counting down, `name`, has run out.
fn assert_injected(name: &str) {
    let left = fs::read_to_string(format!("/sys/module/fxfs/parameters/{name}")).unwrap();
    assert_eq!(left.trim(), "0", "{name} never failed anything");
}

/// fxdax: a chunk of a file in pieces, which its range's own space maps in one request: that
/// request fails having mapped the first (fxfs.dax_fail_map_many), and the pieces are mapped
/// one by one; and that fails too at the second (fxfs.dax_fail_fallbacks), the range left with
/// the first alone: a read fails, and the next maps it all; so does a write, leaving the file
/// as it was. Each on a clone of the file: evicted, a file keeps what its ranges map, which
/// needs no map (TASK-119.13). The file is read in again from sealed extents (doc-13), and its
/// pages rewritten are written back to new ones; each clone, created here, a page-cache inode
/// until read in again (TASK-164.3), is synced and read in again. Fails if a range mapped in
/// part is kept, for reads to go on to what it doesn't map, or a write into it to succeed.
pub(super) fn test_map_many_failed() {
    use std::os::unix::fs::FileExt;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    const PAGE: usize = 4096;
    const SIZE: usize = 4 << 20;
    let name = path("map-many-failed");
    let mut expected: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8 + 1).collect();
    fs::write(&name, &expected).unwrap();
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    for at in [0, 8 * PAGE, 16 * PAGE] {
        file.write_all_at(&[0xa0; PAGE], at as u64).unwrap();
        expected[at..at + PAGE].fill(0xa0);
        file.sync_data().unwrap();
    }
    let clone = |suffix: &str| {
        let name = path(&format!("map-many-failed-{suffix}"));
        let open = fs::OpenOptions::new().read(true).write(true).create(true).clone();
        let clone = open.open(&name).unwrap();
        let cloned = unsafe { libc::ioctl(clone.as_raw_fd(), FICLONE as _, file.as_raw_fd()) };
        assert_eq!(cloned, 0, "FICLONE: {}", std::io::Error::last_os_error());
        drop(clone);
        read_in_sealed(&name);
        let clone = open.open(&name).unwrap();
        (name, clone)
    };
    let (one_by_one, clone_read) = clone("one-by-one");
    {
        let _many = Param::set("dax_fail_map_many", "1");
        assert!(fs::read(&one_by_one).unwrap() == expected, "the file, read one by one");
        assert_injected("dax_fail_map_many");
    }
    drop(clone_read);
    let (read_failed, clone_read) = clone("read-failed");
    {
        let _many = Param::set("dax_fail_map_many", "1");
        let _fallback = Param::set("dax_fail_fallbacks", "1");
        let mut read = vec![0; SIZE];
        let failed = clone_read.read_at(&mut read, 0).unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EIO), "{failed}");
        assert_injected("dax_fail_fallbacks");
        assert!(fs::read(&read_failed).unwrap() == expected, "the file, read after it failed");
    }
    drop(clone_read);
    let (write_failed, clone_write) = clone("write-failed");
    {
        let _many = Param::set("dax_fail_map_many", "1");
        let _fallback = Param::set("dax_fail_fallbacks", "1");
        // The write's chunk's map failed: it fails, and leaves the file as it was.
        let failed = clone_write.write_at(&[0xc0; 17 * PAGE], 0).unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EIO), "{failed}");
        assert_injected("dax_fail_fallbacks");
    }
    assert!(fs::read(&write_failed).unwrap() == expected, "the file, read after it failed");
    clone_write.write_all_at(&[0xc0; 17 * PAGE], 0).unwrap();
    expected[..17 * PAGE].fill(0xc0);
    assert!(fs::read(&write_failed).unwrap() == expected, "the file, read after the write");
    drop(clone_write);
    drop(file);
    evict(&write_failed);
    assert!(fs::read(&write_failed).unwrap() == expected, "the file evicted");
    for name in [name, one_by_one, read_failed, write_failed] {
        fs::remove_file(name).unwrap();
    }
}

/// A read fault of a PMD that says its pages are shared finds their folio's fields clean.
/// dax_folio_put leaves an order 0 folio's index, which is its share count, so that a page that
/// had an entry at a nonzero index looks shared: dax_associate_entry doesn't make the folio
/// compound, WARNs (fs/dax.c:458), and a PMD goes over an order 0 folio. An inode's load gives
/// the second chunk's head page such an entry, and the inode's eviction ends the entry but not
/// the range (found by guest/sim, TASK-149.14). A WARN fails the check, as any does. The file,
/// created here, is read in again from sealed extents, DAX (doc-13, TASK-164.3).
pub(super) fn test_pmd_over_stale_index() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const PIECE: usize = CHUNK / 2;
    let name = path("pmd-over-stale-index");
    let data: Vec<u8> = (0..2 * CHUNK).map(|i| (i % 251) as u8 + 1).collect();
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&name)
        .unwrap();
    // The second chunk in two extents, each whole, as each piece's writeback writes it: a
    // range of its own, not a slice of a region, at a chunk's alignment in the window, its
    // pages in a row.
    for piece in [0..CHUNK, CHUNK..CHUNK + PIECE, CHUNK + PIECE..2 * CHUNK] {
        file.write_all_at(&data[piece.clone()], piece.start as u64).unwrap();
        file.sync_data().unwrap();
    }
    drop(file);
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    // At a chunk's alignment in memory too, or no fault can map a PMD of it: an address that
    // mmap chooses needn't be.
    let map_chunk = |file: &fs::File, len: usize, prot: libc::c_int| {
        let room = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * CHUNK,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(room, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let aligned = (room as usize).next_multiple_of(CHUNK);
        let map = unsafe {
            libc::mmap(
                aligned as *mut libc::c_void,
                len,
                prot,
                libc::MAP_SHARED | libc::MAP_FIXED,
                file.as_raw_fd(),
                CHUNK as libc::off_t,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        map
    };

    // Too small for a PMD: the head page's entry, at index 512, not shared.
    let map = map_chunk(&file, 2 * PAGE, libc::PROT_READ | libc::PROT_WRITE);
    let bytes = map.cast::<u8>();
    assert_eq!(unsafe { bytes.read_volatile() }, data[CHUNK], "the chunk's first page");
    let second = unsafe { bytes.add(PAGE) };
    assert_eq!(unsafe { second.read_volatile() }, data[CHUNK + PAGE], "its second page");
    assert_eq!(unsafe { libc::munmap(map, 2 * PAGE) }, 0);
    drop(file);
    // Eviction ends the entries, not the range.
    evict(&name);

    let file = fs::File::open(&name).unwrap();
    let map = map_chunk(&file, CHUNK, libc::PROT_READ);
    let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, CHUNK) };
    assert!(bytes == &data[CHUNK..], "the chunk as mapped differs from what was written");
    assert_eq!(unsafe { libc::munmap(map, CHUNK) }, 0);
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// fxdax: a file larger than a chunk, written whole by one write(2) and fsynced, then read
/// through a mapping: each byte as written. The first load of such a file died of SIGBUS, its
/// fault failing with no error (TASK-121's pause check). The read maps the first chunk with a
/// PMD, which the store after breaks up (TASK-123). The file is read in again from sealed
/// extents (created here, it is a page-cache inode until then, doc-13, TASK-164.3): the store
/// splits the PMD, making its page alone private, which the fsync after writes back. Fails if
/// the store made the PMD's 2 MiB private, or none, or if the fsync lost the store.
pub(super) fn test_mapped_after_write() {
    const SIZE: usize = 4 << 20;
    let name = path("mapped-after-write");
    let data: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8 + 1).collect();
    fs::write(&name, &data).unwrap();
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
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
    let before = stats()["dax_huge_maps"];
    let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, SIZE) };
    assert!(bytes == &data[..], "the file as mapped differs from what was written");
    assert!(stats()["dax_huge_maps"] > before, "no fault mapped a PMD's 2 MiB: {:?}", stats());
    // A store to the second page, which the first chunk's mapping, read in 2 MiB at once,
    // mapped in a row with the rest, makes it private; then the chunk is read again, page by
    // page.
    let mut data = data;
    let private = || stats()["dax_private_pages"];
    let before = private();
    unsafe { (map as *mut u8).add(4096 + 100).write_volatile(0xee) };
    data[4096 + 100] = 0xee;
    assert_eq!(private(), before + 1, "private pages after a store to a PMD's page");
    file.sync_all().unwrap();
    let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, SIZE) };
    assert!(bytes == &data[..], "the file as mapped differs after a store to a page");
    // And again: each page's fault replaced the kernel's entry of the 2 MiB, which held
    // them all, with its own (TASK-123).
    let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, SIZE) };
    assert!(bytes == &data[..], "the file as mapped differs read again");
    assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// fxdax: stores through a mapping of a file, write(2)s to its pages, holes or not, and other
/// files made, written and removed meanwhile, which make the journal compact, for a few
/// seconds: on writable DAX, a write in the gate waited for a fault's DAX entry, the fault for
/// Fxfs, Fxfs for compaction, and compaction's seal for the write, and the filesystem hung
/// (TASK-79.12). Each finishes. The write(2)s write pages' first halves, the stores their
/// second: each finds its bytes as it left them, now and then as it goes, and the file holds
/// what both left (TASK-119.1). The file, all holes, is read in again, DAX (doc-13,
/// TASK-164.3): its holes map zeros, which stores and write(2)s copy on write.
pub(super) fn test_faults_writes_compaction() {
    use std::os::unix::fs::FileExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    const PAGE: usize = 4096;
    const HALF: usize = PAGE / 2;
    const SIZE: usize = 4 << 20;
    let name = path("faults-writes");
    // Truncated: a check before that failed leaves its file, which the model holds none of.
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&name)
        .unwrap();
    file.set_len(SIZE as u64).unwrap();
    drop(file);
    read_in_sealed(&name);
    let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
    let stop = AtomicBool::new(false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let read = |at: usize, buf: &mut [u8]| file.read_exact_at(buf, at as u64).unwrap();
    let (stored, written) = std::thread::scope(|scope| {
        // Stores through a mapping, a page at a time, holes and pages mapped already.
        let storer = scope.spawn(|| {
            let mut stored = vec![0u8; SIZE];
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
            let mut byte = [0];
            while !stop.load(Ordering::Relaxed) {
                let at = (n * 7919) % (SIZE / PAGE) * PAGE + HALF + n % HALF;
                unsafe { map.cast::<u8>().add(at).write_volatile(n as u8) };
                stored[at] = n as u8;
                if n % 64 == 0 {
                    read(at, &mut byte);
                    assert_eq!(byte[0], n as u8, "the store at {at:#x} reads back other");
                }
                n += 1;
            }
            assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
            stored
        });
        // write(2)s to the pages, fsynced now and then: a writeback and a seal each.
        let writer = scope.spawn(|| {
            let mut written = vec![0u8; SIZE / PAGE];
            let mut half = [0; HALF];
            let mut n = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let page = (n * 104_729 + 3) % (SIZE / PAGE);
                file.write_all_at(&[n as u8; HALF], (page * PAGE) as u64).unwrap();
                written[page] = n as u8;
                if n % 16 == 0 {
                    file.sync_data().unwrap();
                }
                read(page * PAGE, &mut half);
                assert!(half == [n as u8; HALF], "the write to page {page} reads back other");
                n += 1;
            }
            written
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
        (storer.join().unwrap(), writer.join().unwrap())
    });
    file.sync_all().unwrap();
    let mut page = vec![0; PAGE];
    for (n, &byte) in written.iter().enumerate() {
        read(n * PAGE, &mut page);
        assert!(page[..HALF].iter().all(|&b| b == byte), "page {n}'s first half: not {byte}");
        let want = &stored[n * PAGE + HALF..][..HALF];
        if let Some(at) = (0..HALF).find(|&at| page[HALF + at] != want[at]) {
            trace_dump(&name, n as u64);
            let wrong = (0..HALF).filter(|&at| page[HALF + at] != want[at]).count();
            panic!(
                "page {n}'s second half: byte {at:#x} of it reads {:#x}, stored {:#x}; {wrong} \
                 bytes of {HALF} differ, {} of them zeros",
                page[HALF + at],
                want[at],
                (0..HALF).filter(|&at| page[HALF + at] != want[at] && page[HALF + at] == 0).count()
            );
        }
    }
    drop(file);
    fs::remove_file(&name).unwrap();
}
