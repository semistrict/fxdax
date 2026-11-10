// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the checks of appends: the pages, size and writeback of a file growing, mapped or not,
//! and appends stopped short (TASK-133).

use super::*;

/// Sustained page-sized appends, never publishing a reserved tail as file data: sync,
/// eviction, cloning, growing and shrinking preserve the written bytes.
pub(super) fn test_append_pages() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const APPENDS: usize = 48;
    let name = path("append-pages");
    let file = fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap();
    let mut expected = vec![0x55; 100];
    file.write_all_at(&expected, 0).unwrap();
    file.sync_data().unwrap();
    // An unsealed append stream, as SQLite's WAL makes between barriers.
    for _ in 0..2 {
        file.write_all_at(&vec![0x33; PAGE], expected.len() as u64).unwrap();
        expected.extend_from_slice(&vec![0x33; PAGE]);
    }
    for n in 0..APPENDS {
        let bytes = vec![n as u8 + 1; PAGE];
        file.write_all_at(&bytes, expected.len() as u64).unwrap();
        expected.extend_from_slice(&bytes);
    }
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

/// Appends survive an overwrite after another file seals, and an ordinary grow or shrink uses
/// the appended size rather than zeroing data past Fxfs's former end (TASK-92.28).
pub(super) fn test_append_size() {
    use std::os::unix::fs::FileExt;

    let seal = fs::File::create(path("append-size-seal")).unwrap();
    for operation in ["overwrite", "grow", "shrink", "clone"] {
        let name = path(&format!("append-size-{operation}"));
        let file =
            fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap();
        let mut expected = vec![0x11; 100];
        file.write_all_at(&expected, 0).unwrap();
        file.sync_data().unwrap();
        // Appends of a byte each.
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
        // Read before fsync too: writing the size later cannot repair bytes zeroed.
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
/// the last deferred append when fdatasync returns (TASK-92.28). fxdax: and an fdatasync costs the
/// guest little: its time but for the host's (TASK-323).
pub(super) fn test_append_writeback() {
    use std::os::unix::fs::FileExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let name = path("append-writeback");
    let file = Arc::new(
        fs::OpenOptions::new().create_new(true).read(true).write(true).open(&name).unwrap(),
    );
    // A round: the file cut, written, appended to a byte at a time, cut again, synced and read.
    // What it holds, its fdatasync's time, and the host's time meanwhile.
    let round = |round: u8| {
        file.set_len(0).unwrap();
        let mut expected = vec![round; 100];
        file.write_all_at(&expected, 0).unwrap();
        for _ in 0..32 {
            file.write_all_at(&[0xa5], expected.len() as u64).unwrap();
            expected.push(0xa5);
        }
        file.set_len(120).unwrap();
        expected.truncate(120);
        let (began, host) = (Instant::now(), busy());
        file.sync_data().unwrap();
        let (took, host) = (began.elapsed(), busy() - host);
        assert_eq!(fs::read(&name).unwrap(), expected);
        (expected, took, host)
    };
    let stop = Arc::new(AtomicBool::new(false));
    let syncer = {
        let (file, stop) = (file.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                assert_eq!(unsafe { libc::syncfs(file.as_raw_fd()) }, 0);
            }
        })
    };
    let (mut syncing, mut longest) = (Duration::ZERO, Duration::ZERO);
    let host = Host::now();
    for n in 0..64 {
        let (_, took, _) = round(n);
        (syncing, longest) = (syncing + took, longest.max(took));
    }
    stop.store(true, Ordering::Relaxed);
    syncer.join().unwrap();
    // fxdax: where its time goes, against its budget (TASK-168), and what was asked of the host,
    // with the syncfs's, and what it took, which the budget leaves out (TASK-263).
    eprintln!(
        "append_writeback: 64 fdatasyncs in {syncing:.2?}, the longest {longest:.2?} ({})",
        host.since()
    );
    // fxdax: the same rounds, alone. An fdatasync's guest time is its time but for the device's
    // meanwhile, its own requests', whatever the host's disk took for them. Above, the syncfs's
    // requests are always in flight, so all of the time is the host's, which the budget leaves
    // out, and a sleep of 60 ms in each fsync passed (TASK-323).
    let mut expected = Vec::new();
    let mut guest: Vec<Duration> = (0..ALONE)
        .map(|n| {
            let (bytes, took, host) = round(64 + n);
            expected = bytes;
            took.saturating_sub(host)
        })
        .collect();
    guest.sort();
    let (median, longest) = (guest[guest.len() / 2], guest[guest.len() - 1]);
    eprintln!(
        "append_writeback: {ALONE} fdatasyncs alone, the guest's time the median {median:.2?}, \
         the longest {longest:.2?}, all told {:.2?}",
        guest.iter().sum::<Duration>()
    );
    let bound = FDATASYNC.mul_f64(scale());
    assert!(median <= bound, "an fdatasync's guest time, the median, {median:.2?} of {bound:.2?}");
    drop(file);
    evict(&name);
    assert_eq!(fs::read(&name).unwrap(), expected, "after eviction");
    fs::remove_file(name).unwrap();
}

