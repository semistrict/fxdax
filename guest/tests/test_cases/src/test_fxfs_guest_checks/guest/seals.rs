// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The checks of commits and seals: writes racing them, across them, and what they write to the
//! device's log.

use super::*;

/// fxdax: has the kernel log what it traced of the DAX window's page at page frame `frame`, of
/// whichever pages of files (fxfs.dax_trace, TASK-124): where a mapping maps a page that isn't
/// its file's.
fn trace_dump_frame(frame: u64) {
    let traced = fs::read_to_string("/sys/module/fxfs/parameters/dax_trace");
    if traced.is_ok_and(|traced| traced.trim() == "Y") {
        let _ = fs::write("/sys/module/fxfs/parameters/dax_trace_dump", format!("pfn {frame:x}"));
    }
}

/// fxdax: the page frame this process's page table maps `addr` to, if any
/// (/proc/self/pagemap): which page of the DAX window a mapping of a file maps, the window's
/// page frames in a row from its first.
fn mapped_pfn(addr: usize) -> Option<u64> {
    use std::os::unix::fs::FileExt;
    const PRESENT: u64 = 1 << 63;
    const PFN: u64 = (1 << 55) - 1;
    let pagemap = fs::File::open("/proc/self/pagemap").unwrap();
    let mut bytes = [0; 8];
    pagemap.read_exact_at(&mut bytes, (addr / 4096 * 8) as u64).unwrap();
    let entry = u64::from_le_bytes(bytes);
    (entry & PRESENT != 0).then_some(entry & PFN)
}

/// fxdax: dax_writes_ns adds up (TASK-146.2): what writes of a file's every kind of page spent
/// is in the parts /proc/fs/fxfs/<tag> says, the copy among them: a new step in the write would
/// show as dax_write_other_ns. 45% of a pgbench's writes' time had no part to be in. Of a file
/// read in from sealed extents, whose writes are the window's, copying on write (doc-13; a file
/// created here is a page-cache inode, TASK-164.3): its iomaps and its copies. Fails if a
/// write's time is in no part.
pub(super) fn test_write_times_add_up() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    let name = path("write-times");
    cow_base_file(&name, 0, 0);
    let mut file = fs::OpenOptions::new().write(true).open(&name).unwrap();
    let before = stats();
    let data = vec![7u8; 3 * PAGE / 2];
    for round in 0..64 {
        file.write_all(&data).unwrap();
        file.write_all_at(&data, round * PAGE as u64).unwrap();
        if round % 16 == 0 {
            file.sync_all().unwrap();
        }
    }
    let after = stats();
    drop(file);
    fs::remove_file(&name).unwrap();
    let spent = |stat: &str| after[stat] - before[stat];
    let total = spent("dax_writes_ns");
    assert!(spent("dax_write_copy_ns") > 0, "no copying in {total} ns");
    assert!(spent("dax_write_begin_ns") > 0, "no iomaps in {total} ns");
    let other = spent("dax_write_other_ns");
    assert!(other < total / 10, "{other} ns of {total} are in no part of a write");
}

