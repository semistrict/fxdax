// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Test Fxfs in the guest's kernel (fxdax's TASK-26.1 and TASK-26.6).
//
// The host boots the kernel fxdax's scripts/build-guest-kernel.sh builds, which
// FXDAX_GUEST_KERNEL names, with an extent-store device on a new store. The
// guest mounts the store's filesystem with the kernel's Fxfs, writes, links,
// renames and removes, unmounts and mounts again. After the VM exits, the host
// mounts the store with its own Fxfs and finds what the guest wrote.
//
// fxfs-guest: the host makes an fxdax filesystem on the store first, with a
// file, and a directory holding a larger file, which the guest lists and reads.
// fxfs-guest-format: the store holds nothing, and the guest's Fxfs formats it.
// fxfs-guest-crash: the guest formats the store, fsyncs file after file,
// reporting each once its fsync returns, while another thread does too, and
// resets the VM in the middle, without syncing. The host checks the store's
// filesystem (fsck), and finds every file the guest reported, whole.
// fxfs-guest-dax and fxfs-guest-crash-dax: the same with a DAX window, which
// the guest's Fxfs maps files' data from (on hosts whose pages are 4 KiB,
// Fxfs's blocks): files are DAX, and reading one maps it rather than reading it.

use macros::{guest, host};

/// How a test makes and leaves the filesystem.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    HostFormats,
    GuestFormats,
    Crash,
    /// fxdax: writers fsyncing files while the kernel fails their allocations (TASK-44.10).
    Faults,
}

pub struct TestFxfsGuest {
    mode: Mode,
    /// Whether the device has a DAX window.
    dax: bool,
    /// fxdax: the step of the DAX protocol (fxfs.dax_pause's bit) a crash test's VM stops with an
    /// evacuation held at, if any (TASK-119.16).
    crash_step: Option<u32>,
    /// fxdax: whether a crash test's VM process is killed (SIGKILL), rather than its guest reset
    /// (TASK-119.6).
    killed: bool,
}

impl TestFxfsGuest {
    /// fxfs-guest.
    pub fn host_formats() -> Self {
        Self { mode: Mode::HostFormats, dax: false, crash_step: None, killed: false }
    }

    /// fxfs-guest-format.
    pub fn guest_formats() -> Self {
        Self { mode: Mode::GuestFormats, dax: false, crash_step: None, killed: false }
    }

    /// fxfs-guest-crash.
    pub fn crash() -> Self {
        Self { mode: Mode::Crash, dax: false, crash_step: None, killed: false }
    }

    /// fxfs-guest-crash-dax-STEP: the crash test with DAX, its VM stopped with an evacuation of a
    /// file held at `step` (fxfs.dax_pause's bit): its runs gathered, mapped in their ranges' own
    /// space, or placed there (TASK-119.16). What the window shows a warm reader agrees with the
    /// window; the store, which a crash leaves, must hold every fsync acknowledged as well.
    pub fn crash_at(step: u32) -> Self {
        Self { crash_step: Some(step), ..Self::crash().with_dax() }
    }

    /// fxdax: fxfs-guest-crash-dax-killed: the crash test with DAX, its VM process killed
    /// (SIGKILL) where its guest would reset it, as a host's crash would end it, the extent
    /// daemon serving the store on, its requests in flight cut off (TASK-119.6).
    pub fn crash_killed() -> Self {
        Self { killed: true, ..Self::crash().with_dax() }
    }

    /// fxdax: fxfs-guest-faults-dax: two writers create, write and fsync files, as the crash
    /// test's do, while the guest's kernel fails their allocations at random, slabs' and pages'
    /// (failslab, fail_page_alloc, its debug build's: FXDAX_GUEST_KERNEL_BUILD=debug), their
    /// calls failing as they may; the filesystem complains of nothing, and the store holds every
    /// file whose fsync returned (TASK-44.10).
    pub fn faults() -> Self {
        Self { mode: Mode::Faults, dax: true, crash_step: None, killed: false }
    }

    /// The same test with a DAX window.
    pub fn with_dax(self) -> Self {
        Self { dax: true, ..self }
    }
}

/// The DAX window's size, when the device has one.
#[cfg_attr(feature = "guest", allow(dead_code))]
const WINDOW: u64 = 64 << 20;

/// The contents of the crash test's file `index`, of `writer`: of a size and pattern its name
/// gives, which the host checks.
fn durable(writer: u8, index: usize) -> Vec<u8> {
    pattern(4096 * (index % 37) + 13 * index + 1, writer.wrapping_mul(31) ^ index as u8)
}

/// The bytes of the unnamed file (O_TMPFILE) the crash test's guest leaves open as its VM stops,
/// which its store's next mount frees (TASK-26.19.1): large enough that what is left of it once
/// freed, half, stands out from the store's metadata (some 13 MB once the writers ran).
const UNNAMED: usize = 48 << 20;

/// fxdax: the crash test's file, a page of `mapped_synced()`, synced, which its guest then stores
/// a byte to through a mapping, at MAPPED_STORE, as the VM stops (TASK-79.11).
const MAPPED_SYNCED: &str = "mapped-synced";
const MAPPED_STORE: usize = 100;

/// fxdax: the crash test's files whose pages are written again and again, each write then synced
/// (TASK-119.6): through write(2) and fsync, or stores through a mapping and msync.
const REWRITTEN: [&str; 2] = ["rewritten", "rewritten-mapped"];
const REWRITTEN_PAGES: usize = 16;

/// The contents of the page `page` of a REWRITTEN file as its write `generation` writes it.
fn rewritten(page: usize, generation: u32) -> Vec<u8> {
    let g = generation.to_le_bytes();
    (0..4096).map(|i| (i % 251) as u8 ^ page as u8 ^ g[0] ^ g[1].rotate_left(3)).collect()
}

/// fxdax: the crash test's file truncated and extended again and again, each size then synced
/// (TASK-119.6): wherever it holds bytes, they are `resized_byte`'s, but for those an extension not
/// yet synced left zeros.
const RESIZED: &str = "resized";

/// The sizes RESIZED is given are multiples of this, which pages' and blocks' ends fall within.
const RESIZED_STEP: usize = 3001;

/// The size resize `generation` gives RESIZED: across pages and blocks, shrinking and growing it.
fn resized_size(generation: u32) -> usize {
    (generation as usize * 7919 % 16 + 1) * RESIZED_STEP
}

/// RESIZED's byte at `at`, which no zero is.
fn resized_byte(at: usize) -> u8 {
    (at % 251) as u8 + 1
}

/// fxdax: the line the killed crash test's guest says where it would reset its VM, at which the
/// host kills the VM's process (TASK-119.6).
const KILL_ME: &str = "kill me";

/// The contents of MAPPED_SYNCED as synced.
fn mapped_synced() -> Vec<u8> {
    pattern(4096, 7)
}

/// The small file's contents.
#[cfg_attr(feature = "host", allow(dead_code))]
const HELLO: &[u8] = b"Hello from the host's Fxfs\n";

/// The larger file's size: many blocks, and not a whole number of them.
#[cfg_attr(feature = "host", allow(dead_code))]
const NESTED_SIZE: usize = (2 << 20) + 12345;

/// The larger file's contents: a pattern no block repeats.
#[cfg_attr(feature = "host", allow(dead_code))]
fn nested() -> Vec<u8> {
    pattern(NESTED_SIZE, 0)
}

/// `len` bytes of a pattern no block repeats, which `seed` varies.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ (i / 4096) as u8 ^ seed).collect()
}