/// fxdax: how many of append_writeback's rounds it times alone (TASK-323).
const ALONE: u8 = 32;

/// fxdax: the most of the guest's time the median of append_writeback's fdatasyncs alone may take,
/// scaled as the budgets are ([`scale`]): 50 times the most measured. TASK-323, on ubu1, release
/// kernel, 55 rounds (-dax, -dax-small and -dax-delay, rounds on ZFS and on ext4): the median
/// 59-103 us, the longest 2.4 ms, all 32 at most 5.8 ms, while the host took 1.3-14.6 s. A sleep in
/// each fsync of 60 ms made the median 62.9 ms (guest/mutants/task-323-fsync-sleeps.patch), of
/// 6 ms 11.1 ms. The median, not the sum: in most rounds one or two of the 32 took about 2 ms, the
/// rest about 0.1 ms.
const FDATASYNC: Duration = Duration::from_millis(5);

/// fxdax: how long the bound device has had a request in flight, all told (`busy_ns`).
fn busy() -> Duration {
    crate::extent_driver::busy().expect("the device bound")
}

/// fxdax: a file appended to across its chunks while another thread maps it and reads it
/// all, again and again, for a few seconds: each append writes. A fault mapped the chunk again
/// as small as the file was, between the write making room for it and writing, which the write
/// found too small: EIO, and a WARN (TASK-79.28). Each page mapped holds what was appended
/// there, and the file what was appended (TASK-119.1).
pub(super) fn test_append_while_mapped() {
    use std::sync::atomic::{AtomicBool, Ordering};
    const STEP: usize = 192 << 10;
    let name = path("append-mapped");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&name)
        .unwrap();
    let stop = AtomicBool::new(false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    let appended = std::thread::scope(|scope| {
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
                    let byte = unsafe { map.cast::<u8>().add(page).read_volatile() };
                    assert_eq!(byte, (page / STEP) as u8, "byte {page:#x} of {len:#x} mapped");
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
        n as usize
    });
    let read = fs::read(&name).unwrap();
    assert_eq!(read.len(), appended * STEP);
    if let Some(at) = read.iter().enumerate().position(|(at, &b)| b != (at / STEP) as u8) {
        panic!("byte {at:#x} reads {:#x}, not what was appended there", read[at]);
    }
    drop(file);
    fs::remove_file(&name).unwrap();
}

/// fxdax: an append grows the file as it ends, its bytes written, not as it begins: one paused
/// with its iomap made, before its copy (WRITE_MAPPED), leaves the file as long as it was, so
/// that a reader finding the file longer reads what was appended there, never zeros
/// (append_while_mapped, TASK-133). Of a file read in again from sealed extents (created here,
/// it is a page-cache inode until then, doc-13, TASK-164.3), whose append pauses with its pages
/// made private, before its copy.
pub(super) fn test_append_size_follows_data() {
    use std::io::Write as _;
    const OLD: usize = 4096;
    const APPENDED: usize = 64 << 10;
    let name = path("append-size");
    fs::write(&name, vec![1; OLD]).unwrap();
    read_in_sealed(&name);
    let (pauses, only) = Pauses::begin_of(Pauses::WRITE_MAPPED, &name);
    std::thread::scope(|scope| {
        let appender = scope.spawn(|| {
            let mut file = fs::OpenOptions::new().append(true).open(&name).unwrap();
            file.write_all(&vec![2; APPENDED]).unwrap();
        });
        Pauses::wait(Pauses::WRITE_MAPPED);
        let len = fs::metadata(&name).unwrap().len() as usize;
        assert_eq!(len, OLD, "the file grew before its append wrote there");
        drop(pauses);
        appender.join().unwrap();
    });
    drop(only);
    let read = fs::read(&name).unwrap();
    assert_eq!(read.len(), OLD + APPENDED);
    assert!(read[..OLD].iter().all(|&byte| byte == 1), "the file's bytes before the append");
    assert!(read[OLD..].iter().all(|&byte| byte == 2), "the append's bytes");
    fs::remove_file(&name).unwrap();
}