/// fxdax: writes of several of the device's requests each, written back as fsyncs elsewhere
/// seal: the device applied a write in flight while a seal's barrier was on both sides of it,
/// sealing the extent its first request opened, and the rest failed: "extent … is sealed", and
/// EIO (TASK-91.1, the sandbox team's pgbench).
pub(super) fn test_writes_across_seals() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

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
/// of an extent the guest took for open was refused (EPERM) under pgbench (TASK-91.9). Each
/// write reads back as written once fdatasynced, and each file, in the end, as its writes left
/// it (TASK-119.1).
pub(super) fn test_log_writes() {
    use std::os::unix::fs::FileExt;

    const WRITERS: usize = 4;
    const SIZE: usize = 16 << 20;
    const WRITE: usize = 8 << 10;
    let deadline = Instant::now() + Duration::from_secs(10);
    let writers: Vec<_> = (0..WRITERS)
        .map(|n| {
            std::thread::spawn(move || {
                let name = path(&format!("log-{n}"));
                let file = fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(&name)
                    .unwrap();
                file.write_all_at(&vec![0; SIZE], 0).unwrap();
                file.sync_all().unwrap();
                let block = vec![n as u8 + 1; WRITE];
                let mut read = vec![0; WRITE];
                let (mut offset, mut written) = (0, 0);
                while Instant::now() < deadline {
                    file.write_all_at(&block, offset as u64).unwrap();
                    file.sync_data().unwrap();
                    file.read_exact_at(&mut read, offset as u64).unwrap();
                    assert!(read == block, "log-{n}: the write at {offset:#x} reads back other");
                    offset = (offset + WRITE) % SIZE;
                    written = (written + WRITE).min(SIZE);
                }
                for at in (0..SIZE).step_by(WRITE) {
                    file.read_exact_at(&mut read, at as u64).unwrap();
                    let byte = if at < written { n as u8 + 1 } else { 0 };
                    assert!(read.iter().all(|&b| b == byte), "log-{n} at {at:#x}: not {byte}");
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

/// write(2)s and stores through a mapping to the same pages of a file, while its own fsyncs
/// write it back, and another file's seal, again and again. On writable DAX, as the pages
/// moved, a write(2) in the device's gate invalidated their DAX entries, which a fault holding
/// an entry must not wait for the gate with (the seal waits for the write(2), the write(2) for
/// the fault): a deadlock, which the test's timeout catches. fxdax: with DAX, the file is read
/// in from sealed extents (doc-13, TASK-164.3), and its fsyncs write back pages the stores and
/// write(2)s dirty meanwhile, write-protecting them. Each byte has one owner, the write(2)s a
/// page's first, the stores its second: after each of its rounds, the owner finds its byte as
/// it left it, through the mapping and read(2) alike, and the other's no older than it found
/// it last (TASK-119.1). Then each byte holds what was written last, by each in turn alone.
pub(super) fn test_seal_race(dax: bool) {
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
    let file = if dax {
        drop(file);
        read_in_sealed(&path("seal-race"));
        fs::OpenOptions::new().read(true).write(true).open(path("seal-race")).unwrap()
    } else {
        file
    };
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
    let mapped_at = mapped;
    let file = &file;
    // The byte at `at`, through the mapping and through read(2).
    let load = move |at: usize| unsafe { ((mapped + at) as *const u8).read_volatile() };
    let read = move |at: usize| {
        let mut byte = [0];
        file.read_exact_at(&mut byte, at as u64).unwrap();
        byte[0]
    };
    // After its round, the owner of byte `own` of each page checks it, and that the other
    // owner's, `other`, is no older than in `seen`.
    let check = move |what: &str, round: usize, own: usize, other: usize, seen: &mut [u8]| {
        for (n, page) in (0..SIZE).step_by(PAGE).enumerate() {
            let (mapped, read) = (load(page + own), read(page + own));
            // Looked into only where they differ: a page table read for every byte slowed the
            // check, whose seals then filled the window's regions for the checks after.
            if (mapped, read) != (round as u8, round as u8) {
                let frame = mapped_pfn(mapped_at + page);
                trace_dump(&path("seal-race"), n as u64);
                if let Some(frame) = frame {
                    trace_dump_frame(frame);
                }
                assert_eq!(
                    (mapped, read),
                    (round as u8, round as u8),
                    "page {n}: the {what}' byte, mapped and read, after their round {round}; \
                     the mapping maps page frame {frame:x?}"
                );
            }
            let other = load(page + other);
            assert!(
                other >= seen[n],
                "page {n}: the other's byte went back from {} to {other}, after the \
                 {what}' round {round}",
                seen[n]
            );
            seen[n] = other;
        }
    };
    let store = move |round: usize, seen: &mut [u8]| {
        for page in (0..SIZE).step_by(PAGE) {
            unsafe { ((mapped + page + 1) as *mut u8).write_volatile(round as u8) };
        }
        check("stores", round, 1, 0, seen);
    };
    let write = move |round: usize, seen: &mut [u8]| {
        for page in (0..SIZE).step_by(PAGE) {
            file.write_all_at(&[round as u8], page as u64).unwrap();
        }
        check("write(2)s", round, 0, 1, seen);
    };
    let sealing = AtomicBool::new(true);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..SEALS {
                if !sealing.load(Ordering::Relaxed) {
                    break;
                }
                file.sync_data().unwrap();
                fs::write(path("seal-race-other"), b"other").unwrap();
                fs::File::open(path("seal-race-other")).unwrap().sync_all().unwrap();
            }
        });
        let storer = scope.spawn(move || {
            let mut seen = [0; SIZE / PAGE];
            (0..ROUNDS).for_each(|round| store(round, &mut seen));
        });
        let mut seen = [0; SIZE / PAGE];
        (0..ROUNDS).for_each(|round| write(round, &mut seen));
        sealing.store(false, Ordering::Relaxed);
        storer.join().unwrap();
    });

    write(ROUNDS, &mut [0; SIZE / PAGE]);
    store(ROUNDS, &mut [0; SIZE / PAGE]);
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

/// fxdax: files written and fsynced, then truncated or removed at once, again and again
/// (TASK-91.10). The filesystem stays whole: the last file reads back.
pub(super) fn test_dropped_after_seals() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    for round in 0..300 {
        let name = path(&format!("dropped-after-seals-{}", round % 4));
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
    let name = path("dropped-after-seals-last");
    let data = vec![0x3c; 64 * PAGE];
    fs::write(&name, &data).unwrap();
    evict(&name);
    assert!(fs::read(&name).unwrap() == data, "the last file differs from what was written");
    // The even rounds' files, truncated, are left: the odd rounds' were removed.
    for n in [0, 2] {
        fs::remove_file(path(&format!("dropped-after-seals-{n}"))).unwrap();
    }
    fs::remove_file(&name).unwrap();
}