/// What the guest writes, which the host reads back: more than one of the kernel's
/// chunks of writes, and not a whole number of blocks, then an append.
fn written() -> Vec<u8> {
    let mut data = pattern((3 << 20) + 777, 0x5a);
    data.extend_from_slice(b"appended\n");
    data
}

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::{Filesystem, Owner, Storage};

    /// The store's capacity: room for the crash test's unnamed file too.
    const CAPACITY: u64 = 128 << 20;

    /// The files whose fsync the guest says returned, its writer and index, as `output` says.
    fn synced_files(output: &str) -> Vec<(u8, usize)> {
        (output.lines())
            .filter_map(|line| line.strip_prefix("synced "))
            // A line the VM stopped in the middle of: of a file a line before names too.
            .filter_map(|file| {
                let (writer, index) = file.split_once(' ')?;
                Some((writer.parse().ok()?, index.parse().ok()?))
            })
            .collect()
    }

    /// Asserts `fs` holds each of the files `synced` names whole, as its writer wrote it.
    fn assert_synced(fs: &Filesystem, synced: &[(u8, usize)]) {
        for &(writer, index) in synced {
            let name = format!("synced-{writer}-{index}");
            let (file, attr) = block_on(fs.root().lookup(name.as_bytes())).unwrap();
            let expected = durable(writer, index);
            assert_eq!(attr.size, expected.len() as u64, "{name}");
            let mut contents = vec![0; expected.len()];
            let mut read = 0;
            while read < contents.len() {
                let n = block_on(file.read(read as u64, &mut contents[read..])).unwrap();
                assert_ne!(n, 0, "{name} ends early, at {read}");
                read += n;
            }
            assert!(contents == expected, "{name} holds other data");
        }
    }

    impl Test for TestFxfsGuest {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            // fxdax: the crash test held at a step waits up to 10 s for an evacuation there.
            if self.crash_step.is_some() { 40 } else { 15 }
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let path = test_setup.tmp_dir.join("extents");
            if self.mode == Mode::HostFormats {
                block_on(async {
                    let fs = Filesystem::format(Storage::create(&path, CAPACITY)?).await?;
                    let owner = Owner { uid: 0, gid: 0 };
                    let (hello, _) = fs.root().create(b"hello", 0o644, owner).await?;
                    hello.write(0, HELLO).await?;
                    let (dir, _) = fs.root().mkdir(b"dir", 0o750, owner).await?;
                    let (file, _) = dir.create(b"nested", 0o600, owner).await?;
                    file.write(0, &nested()).await?;
                    fs.close().await?;
                    anyhow::Ok(())
                })?;
            }

            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            // fxdax: holding an evacuation, a window whose regions the writers don't fill, as
            // they fill a small one's, after which their pages move nowhere to evacuate from.
            let window = match (self.dax, self.crash_step) {
                (false, _) => 0,
                (true, None) => WINDOW,
                (true, Some(_)) => 1 << 29,
            };
            vm.extent_store("extents", &path, CAPACITY, window)?;
            if self.mode == Mode::Crash {
                // fxdax: when the VM stops, from when its writers begin: FXDAX_CRASH_MS, to replay
                // a run, or else a time of the run's own, before the writers fill the store, and
                // with DAX after its store through a mapping (TASK-119.6).
                let first = if self.dax { 1100 } else { 300 };
                let crash_ms = std::env::var("FXDAX_CRASH_MS").ok().and_then(|ms| ms.parse().ok());
                let crash_ms: u64 = crash_ms.unwrap_or_else(|| {
                    let now =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
                    first + now.subsec_nanos() as u64 % (2000 - first)
                });
                eprintln!("{}: crash at {crash_ms} ms (FXDAX_CRASH_MS)", test_setup.test_case);
                vm.env(&std::ffi::CString::new(format!("FXDAX_CRASH_MS={crash_ms}"))?);
                if self.killed {
                    vm.kill_on(KILL_ME);
                }
            }
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            // fxdax: as the guest wrote it, a stop's kernel lines in its midst gone (GCE r248).
            let output = crate::vm::without_kernel_lines(&String::from_utf8(stdout).unwrap());
            let path = test_setup.tmp_dir.join("extents");
            // fxdax: the writers' calls failed, some of them, as their allocations did; the store
            // holds every file whose fsync returned, whole (TASK-44.10).
            if self.mode == Mode::Faults {
                // Calls may all succeed, allocations failing where the code falls back: that the
                // kernel forced failures, its log says.
                let forced: Option<u64> = (output.lines())
                    .find_map(|line| line.strip_prefix("forced failures ")?.parse().ok());
                let synced = synced_files(&output);
                if forced.is_none_or(|forced| forced == 0) || synced.is_empty() {
                    return TestOutcome::Fail(format!(
                        "no allocation failed, or no fsync returned: {output:?}"
                    ));
                }
                let storage = block_on(Storage::open(&path).unwrap().check()).unwrap();
                let fs = block_on(Filesystem::mount(storage)).unwrap();
                assert_synced(&fs, &synced);
                block_on(fs.close()).unwrap();
                return TestOutcome::Pass;
            }
            if self.mode == Mode::Crash {
                // The guest stopped mid-workload: its filesystem is consistent, and holds every
                // file whose fsync returned before it did.
                let synced = synced_files(&output);
                if synced.len() < 10 {
                    return TestOutcome::Fail(format!("too few fsyncs returned: {output:?}"));
                }
                // fxdax: the VM stopped with an evacuation held at the step asked (TASK-119.16).
                if let Some(step) = self.crash_step
                    && !output.lines().any(|line| line == format!("held at step {step}"))
                {
                    return TestOutcome::Fail(format!(
                        "no evacuation was held at step {step}: {output:?}"
                    ));
                }
                // The store as the crash left it, for a failure to be looked into: checking it
                // recovers it, and mounting it replays its journal.
                crate::common::copy_store(&path, &test_setup.tmp_dir.join("extents-at-crash"))
                    .unwrap();
                let storage = block_on(Storage::open(&path).unwrap().check()).unwrap();
                let fs = block_on(Filesystem::mount(storage)).unwrap();
                assert_synced(&fs, &synced);
                // fxdax: each page written again and again holds, byte by byte, what one of its
                // writes since the last synced (that one's included) wrote: none torn into others',
                // none older (TASK-119.6).
                for (file, name) in REWRITTEN.iter().enumerate() {
                    let (node, attr) = block_on(fs.root().lookup(name.as_bytes())).unwrap();
                    assert_eq!(attr.size, (REWRITTEN_PAGES * 4096) as u64, "{name}'s size");
                    for page in 0..REWRITTEN_PAGES {
                        let written: Vec<(u32, bool)> = (output.lines())
                            .filter_map(|line| {
                                let (synced, rest) = match line.strip_prefix("rewrote ") {
                                    Some(rest) => (true, rest),
                                    None => (false, line.strip_prefix("rewriting ")?),
                                };
                                // The VM may stop mid-line: a line cut short is of a write
                                // not begun, or a sync whose write the line before names.
                                let fields: Vec<usize> = rest
                                    .split(' ')
                                    .map(|n| n.parse().ok())
                                    .collect::<Option<_>>()?;
                                (fields.len() == 3 && fields[0] == file && fields[1] == page)
                                    .then_some((fields[2] as u32, synced))
                            })
                            .collect();
                        let last_synced = (written.iter())
                            .filter(|(_, synced)| *synced)
                            .map(|&(generation, _)| generation)
                            .max()
                            .unwrap_or_else(|| panic!("{name} page {page} never synced"));
                        let allowed: Vec<Vec<u8>> = (written.iter())
                            .filter(|&&(generation, synced)| !synced && generation >= last_synced)
                            .map(|&(generation, _)| rewritten(page, generation))
                            .chain([rewritten(page, last_synced)])
                            .collect();
                        let mut contents = vec![0; 4096];
                        let read =
                            block_on(node.read((page * 4096) as u64, &mut contents)).unwrap();
                        assert_eq!(read, 4096, "{name} page {page} ends early");
                        for (at, byte) in contents.iter().enumerate() {
                            assert!(
                                allowed.iter().any(|allowed| allowed[at] == *byte),
                                "{name} page {page} byte {at} holds {byte:#x}: no write since \
                                 generation {last_synced}, the last synced, wrote it"
                            );
                        }
                    }
                }
                // fxdax: the file resized again and again is as large as its last resize synced
                // made it, or one after; its bytes the pattern's below both, and the pattern's or
                // an extension's zeros above (TASK-119.6).
                {
                    let resizes: Vec<(u32, usize, bool)> = (output.lines())
                        .filter_map(|line| {
                            let (synced, rest) = match line.strip_prefix("resized ") {
                                Some(rest) => (true, rest),
                                None => (false, line.strip_prefix("resizing ")?),
                            };
                            let (generation, size) = rest.split_once(' ')?;
                            let (generation, size) = (generation.parse().ok()?, size.parse().ok()?);
                            // A line the VM stopped in the middle of says another size.
                            (size == resized_size(generation)).then_some((generation, size, synced))
                        })
                        .collect();
                    let (last_synced, synced_size) = (resizes.iter())
                        .filter(|(_, _, synced)| *synced)
                        .map(|&(generation, size, _)| (generation, size))
                        .max()
                        .expect("RESIZED never synced");
                    let allowed: Vec<usize> = (resizes.iter())
                        .filter(|&&(generation, _, synced)| !synced && generation > last_synced)
                        .map(|&(_, size, _)| size)
                        .chain([synced_size])
                        .collect();
                    let (node, attr) = block_on(fs.root().lookup(RESIZED.as_bytes())).unwrap();
                    let size = attr.size as usize;
                    assert!(
                        allowed.contains(&size),
                        "{RESIZED} is {size} bytes: no resize since generation {last_synced}, \
                         the last synced, of {synced_size}, made it so: {allowed:?}"
                    );
                    let mut contents = vec![0; size];
                    let mut read = 0;
                    while read < size {
                        let n = block_on(node.read(read as u64, &mut contents[read..])).unwrap();
                        assert_ne!(n, 0, "{RESIZED} ends early, at {read}");
                        read += n;
                    }
                    let kept = allowed.iter().copied().min().unwrap();
                    for (at, &byte) in contents.iter().enumerate() {
                        assert!(
                            byte == resized_byte(at) || (at >= kept && byte == 0),
                            "{RESIZED} byte {at} of {size} holds {byte:#x}: neither the pattern's \
                             nor, past {kept}, an extension's zero"
                        );
                    }
                }
                // fxdax: a page synced, then stored to through a mapping, the fault's block moved
                // and a seal right after: it holds what was synced, but for the byte stored, which
                // it may or may not hold. It held zeros (TASK-79.11).
                if self.dax {
                    assert!(
                        output.lines().any(|line| line == "stored mapped"),
                        "the guest didn't store to {MAPPED_SYNCED}: {output:?}"
                    );
                    let (file, _) = block_on(fs.root().lookup(MAPPED_SYNCED.as_bytes())).unwrap();
                    let mut contents = vec![0; 4096];
                    let read = block_on(file.read(0, &mut contents)).unwrap();
                    let expected = mapped_synced();
                    assert_eq!(read, expected.len(), "{MAPPED_SYNCED} ends early");
                    contents[MAPPED_STORE] = expected[MAPPED_STORE];
                    assert!(contents == expected, "{MAPPED_SYNCED} holds other data");
                }
                // The unnamed file open as the VM stopped, which nothing names: freed once the
                // store is mounted again, its blocks with it, as the graveyard frees them.
                let names: Vec<Vec<u8>> = (block_on(fs.root().entries()).unwrap().into_iter())
                    .map(|entry| entry.name)
                    .collect();
                let named: u64 = (names.iter())
                    .map(|name| block_on(fs.root().lookup(name)).unwrap().1.size)
                    .sum();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    let statfs = block_on(fs.statfs()).unwrap();
                    let used = (statfs.blocks - statfs.blocks_free) * statfs.block_size as u64;
                    // Less than half the unnamed file's more than what names hold: metadata.
                    if used < named + UNNAMED as u64 / 2 {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{used} bytes used, {named} named: the unnamed file was not freed"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                block_on(fs.close()).unwrap();
                return TestOutcome::Pass;
            }
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
            }
            // What the guest wrote is in the store, for the host's Fxfs as for the guest's.
            let fs = block_on(Filesystem::mount(Storage::open(&path).unwrap())).unwrap();
            let lookup = |parent: &fxdax::Node, name: &str| {
                block_on(parent.lookup(name.as_bytes())).unwrap()
            };
            let (dir, _) = lookup(fs.root(), "guest-dir");
            let (file, attr) = lookup(&dir, "renamed");
            assert_eq!(attr.size, written().len() as u64);
            assert_eq!(attr.perm, 0o640);
            assert_eq!(attr.nlink, 2);
            let mut contents = vec![0; written().len()];
            let mut read = 0;
            while read < contents.len() {
                let n = block_on(file.read(read as u64, &mut contents[read..])).unwrap();
                assert_ne!(n, 0, "the file ends early, at {read}");
                read += n;
            }
            assert_eq!(contents, written());
            assert_eq!(lookup(fs.root(), "link").1.inode, attr.inode);
            let (symlink, _) = lookup(fs.root(), "symlink");
            assert_eq!(block_on(symlink.readlink()).unwrap(), b"guest-dir/renamed");
            // Removed by the guest, or never made.
            assert!(block_on(fs.root().lookup(b"hello")).is_err());
            block_on(fs.close()).unwrap();
            TestOutcome::Pass
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::{CStr, CString};
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// What the guest writes through a mapping: three pages and a bit.
    fn mapped() -> Vec<u8> {
        pattern(3 * 4096 + 100, 0xa5)
    }

    const MNT: &str = "/mnt/fxfs";

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

    fn mount(source: &str) -> std::io::Result<()> {
        mount_at(source, MNT)
    }

    fn mount_at(source: &str, target: &str) -> std::io::Result<()> {
        let source = CString::new(source).unwrap();
        let target = CString::new(target).unwrap();
        let fstype = CString::new("fxfs").unwrap();
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    fn unmount() {
        unmount_at(MNT)
    }

    fn unmount_at(target: &str) {
        let target = CString::new(target).unwrap();
        let ret = unsafe { libc::umount(target.as_ptr()) };
        assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
    }

    /// Makes `dir`, and in it what `check_written` finds.
    fn write(dir: &str) {
        fs::create_dir(dir).unwrap();
        assert!(fs::metadata(dir).unwrap().is_dir());
        assert_eq!(fs::create_dir(dir).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);

        let path = format!("{dir}/file");
        let data = written();
        let (body, appended) = data.split_at(data.len() - b"appended\n".len());
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(body).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(appended).unwrap();
        drop(file);
        assert_eq!(fs::read(&path).unwrap(), data);

        // Truncation: shrinking, then growing with zeros.
        let scratch = format!("{dir}/scratch");
        fs::write(&scratch, b"0123456789").unwrap();
        let file = fs::OpenOptions::new().write(true).open(&scratch).unwrap();
        file.set_len(4).unwrap();
        assert_eq!(fs::read(&scratch).unwrap(), b"0123");
        file.set_len(6).unwrap();
        assert_eq!(fs::read(&scratch).unwrap(), b"0123\0\0");
        // And with what the file held written back and dropped from memory in between, so that
        // the zeros come from the filesystem.
        file.sync_all().unwrap();
        file.set_len(2).unwrap();
        file.sync_all().unwrap();
        fs::write("/proc/sys/vm/drop_caches", b"1").unwrap();
        file.set_len(5).unwrap();
        assert_eq!(fs::read(&scratch).unwrap(), b"01\0\0\0");
        drop(file);
        fs::remove_file(&scratch).unwrap();
        assert_eq!(fs::metadata(&scratch).unwrap_err().kind(), std::io::ErrorKind::NotFound);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        fs::hard_link(&path, format!("{MNT}/link")).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 2);
        fs::rename(&path, format!("{dir}/renamed")).unwrap();
        std::os::unix::fs::symlink("guest-dir/renamed", format!("{MNT}/symlink")).unwrap();
        // A target as long as Linux's PATH_MAX - 1, which Fxfs keeps outside the symlink's
        // record, and one longer, which it refuses.
        let long_target = "a/".repeat(2047) + "z";
        std::os::unix::fs::symlink(&long_target, format!("{dir}/long-link")).unwrap();
        assert_eq!(
            fs::read_link(format!("{dir}/long-link")).unwrap(),
            std::path::Path::new(&long_target)
        );
        assert_eq!(
            std::os::unix::fs::symlink(long_target + "z", format!("{dir}/longer-link"))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENAMETOOLONG)
        );
        fs::remove_file(format!("{dir}/long-link")).unwrap();

        // A directory emptied while it is listed, as `rm -r` empties it: no entry is skipped,
        // though removing those listed before it moves it, and each listing reads several.
        let listed = format!("{dir}/listed");
        fs::create_dir(&listed).unwrap();
        for i in 0..2000 {
            fs::write(format!("{listed}/entry-with-a-long-name-{i}"), b"").unwrap();
        }
        let mut removed = 0;
        for entry in fs::read_dir(&listed).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
            removed += 1;
        }
        assert_eq!(removed, 2000);
        fs::remove_dir(&listed).unwrap();

        // A directory, removed once empty.
        let gone = format!("{dir}/gone");
        fs::create_dir(&gone).unwrap();
        fs::write(format!("{gone}/f"), b"x").unwrap();
        assert_eq!(fs::remove_dir(&gone).unwrap_err().raw_os_error(), Some(libc::ENOTEMPTY));
        fs::remove_file(format!("{gone}/f")).unwrap();
        fs::remove_dir(&gone).unwrap();

        // Written through a shared mapping, which writeback writes.
        let mapped_path = format!("{dir}/mapped");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&mapped_path)
            .unwrap();
        let data = mapped();
        file.set_len(data.len() as u64).unwrap();
        unsafe {
            let addr = libc::mmap(
                std::ptr::null_mut(),
                data.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            );
            assert_ne!(addr, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            std::ptr::copy_nonoverlapping(data.as_ptr(), addr.cast(), data.len());
            assert_eq!(libc::msync(addr, data.len(), libc::MS_SYNC), 0);
            assert_eq!(libc::munmap(addr, data.len()), 0);
        }
        drop(file);
        assert_eq!(fs::read(&mapped_path).unwrap(), data);

        // A program run from the filesystem: this one, which fails without a test's name.
        let program = format!("{dir}/program");
        fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(&program).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Missing test name argument"),
            "{output:?}"
        );
        fs::remove_file(&program).unwrap();

        xattrs(dir);
        exchange(dir);

        let statfs = nix::sys::statfs::statfs(MNT).unwrap();
        assert!(statfs.blocks_free() < statfs.blocks());
    }

    /// Reads what the host made, and removes its file while it is open, which keeps it readable
    /// through its descriptor.
    fn read_hosts() {
        assert_eq!(names(MNT), ["dir", "hello"]);
        assert_eq!(fs::read(format!("{MNT}/hello")).unwrap(), HELLO);
        let hello = fs::metadata(format!("{MNT}/hello")).unwrap();
        assert!(hello.is_file());
        assert_eq!(hello.permissions().mode(), 0o100644);
        assert_eq!(hello.len(), HELLO.len() as u64);

        let dir = fs::metadata(format!("{MNT}/dir")).unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.permissions().mode(), 0o040750);
        assert_eq!(names(&format!("{MNT}/dir")), ["nested"]);
        assert_eq!(fs::read(format!("{MNT}/dir/nested")).unwrap(), nested());

        let hello = fs::File::open(format!("{MNT}/hello")).unwrap();
        fs::remove_file(format!("{MNT}/hello")).unwrap();
        let mut contents = Vec::new();
        (&hello).read_to_end(&mut contents).unwrap();
        assert_eq!(contents, HELLO);
        drop(hello);
        assert_eq!(names(MNT), ["dir"]);
    }

    /// Extended attributes: set, created, replaced, listed, read, removed, and kept.
    fn xattrs(dir: &str) {
        let path = CString::new(format!("{dir}/renamed")).unwrap();
        let set = |name: &CStr, value: &[u8], flags| unsafe {
            libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), flags)
        };
        let get = |name: &CStr| {
            let mut buf = [0u8; 256];
            let len = unsafe {
                libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
            };
            if len < 0 {
                Err(std::io::Error::last_os_error().raw_os_error().unwrap())
            } else {
                Ok(buf[..len as usize].to_vec())
            }
        };
        assert_eq!(set(c"user.colour", b"blue", 0), 0);
        assert_eq!(get(c"user.colour"), Ok(b"blue".to_vec()));
        // Creating one that exists fails, and replacing one that doesn't.
        assert_eq!(set(c"user.colour", b"red", libc::XATTR_CREATE), -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EEXIST));
        assert_eq!(set(c"user.size", b"large", libc::XATTR_REPLACE), -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENODATA));
        assert_eq!(set(c"user.colour", b"green", libc::XATTR_REPLACE), 0);
        assert_eq!(set(c"user.size", b"large", libc::XATTR_CREATE), 0);
        assert_eq!(set(c"trusted.kept", b"", 0), 0);
        assert_eq!(get(c"user.colour"), Ok(b"green".to_vec()));
        assert_eq!(get(c"trusted.kept"), Ok(Vec::new()));
        assert_eq!(get(c"user.absent"), Err(libc::ENODATA));

        let mut list = [0u8; 256];
        let len = unsafe { libc::listxattr(path.as_ptr(), list.as_mut_ptr().cast(), list.len()) };
        assert!(len >= 0, "listxattr: {}", std::io::Error::last_os_error());
        let mut names: Vec<&[u8]> =
            list[..len as usize].split(|&b| b == 0).filter(|name| !name.is_empty()).collect();
        names.sort();
        assert_eq!(names, [&b"trusted.kept"[..], b"user.colour", b"user.size"]);

        assert_eq!(unsafe { libc::removexattr(path.as_ptr(), c"user.size".as_ptr()) }, 0);
        assert_eq!(get(c"user.size"), Err(libc::ENODATA));
        // Set and removed before the inode is written back.
        assert_eq!(set(c"user.brief", b"gone", 0), 0);
        assert_eq!(unsafe { libc::removexattr(path.as_ptr(), c"user.brief".as_ptr()) }, 0);
        assert_eq!(unsafe { libc::removexattr(path.as_ptr(), c"user.brief".as_ptr()) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENODATA));

        // Written back with the inode, and read in again.
        evict(&format!("{dir}/renamed"));
        assert_eq!(get(c"user.colour"), Ok(b"green".to_vec()));
        assert_eq!(get(c"trusted.kept"), Ok(Vec::new()));
        assert_eq!(get(c"user.size"), Err(libc::ENODATA));
        assert_eq!(get(c"user.brief"), Err(libc::ENODATA));
    }

    /// RENAME_EXCHANGE: a file and a directory swap names, across directories.
    fn exchange(dir: &str) {
        let (file, sub) = (format!("{dir}/swap-file"), format!("{MNT}/swap-dir"));
        fs::write(&file, b"was a file").unwrap();
        fs::create_dir(&sub).unwrap();
        fs::write(format!("{sub}/inside"), b"in the directory").unwrap();
        let (file_c, sub_c) =
            (CString::new(file.as_str()).unwrap(), CString::new(sub.as_str()).unwrap());
        let ret = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                file_c.as_ptr(),
                libc::AT_FDCWD,
                sub_c.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        assert_eq!(ret, 0, "renameat2: {}", std::io::Error::last_os_error());
        assert_eq!(fs::read(&sub).unwrap(), b"was a file");
        assert_eq!(fs::read(format!("{file}/inside")).unwrap(), b"in the directory");
        // The directories' links follow the directory that moved between them.
        assert_eq!(fs::metadata(dir).unwrap().nlink(), 3);
        fs::remove_file(format!("{file}/inside")).unwrap();
        fs::remove_dir(&file).unwrap();
        fs::remove_file(&sub).unwrap();
    }

    /// What `write` made, after mounting again, beside what the host made, if it did.
    fn check_written(host_formats: bool) {
        let mut expected = vec!["guest-dir", "link", "symlink"];
        if host_formats {
            expected.insert(0, "dir");
        }
        assert_eq!(names(MNT), expected);
        assert_eq!(names(&format!("{MNT}/guest-dir")), ["mapped", "renamed"]);
        assert_eq!(fs::read(format!("{MNT}/guest-dir/mapped")).unwrap(), mapped());
        let renamed = fs::metadata(format!("{MNT}/guest-dir/renamed")).unwrap();
        assert_eq!(renamed.permissions().mode(), 0o100640);
        assert_eq!(renamed.nlink(), 2);
        assert_eq!(fs::read(format!("{MNT}/symlink")).unwrap(), written());
        assert_eq!(
            fs::read_link(format!("{MNT}/symlink")).unwrap(),
            std::path::Path::new("guest-dir/renamed")
        );
        assert_eq!(fs::read(format!("{MNT}/link")).unwrap(), written());
        let path = CString::new(format!("{MNT}/guest-dir/renamed")).unwrap();
        let mut buf = [0u8; 16];
        let len = unsafe {
            libc::getxattr(path.as_ptr(), c"user.colour".as_ptr(), buf.as_mut_ptr().cast(), 16)
        };
        assert_eq!(&buf[..len.max(0) as usize], b"green");
    }

    /// Writes a file and fsyncs it and its directory, and writes more without, and leaves the
    /// filesystem mounted, for the VM to stop with.
    /// Writes and fsyncs file after file, and the directory, reporting each once both fsyncs
    /// return, until the VM stops.
    /// fxdax: the faults test's (TASK-44.10): its writers' allocations failed at random for a few
    /// seconds, as the debug kernel's fault injection does them for tasks that ask, through
    /// debugfs; each writer reports each file whose fsync returned, and in the end how many of its
    /// calls failed. Not those of Fxfs's Rust's runtime, rt.c's (fxfs_rt_*), its allocator's and
    /// its executor's, which can't fail: a failure there panicked the kernel, or wrote through
    /// NULL (TASK-60.1, GCE r260, r262).
    fn faults() {
        const DEBUGFS: &str = "/sys/kernel/debug";
        const ATTRS: [&str; 2] = ["failslab", "fail_page_alloc"];
        let debugfs = CString::new(DEBUGFS).unwrap();
        // SAFETY: Plain system call on NUL-terminated strings.
        let mounted = unsafe {
            libc::mount(
                c"debugfs".as_ptr(),
                debugfs.as_ptr(),
                c"debugfs".as_ptr(),
                0,
                std::ptr::null(),
            )
        };
        let err = std::io::Error::last_os_error();
        assert!(mounted == 0 || err.raw_os_error() == Some(libc::EBUSY), "debugfs: {err}");
        let set = |attr: &str, name: &str, value: &str| {
            let path = format!("{DEBUGFS}/{attr}/{name}");
            fs::write(&path, value).unwrap_or_else(|err| {
                panic!("{path}: {err} (the debug kernel injects faults: FXDAX_GUEST_KERNEL_BUILD=debug)")
            });
        };
        let (rust_start, rust_end) = symbols_range("fxfs_rt_alloc", "fxfs_rt_");
        eprintln!("faults: none forced from {rust_start:#x} to {rust_end:#x}, Fxfs's runtime");
        for attr in ATTRS {
            set(attr, "reject-start", &format!("{rust_start:#x}"));
            set(attr, "reject-end", &format!("{rust_end:#x}"));
            set(attr, "stacktrace-depth", "32");
            set(attr, "task-filter", "Y");
            set(attr, "interval", "1");
            set(attr, "times", "-1");
            // A line in the kernel's log for each failure forced, which proves some were.
            set(attr, "verbose", "1");
            set(attr, "ignore-gfp-wait", "N");
            set(attr, "probability", "1");
        }
        // Not the pages the writers' own memory faults in (__GFP_HIGHMEM): one failing killed the
        // agent, init, with SIGSEGV (GCE r267).
        set("fail_page_alloc", "ignore-gfp-highmem", "Y");
        set("fail_page_alloc", "min-order", "0");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (1..=2u8)
            .map(|writer| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    // Its allocations, alone, fail.
                    fs::write("/proc/thread-self/make-it-fail", "1").unwrap();
                    let mut failed = 0u64;
                    for index in 0.. {
                        if stop.load(std::sync::atomic::Ordering::Acquire) {
                            break;
                        }
                        let attempt = || -> std::io::Result<()> {
                            let name = format!("{MNT}/synced-{writer}-{index}");
                            let mut file = fs::File::create(name)?;
                            file.write_all(&durable(writer, index))?;
                            file.sync_all()?;
                            fs::File::open(MNT)?.sync_all()
                        };
                        match attempt() {
                            Ok(()) => report(&format!("synced {writer} {index}")),
                            Err(_) => failed += 1,
                        }
                    }
                    fs::write("/proc/thread-self/make-it-fail", "0").unwrap();
                    failed
                })
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_secs(3));
        stop.store(true, std::sync::atomic::Ordering::Release);
        let failed: u64 = writers.into_iter().map(|writer| writer.join().unwrap()).sum();
        for attr in ATTRS {
            set(attr, "probability", "0");
        }
        report(&format!("failed calls {failed}"));
        report(&format!("forced failures {}", forced_failures()));
    }

    /// fxdax: how many failures the kernel's fault injection says it forced, in its log
    /// (/dev/kmsg), each a line while its rate allows.
    fn forced_failures() -> usize {
        use std::os::unix::fs::OpenOptionsExt;
        let mut kmsg = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/kmsg")
            .unwrap();
        let (mut forced, mut record) = (0, vec![0; 8192]);
        loop {
            match kmsg.read(&mut record) {
                Ok(0) => return forced,
                Ok(n) => {
                    let line = String::from_utf8_lossy(&record[..n]);
                    forced += usize::from(line.contains("FAULT_INJECTION: forcing a failure"));
                }
                // Records overwritten while reading: read on from the oldest kept.
                Err(err) if err.raw_os_error() == Some(libc::EPIPE) => continue,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return forced,
                Err(err) => panic!("/dev/kmsg: {err}"),
            }
        }
    }

    /// fxdax: the addresses of the kernel's functions around `name` whose names begin with
    /// `prefix`, one after another, one object's text, from the first's start to the end of the
    /// last, the next symbol's start, as /proc/kallsyms has them: not those of the prefix elsewhere,
    /// as an __init one is, which took most of the kernel in between, and the injection forced no
    /// failure (GCE r266).
    fn symbols_range(name: &str, prefix: &str) -> (u64, u64) {
        let symbols = fs::read_to_string("/proc/kallsyms").unwrap();
        let mut addresses: Vec<(u64, &str)> = (symbols.lines())
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let address = u64::from_str_radix(fields.next()?, 16).ok()?;
                Some((address, fields.nth(1)?))
            })
            .collect();
        addresses.sort_unstable();
        let at = addresses.iter().position(|&(_, symbol)| symbol == name).expect(name);
        let named = |at: &usize| addresses[*at].1.starts_with(prefix);
        let first = (0..=at).rev().take_while(named).last().unwrap();
        let last = (at..addresses.len()).take_while(named).last().unwrap();
        let start = addresses[first].0;
        assert_ne!(start, 0, "/proc/kallsyms hides addresses");
        let end = addresses[last..].iter().find(|&&(address, _)| address > addresses[last].0);
        (start, end.unwrap().0)
    }

    /// Whether the crash test's writers wait, as its guest stores through a mapping (TASK-79.11).
    static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn fsync_files(writer: u8) -> ! {
        for index in 0.. {
            while PAUSED.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let mut file = fs::File::create(format!("{MNT}/synced-{writer}-{index}")).unwrap();
            file.write_all(&durable(writer, index)).unwrap();
            file.sync_all().unwrap();
            fs::File::open(MNT).unwrap().sync_all().unwrap();
            report(&format!("synced {writer} {index}"));
            // Unsynced changes beside them, which the filesystem may hold or not.
            fs::write(format!("{MNT}/pending-{writer}-{index}"), pattern(5000, writer)).unwrap();
            if index % 3 == 0 {
                let _ = fs::remove_file(format!("{MNT}/pending-{writer}-{}", index / 2));
            }
        }
        unreachable!()
    }

    /// Reports `line` on its own write, as the VM may stop between writes: unless it is about to,
    /// when the caller waits here for it ([`quiet`]), its write, if this is said as it begins, not
    /// begun.
    fn report(line: &str) {
        while quiet().load(std::sync::atomic::Ordering::Acquire) {
            std::thread::park();
        }
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}").unwrap();
        stdout.flush().unwrap();
    }

    /// fxdax: makes RESIZED, of resize 0's size, synced, then truncates or extends it again and
    /// again, to each resize's size, writing the pattern where it extends, and syncing it,
    /// reporting each resize as it begins, and as its sync returns, until the VM stops
    /// (TASK-119.6).
    fn resize() -> ! {
        use std::os::unix::fs::FileExt;
        let path = format!("{MNT}/{RESIZED}");
        let file = fs::OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();
        // As much as RESIZED holds at most.
        let full: Vec<u8> = (0..16 * RESIZED_STEP).map(resized_byte).collect();
        let first = resized_size(0);
        file.write_all_at(&full[..first], 0).unwrap();
        file.sync_all().unwrap();
        fs::File::open(MNT).unwrap().sync_all().unwrap();
        report(&format!("resized 0 {first}"));
        let mut size = first;
        for generation in 1u32.. {
            while PAUSED.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let next = resized_size(generation);
            report(&format!("resizing {generation} {next}"));
            file.set_len(next as u64).unwrap();
            if next > size {
                file.write_all_at(&full[size..next], size as u64).unwrap();
            }
            file.sync_all().unwrap();
            size = next;
            report(&format!("resized {generation} {next}"));
        }
        unreachable!()
    }

    /// fxdax: makes REWRITTEN[`file`], each page's generation 0 synced, then writes its pages
    /// again and again, each a generation more, through write(2) and fsync, or (`file` 1) stores
    /// through a mapping and msync of the page, reporting each write as it begins, and as its sync
    /// returns, until the VM stops (TASK-119.6). The pages of a crash's store hold what one of
    /// their writes since the last synced wrote.
    fn rewrite(file: usize) -> ! {
        let path = format!("{MNT}/{}", REWRITTEN[file]);
        let mut handle = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        for page in 0..REWRITTEN_PAGES {
            handle.write_all(&rewritten(page, 0)).unwrap();
        }
        handle.sync_all().unwrap();
        fs::File::open(MNT).unwrap().sync_all().unwrap();
        for page in 0..REWRITTEN_PAGES {
            report(&format!("rewrote {file} {page} 0"));
        }
        let size = REWRITTEN_PAGES * 4096;
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                handle.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        for generation in 1u32.. {
            while PAUSED.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            // Pages in an order that varies, from the generation.
            let page = (generation as usize * 7) % REWRITTEN_PAGES;
            let data = rewritten(page, generation);
            report(&format!("rewriting {file} {page} {generation}"));
            if file == 0 {
                std::os::unix::fs::FileExt::write_all_at(&handle, &data, (page * 4096) as u64)
                    .unwrap();
                handle.sync_all().unwrap();
            } else {
                let at = unsafe { map.cast::<u8>().add(page * 4096) };
                unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), at, 4096) };
                let synced = unsafe { libc::msync(at.cast(), 4096, libc::MS_SYNC) };
                assert_eq!(synced, 0, "msync: {}", std::io::Error::last_os_error());
            }
            report(&format!("rewrote {file} {page} {generation}"));
        }
        unreachable!()
    }

    /// Stops the VM mid-workload: with two writers fsyncing, and without syncing (reboot(2)
    /// doesn't); with an evacuation held at `step` of the DAX protocol, if asked; or, if `killed`,
    /// has its host kill its process there ([`KILL_ME`]).
    fn crash(dax: bool, step: Option<u32>, killed: bool) -> ! {
        assert_eq!(names(MNT), Vec::<String>::new());
        // Files map their data from the window, if there is one: small ones too.
        let (probe, small) = (format!("{MNT}/probe"), format!("{MNT}/small"));
        fs::write(&probe, pattern(2 << 20, 1)).unwrap();
        fs::write(&small, b"small").unwrap();
        evict(&probe);
        evict(&small);
        assert_eq!(is_dax(&probe), dax, "the DAX attribute");
        assert_eq!(is_dax(&small), dax, "a small file's DAX attribute");
        fs::remove_file(&probe).unwrap();
        fs::remove_file(&small).unwrap();
        // An unnamed file, made durable, open as the VM stops: which the store's next mount frees.
        let dir = CString::new(MNT).unwrap();
        let fd = unsafe { libc::open(dir.as_ptr(), libc::O_TMPFILE | libc::O_RDWR, 0o600) };
        assert!(fd >= 0, "O_TMPFILE: {}", std::io::Error::last_os_error());
        let mut unnamed = unsafe { <fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        unnamed.write_all(&pattern(UNNAMED, 9)).unwrap();
        unnamed.sync_all().unwrap();
        std::mem::forget(unnamed);
        let mapped = format!("{MNT}/{MAPPED_SYNCED}");
        if dax {
            let mut file = fs::File::create(&mapped).unwrap();
            file.write_all(&mapped_synced()).unwrap();
            file.sync_all().unwrap();
            fs::File::open(MNT).unwrap().sync_all().unwrap();
        }
        // fxdax: unless its process is to be killed, whose host the guest asks to (TASK-119.6).
        let stopper = (!killed).then(|| Stopper::fork(step));
        std::thread::spawn(|| fsync_files(1));
        std::thread::spawn(|| fsync_files(2));
        std::thread::spawn(|| rewrite(0));
        std::thread::spawn(|| rewrite(1));
        std::thread::spawn(resize);
        // fxdax: with DAX, a store through a mapping as the VM stops, before the writers fill the
        // store, which they do in 2 seconds (TASK-79.11). The VM stops at a time the host picks,
        // FXDAX_CRASH_MS, from when the writers begin (TASK-119.6).
        let crash_ms: u64 = std::env::var("FXDAX_CRASH_MS").unwrap().parse().unwrap();
        if dax {
            std::thread::sleep(std::time::Duration::from_secs(1));
            store_mapped(&mapped);
            PAUSED.store(false, std::sync::atomic::Ordering::Release);
            std::thread::sleep(std::time::Duration::from_millis(crash_ms.saturating_sub(1000)));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(crash_ms));
        }
        match stopper {
            Some(stopper) => stopper.stop(),
            None => {
                report(KILL_ME);
                loop {
                    std::thread::park();
                }
            }
        }
    }

    /// fxdax: what stops the crash test's VM, a process of its own, forked before the writers
    /// begin: a writer's fault on a file whose evacuation it holds waits holding its process's
    /// memory map lock, which the agent's threads then waited for, the stop's among them, which
    /// said nothing and stopped no VM (GCE r221); nor does it say anything through the agent's
    /// forwarders, in that process too (TASK-119.16).
    struct Stopper {
        /// The pipe whose write says to stop.
        go: fs::File,
    }

    /// fxdax: whether the VM is about to stop, in memory the process that stops it shares, which
    /// sets it before it drains the console: the writers report nothing more, and begin no write,
    /// so that the console drains, as writers printing all the while kept it from, and the VM ran
    /// on (TASK-119.6, GCE r247). Made before that process is forked ([`Stopper::fork`]).
    fn quiet() -> &'static std::sync::atomic::AtomicBool {
        static QUIET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let page = *QUIET.get_or_init(|| {
            // SAFETY: A new anonymous mapping, shared with the processes forked after.
            let page = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    4096,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(page, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            page as usize
        });
        // SAFETY: Zeroed memory, mapped for good, is a false AtomicBool.
        unsafe { &*(page as *const std::sync::atomic::AtomicBool) }
    }

    impl Stopper {
        /// Forks the process that stops the VM, once told to, with an evacuation held at `step` of
        /// the DAX protocol first, if asked.
        fn fork(step: Option<u32>) -> Self {
            quiet();
            let mut fds = [0; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
            // SAFETY: The pipe's ends, which nothing else owns.
            let (mut wait, go) = unsafe {
                use std::os::fd::FromRawFd;
                (fs::File::from_raw_fd(fds[0]), fs::File::from_raw_fd(fds[1]))
            };
            // SAFETY: The agent's threads, its forwarders, hold no lock the child takes.
            match unsafe { libc::fork() } {
                -1 => panic!("fork: {}", std::io::Error::last_os_error()),
                0 => {
                    drop(go);
                    wait.read_exact(&mut [0]).unwrap();
                    if let Some(step) = step {
                        hold_evacuation(step);
                    }
                    quiet().store(true, std::sync::atomic::Ordering::Release);
                    // A reset, which ends the VM on every architecture: x86's has no power-off.
                    crate::guest_init::crash()
                }
                _ => Self { go },
            }
        }

        /// Has the VM stop.
        fn stop(mut self) -> ! {
            self.go.write_all(&[1]).unwrap();
            let mut status = 0;
            unsafe { libc::wait(&mut status) };
            panic!("the process stopping the VM ended, status {status:#x}");
        }
    }

    /// fxdax: holds the next evacuation of a file at `step` of the DAX protocol (fxfs.dax_pause's
    /// bit, a pause point), which the writers' fsyncs start as they seal, and says so on the
    /// console once one is held, for the VM to stop with it so (TASK-119.16).
    fn hold_evacuation(step: u32) {
        const PARAMETERS: &str = "/sys/module/fxfs/parameters";
        fs::write(format!("{PARAMETERS}/dax_pause"), (1u64 << step).to_string()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let paused = fs::read_to_string(format!("{PARAMETERS}/dax_paused")).unwrap();
            if paused.trim().parse::<u64>().unwrap() & (1 << step) != 0 {
                crate::guest_init::say(&format!("held at step {step}"));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// fxdax: stores a byte to the page at `path`, synced, through a mapping, as the VM is about
    /// to stop, with the writers waiting: the fault moves the page's block, and a seal comes right
    /// after (the fxfs.dax_fault_seals_moved fault injection), with none after it (TASK-79.11).
    fn store_mapped(path: &str) {
        const SEALS_MOVED: &str = "/sys/module/fxfs/parameters/dax_fault_seals_moved";
        PAUSED.store(true, std::sync::atomic::Ordering::Release);
        // A writer's fsync in progress, done.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let file = fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        // A load first maps the file's range, which the writers' files may have taken: the store
        // then finds the page mapped, read-only, as a fault holding it does.
        let loaded = unsafe { map.cast::<u8>().add(MAPPED_STORE).read_volatile() };
        assert_eq!(loaded, mapped_synced()[MAPPED_STORE], "the mapping maps other data");
        fs::write(SEALS_MOVED, "1").unwrap();
        unsafe { map.cast::<u8>().add(MAPPED_STORE).write_volatile(0xee) };
        assert_eq!(fs::read_to_string(SEALS_MOVED).unwrap().trim(), "0", "no seal after a move");
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "stored mapped").unwrap();
        stdout.flush().unwrap();
    }

    /// Whether the kernel accesses `path` through the DAX window: STATX_ATTR_DAX.
    fn is_dax(path: &str) -> bool {
        const STATX_ATTR_DAX: u64 = 0x200000;
        // struct statx: stx_attributes at byte 8, stx_attributes_mask at 56.
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
        assert_ne!(mask & STATX_ATTR_DAX, 0, "the kernel does not report DAX");
        attributes & STATX_ATTR_DAX != 0
    }

    /// The device's count of the requests of `kind` (as `device_requests` names them).
    fn requests(kind: &str) -> u64 {
        device_requests()
            .lines()
            .find_map(|line| line.strip_prefix(kind)?.strip_prefix(' ')?.parse().ok())
            .unwrap_or_else(|| panic!("no count of {kind} requests"))
    }

    /// A file's data is written through the window, and its window ranges outlive its inode:
    /// evicted and read in again, it takes no MAP and no READ.
    fn read_maps() {
        let path = format!("{MNT}/mapped-read");
        let data = pattern((3 << 20) + 5, 0x3c);
        fs::write(&path, &data).unwrap();
        assert!(is_dax(&path), "{path} is not DAX");
        evict(&path);
        // The spare a seal leaves mapped, before counting: a sync waits for it.
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        let (reads, maps) = (requests("read"), requests("map"));
        assert_eq!(fs::read(&path).unwrap(), data);
        assert_eq!(requests("read"), reads, "reading the file read from the device");
        assert_eq!(requests("map"), maps, "reading the file mapped its extents");
        fs::remove_file(&path).unwrap();
    }

    /// Small files are DAX, and share an extent, which the window maps once for them all: as the
    /// files' blocks are handed out, further each time, 16 pages first, then doubling, a step
    /// ahead of them once they reach the second half of what is mapped. The first 16 pages are
    /// mapped as a seal ends, before the writes (the spare), and the next extent's once the next
    /// seal does, so 32 files of a block take 3 MAPs (to 64 pages, and the next spare), and no
    /// READ. Evicted and read in again, they take neither.
    fn small_files_share() {
        let dir = format!("{MNT}/small");
        fs::create_dir(&dir).unwrap();
        let files: Vec<(String, Vec<u8>)> =
            (0..32).map(|i| (format!("{dir}/file-{i}"), pattern(100 + i, i as u8))).collect();
        // Sealed, the next small write takes a new extent, mapped ahead of it: a sync waits for
        // the spare the seal before it left to be mapped.
        fs::write(format!("{dir}/seed"), b"seed").unwrap();
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        let (reads, maps) = (requests("read"), requests("map"));
        for (path, data) in &files {
            fs::write(path, data).unwrap();
        }
        // A sync waits for the region to grow as far ahead of the writes as it does.
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        assert_eq!(requests("map"), maps + 3, "the small files' MAPs");
        assert_eq!(requests("read"), reads, "writing small files read from the device");
        assert!(is_dax(&files[0].0), "a small file is not DAX");
        for (path, _) in &files {
            evict(path);
        }
        let (reads, maps) = (requests("read"), requests("map"));
        for (path, data) in &files {
            assert_eq!(&fs::read(path).unwrap(), data, "{path}");
        }
        assert_eq!(requests("map"), maps, "reading small files again mapped them");
        assert_eq!(requests("read"), reads, "reading small files again read from the device");
        for (path, _) in &files {
            fs::remove_file(path).unwrap();
        }
        fs::remove_file(format!("{dir}/seed")).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    /// How many requests each of the device's request queues has taken.
    fn queue_requests() -> Vec<u64> {
        let devices: Vec<_> = fs::read_dir("/sys/bus/virtio/drivers/virtio_extent")
            .unwrap()
            .map(|entry| entry.unwrap().path().join("queues"))
            .filter(|path| path.exists())
            .collect();
        assert_eq!(devices.len(), 1, "{devices:?}");
        let counts = fs::read_to_string(&devices[0]).unwrap();
        counts.lines().map(|count| count.parse().unwrap()).collect()
    }

    /// The driver uses a request queue per CPU, of the device's four (fxdax TASK-26.18.5): a
    /// writer on each CPU fsyncs a file, through its CPU's queue.
    fn queue_per_cpu() {
        let cpus = std::thread::available_parallelism().unwrap().get();
        let before = queue_requests();
        assert_eq!(before.len(), cpus.min(4), "the driver's queues");
        let writers: Vec<_> = (0..before.len())
            .map(|cpu| {
                std::thread::spawn(move || {
                    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
                    unsafe { libc::CPU_SET(cpu, &mut set) };
                    let pinned =
                        unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set) };
                    assert_eq!(pinned, 0, "{}", std::io::Error::last_os_error());
                    let path = format!("{MNT}/queue-{cpu}");
                    let mut file = fs::File::create(&path).unwrap();
                    file.write_all(&pattern(64 << 10, cpu as u8)).unwrap();
                    file.sync_all().unwrap();
                    drop(file);
                    fs::remove_file(&path).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let after = queue_requests();
        for (queue, (before, after)) in before.iter().zip(&after).enumerate() {
            assert!(after > before, "queue {queue} took no request: {after} of {before}");
        }
    }

    /// The extent-store device's count of the requests it has taken, by type.
    fn device_requests() -> String {
        let devices: Vec<_> = fs::read_dir("/sys/bus/virtio/drivers/virtio_extent")
            .unwrap()
            .map(|entry| entry.unwrap().path().join("requests"))
            .filter(|path| path.exists())
            .collect();
        assert_eq!(devices.len(), 1, "{devices:?}");
        fs::read_to_string(&devices[0]).unwrap()
    }

    /// Lookups, stats, creates, renames and unlinks of metadata in memory take no device requests.
    fn metadata_cached() {
        let dir = format!("{MNT}/cached");
        fs::create_dir(&dir).unwrap();
        // Read in what the operations will touch.
        let _ = names(MNT);
        let _ = fs::metadata(format!("{MNT}/guest-dir/renamed")).unwrap();
        // What earlier writes left to do in the background, done: a sync waits for the frees,
        // and the next for the spare mapped once the first's seal ended.
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        let before = device_requests();
        for i in 0..20 {
            let path = format!("{dir}/file-{i}");
            fs::File::create(&path).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().len(), 0);
            fs::rename(&path, format!("{dir}/moved-{i}")).unwrap();
        }
        assert_eq!(names(&dir).len(), 20);
        let _ = fs::metadata(format!("{MNT}/guest-dir/renamed")).unwrap();
        assert_eq!(
            fs::metadata(format!("{MNT}/guest-dir/absent")).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        for i in 0..20 {
            fs::remove_file(format!("{dir}/moved-{i}")).unwrap();
        }
        assert_eq!(device_requests(), before);
        fs::remove_dir(&dir).unwrap();
    }

    /// The names in `dir`, sorted.
    fn names(dir: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    impl Test for TestFxfsGuest {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            // A tag no device has.
            assert_eq!(mount("missing").unwrap_err().raw_os_error(), Some(libc::ENOENT));
            mount("extents").unwrap();
            if self.mode == Mode::Crash {
                crash(self.dax, self.crash_step, self.killed);
            }
            if self.mode == Mode::Faults {
                faults();
                return;
            }
            let host_formats = self.mode == Mode::HostFormats;
            if host_formats {
                read_hosts();
            } else {
                assert_eq!(names(MNT), Vec::<String>::new());
            }
            assert_eq!(
                fs::metadata(format!("{MNT}/missing")).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );

            write(&format!("{MNT}/guest-dir"));
            // Read in again, a file maps its data from the window, if there is one.
            let renamed = format!("{MNT}/guest-dir/renamed");
            evict(&renamed);
            assert_eq!(is_dax(&renamed), self.dax, "the DAX attribute");
            metadata_cached();
            queue_per_cpu();
            if self.dax {
                read_maps();
                small_files_share();
            }

            // The store mounted again, elsewhere, is the same filesystem.
            const MNT2: &str = "/mnt/fxfs2";
            fs::create_dir_all(MNT2).unwrap();
            mount_at("extents", MNT2).unwrap();
            assert_eq!(names(MNT2), names(MNT));
            fs::write(format!("{MNT2}/shared"), b"through the second mount").unwrap();
            assert_eq!(fs::read(format!("{MNT}/shared")).unwrap(), b"through the second mount");
            fs::remove_file(format!("{MNT}/shared")).unwrap();
            unmount_at(MNT2);

            unmount();
            mount("extents").unwrap();
            check_written(host_formats);
            unmount();

            println!("OK");
        }
    }
}