/// fxdax: an append whose user memory ends part way through it stops there, and leaves the
/// file as long as it wrote, in Fxfs as in the inode, which is read in again here: Fxfs took
/// the size the write asked for as it began (TASK-133).
pub(super) fn test_append_stopped_short() {
    const PAGE: usize = 4096;
    let name = path("append-short");
    fs::write(&name, vec![1; PAGE]).unwrap();
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
    unsafe { std::ptr::write_bytes(map.cast::<u8>(), 2, PAGE) };
    assert_eq!(unsafe { libc::munmap(map.cast::<u8>().add(PAGE).cast(), PAGE) }, 0);
    let file = fs::OpenOptions::new().append(true).open(&name).unwrap();
    let wrote = unsafe { libc::write(file.as_raw_fd(), map.cast(), 2 * PAGE) };
    assert_eq!(wrote, PAGE as isize, "write: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, PAGE) }, 0);
    drop(file);
    assert_eq!(fs::metadata(&name).unwrap().len(), 2 * PAGE as u64);
    evict(&name);
    let read = fs::read(&name).unwrap();
    assert_eq!(read.len(), 2 * PAGE);
    assert!(read[..PAGE].iter().all(|&byte| byte == 1), "the file before the append");
    assert!(read[PAGE..].iter().all(|&byte| byte == 2), "the append");
    fs::remove_file(&name).unwrap();
}

/// fxdax: as `test_append_stopped_short`, stopping part way through a page, with more
/// appended after it and no seal between. Cutting the size Fxfs took for the write, Fxfs zeroed
/// what the last block holds past the new end by moving the block, which the window went on
/// mapping where it was, writable: the next append's bytes went to a block freed, and read as
/// zeros once the file was mapped again (TASK-150.9).
pub(super) fn test_append_stopped_short_in_a_page() {
    use std::os::unix::fs::FileExt;
    const PAGE: usize = 4096;
    const READABLE: usize = 5000;
    const AFTER: usize = 100;
    let name = path("append-short-in-a-page");
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(&name).unwrap();
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            3 * PAGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    unsafe { std::ptr::write_bytes(map.cast::<u8>(), 2, 2 * PAGE) };
    assert_eq!(unsafe { libc::munmap(map.cast::<u8>().add(2 * PAGE).cast(), PAGE) }, 0);
    let source = unsafe { map.cast::<u8>().add(2 * PAGE - READABLE) };
    let wrote = unsafe { libc::write(file.as_raw_fd(), source.cast(), 2 * PAGE) };
    assert_eq!(wrote, READABLE as isize, "write: {}", std::io::Error::last_os_error());
    assert_eq!(unsafe { libc::munmap(map, 2 * PAGE) }, 0);
    (&file).write_all(&[3; AFTER]).unwrap();
    file.set_len(2 * PAGE as u64).unwrap();
    let mut expected = vec![0; 2 * PAGE];
    expected[..READABLE].fill(2);
    expected[READABLE..READABLE + AFTER].fill(3);
    let mut read = vec![0; 2 * PAGE];
    file.read_exact_at(&mut read, 0).unwrap();
    assert!(read == expected, "after the append: {:?}", &read[READABLE - 2..READABLE + 2]);
    drop(file);
    evict(&name);
    assert!(fs::read(&name).unwrap() == expected, "the file evicted");
    fs::remove_file(&name).unwrap();
}

/// fxdax: appends that end part way through a page leave no blocks past the file's end, which
/// a crash would leave as a clean unmount does: the file is left for the host's fsck, which
/// writable DAX's runs reserved past the end failed (TASK-92.58, TASK-133), and its contents.
/// Shrunk first, as those reservations stopped at the end of the file's range.
pub(super) fn test_append_reserved_tail() {
    use std::os::unix::fs::{FileExt, MetadataExt};
    const PAGE: usize = 4096;
    let name = path(APPEND_RESERVED);
    let file = fs::File::create_new(&name).unwrap();
    file.write_all_at(&vec![0x77; 1 << 20], 0).unwrap();
    file.sync_data().unwrap();
    let expected = append_reserved();
    file.set_len((expected.len() - 3 * PAGE) as u64).unwrap();
    file.write_all_at(&expected[..expected.len() - 3 * PAGE], 0).unwrap();
    file.sync_data().unwrap();
    // Two pages make an append stream, as in append_pages; the third ends it.
    for page in (1..=3).rev() {
        let at = expected.len() - page * PAGE;
        file.write_all_at(&expected[at..at + PAGE], at as u64).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    evict(&name);
    assert_eq!(fs::read(&name).unwrap(), expected);
    let allocated = fs::metadata(&name).unwrap().blocks() * 512;
    // fxdax: created here, the file is a page-cache inode (doc-13, TASK-164.3), whose
    // writeback allocates the blocks it writes, and reserves none past them.
    assert_eq!(allocated, (expected.len() as u64).next_multiple_of(PAGE as u64));
}
