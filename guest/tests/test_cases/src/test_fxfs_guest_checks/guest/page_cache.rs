// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of files in the page cache, without a DAX window: large folios and writeback.

use super::*;

/// TASK-26.19.11: a file in the page cache is cached in large folios: of a large file read
/// through once, most of the pages its mapping maps are compound pages' tails
/// (/proc/kpageflags' KPF_COMPOUND_TAIL).
pub(super) fn test_large_folios() {
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
    assert!(tails >= SIZE / PAGE / 2, "{tails} of {} pages are large folios' tails", SIZE / PAGE);
    fs::remove_file(&file).unwrap();
}

/// fxdax: pages scattered across a file, written back together: every other page of a file
/// of 8 MiB, each written by a write(2) of its own once the file is out of the page cache, so
/// that each is a folio of its own, and then part of a page past the file's end. Writeback writes
/// many such folios with one call to Fxfs (fxfs_write_many, TASK-92.10). Read again from the
/// device, the file holds what each wrote.
pub(super) fn test_scattered_writeback() {
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
pub(super) fn test_partial_writeback() {
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

/// fxdax: a page cache writeback that finds no memory keeps what it was to write (TASK-212): of a
/// file's 16 pages written, its fsync with each of its first 64 allocations that may fail failing in
/// turn (the debug kernel's fault injection, /proc/thread-self/fail-nth), the fsync failing ENOMEM
/// or not at all; then the file's clean pages dropped (POSIX_FADV_DONTNEED, drop_caches), which a
/// writeback that left them clean lost, the store holding the round before, as TASK-207 found of
/// DAX; then synced and evicted, which reads what was written. Said, and passed, on a kernel
/// without fault injection: the debug kernel's suites run it (soak-sanitized).
pub(super) fn test_writeback_out_of_memory() {
    use std::os::unix::fs::FileExt;
    const PAGES: usize = 16;
    const NTH: u64 = 64;
    if !script_run::Faults::available() {
        eprintln!(
            "writeback_out_of_memory: no fault injection here (FXDAX_GUEST_KERNEL_BUILD=debug)"
        );
        return;
    }
    let faults = script_run::Faults::set_up(0);
    let name = path("writeback-out-of-memory");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&name)
        .unwrap();
    let mut failed = 0;
    for nth in 1..=NTH {
        let data: Vec<u8> = (0..PAGES * 4096).map(|at| (at as u64 * 31 + nth) as u8).collect();
        file.write_all_at(&data, 0).unwrap();
        let (synced, fired) = faults.failing(nth, || file.sync_all());
        if let Err(err) = &synced {
            assert_eq!(
                err.raw_os_error(),
                Some(libc::ENOMEM),
                "allocation {nth} failing: the fsync: {err}"
            );
        }
        failed += u64::from(fired);
        assert_eq!(
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) },
            0
        );
        fs::write("/proc/sys/vm/drop_caches", b"1").unwrap();
        let mut got = vec![0; data.len()];
        file.read_exact_at(&mut got, 0).unwrap();
        let first = got.iter().zip(&data).position(|(a, b)| a != b);
        assert_eq!(
            first, None,
            "allocation {nth} failing, the fsync {synced:?}: the file dropped differs"
        );
        file.sync_all().unwrap();
        evict(&name);
        file.read_exact_at(&mut got, 0).unwrap();
        assert!(got == data, "allocation {nth} failing: the file evicted, synced again, differs");
    }
    eprintln!("writeback_out_of_memory: {failed} of {NTH} fsyncs had an allocation fail");
    assert!(failed > 0, "no allocation of an fsync's failed");
    fs::remove_file(&name).unwrap();
}
