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
    /// fxdax: whether a crash test's VM process is killed (SIGKILL), rather than its guest reset
    /// (TASK-119.6).
    killed: bool,
    /// fxdax: whether the extent daemon is killed in the VM process's place, which what it holds
    /// goes with, of the commits it has yet to make durable (doc-12, TASK-67).
    #[cfg_attr(feature = "guest", allow(dead_code))]
    daemon_killed: bool,
    /// fxdax: whether a crash test's guest stops its VM by touching a part of the window it was
    /// never given, as one that keeps to no protocol may (TASK-77).
    stray: bool,
}

impl TestFxfsGuest {
    /// fxfs-guest.
    pub fn host_formats() -> Self {
        Self { mode: Mode::HostFormats, dax: false, killed: false, daemon_killed: false, stray: false }
    }

    /// fxfs-guest-format.
    pub fn guest_formats() -> Self {
        Self { mode: Mode::GuestFormats, dax: false, killed: false, daemon_killed: false, stray: false }
    }

    /// fxfs-guest-crash.
    pub fn crash() -> Self {
        Self { mode: Mode::Crash, dax: false, killed: false, daemon_killed: false, stray: false }
    }

    /// fxdax: fxfs-guest-crash-dax-killed: the crash test with DAX, its VM process killed
    /// (SIGKILL) where its guest would reset it, as a host's crash would end it, the extent
    /// daemon serving the store on, its requests in flight cut off (TASK-119.6).
    pub fn crash_killed() -> Self {
        Self { killed: true, ..Self::crash().with_dax() }
    }

    /// fxdax: fxfs-guest-crash-dax-daemon-killed: the crash test with DAX, on a daemon of its own,
    /// killed where its guest would reset its VM: of what fsyncs were told, a prefix stays.
    pub fn crash_daemon_killed() -> Self {
        Self { daemon_killed: true, ..Self::crash_killed() }
    }

    /// fxdax: fxfs-guest-crash-stray-dax: the crash test with DAX, its guest touching the window
    /// where nothing is mapped where it would reset its VM: a load or a store, within a page or
    /// across two, which FXDAX_CRASH_MS says. That VM stops, and nothing else does: its store
    /// holds what fsyncs were told, and the daemon serves on (TASK-77).
    pub fn crash_stray() -> Self {
        Self { stray: true, ..Self::crash_killed() }
    }

    /// fxdax: fxfs-guest-faults-dax: two writers create, write and fsync files, as the crash
    /// test's do, while the guest's kernel fails their allocations at random, slabs' and pages'
    /// (failslab, fail_page_alloc, its debug build's: FXDAX_GUEST_KERNEL_BUILD=debug), their
    /// calls failing as they may; the filesystem complains of nothing, and the store holds every
    /// file whose fsync returned (TASK-44.10).
    pub fn faults() -> Self {
        Self { mode: Mode::Faults, dax: true, killed: false, daemon_killed: false, stray: false }
    }

    /// The same test with a DAX window.
    pub fn with_dax(self) -> Self {
        Self { dax: true, ..self }
    }
}


/// fxdax: the window of a test that strays out of what it was given, and how much of its start the
/// test's writers reach: its first half is files' ranges, and regions fill the second from its
/// start, of which they use tens of mebibytes in their seconds (GCE r350 mapped 270 MiB in).
#[cfg_attr(feature = "guest", allow(dead_code))]
const STRAY_WINDOW: u64 = 1 << 29;
#[cfg_attr(feature = "host", allow(dead_code))]
const STRAY_WITHIN: u64 = STRAY_WINDOW - (64 << 20);

/// The contents of the crash test's file `index`, of `writer`: of a size and pattern its name
/// gives, which the host checks.
fn durable(writer: u8, index: usize) -> Vec<u8> {
    let mut bytes = vec![0; durable_len(index)];
    durable_into(writer, index, &mut bytes);
    bytes
}

/// The length of file `index`'s bytes, `durable`'s.
fn durable_len(index: usize) -> usize {
    4096 * (index % 37) + 13 * index + 1
}

/// `durable`'s bytes in `bytes`, as long as they are: for a writer whose allocations may fail.
fn durable_into(writer: u8, index: usize, bytes: &mut [u8]) {
    let seed = writer.wrapping_mul(31) ^ index as u8;
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (i % 251) as u8 ^ (i / 4096) as u8 ^ seed;
    }
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

/// fxdax: the crash test's directory with a default ACL, durable, and what it created inherits it:
/// a file and a directory of these names.
const INHERITS: &str = "inherits";
const INHERITS_FILE: &str = "file";
const INHERITS_DIR: &str = "dir";

/// The default ACL of [`INHERITS`]: a user's rw- beside the owners', masked to rwx.
fn default_acl() -> Vec<u8> {
    inherited_acl(7, 7, 5)
}

/// A POSIX ACL as [`INHERITS`]'s entries make it: `owner`'s and `mask`'s permissions, and others'.
fn inherited_acl(owner: u16, mask: u16, other: u16) -> Vec<u8> {
    const UNDEFINED: u32 = u32::MAX;
    const USER: u32 = 1000;
    crate::posix_acl(&[
        (0x01, owner, UNDEFINED),
        (0x02, 6, USER),
        (0x04, 5, UNDEFINED),
        (0x10, mask, UNDEFINED),
        (0x20, other, UNDEFINED),
    ])
}

/// fxdax: the line the killed crash test's guest says where it would reset its VM, at which the
/// host kills the VM's process (TASK-119.6).
const KILL_ME: &str = "kill me";

/// fxdax: what the crash test's guest says before [`KILL_ME`], and the milliseconds since its
/// writers began, when it asked to be killed (TASK-165.9).
const KILLING: &str = "killing at ";

/// fxdax: what the guest says before its stray access ([`TestFxfsGuest::crash_stray`]).
const STRAY: &str = "stray ";

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

    use crate::common::{fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::{Filesystem, Owner, Storage};

    /// The store's capacity: room for the crash test's unnamed file too.
    const CAPACITY: u64 = 128 << 20;

    /// fxdax: doc-12's crash bound: what a daemon crash may lose, the commits ordered, and
    /// answered, within it before. A file whose fsync returned that soon before the kill may be
    /// found short or empty, as one the daemon lost, its creation durable, its data not (TASK-221:
    /// set from the case's longest lags, main's 60-81 ms, TASK-165.9's up to 288 ms).
    const CRASH_LOSS_BOUND_MS: u128 = 500;

    /// The files whose fsync the guest says returned, its writer and index, as `output` says.
    fn synced_files(output: &str) -> Vec<(u8, usize)> {
        synced(output).map(|(file, _)| file).collect()
    }

    /// The files whose fsync the guest says returned, and when, in milliseconds since its writers
    /// began, if it says ([`since_writers_began`]).
    fn synced(output: &str) -> impl Iterator<Item = ((u8, usize), Option<u128>)> + '_ {
        (output.lines())
            .filter_map(|line| line.strip_prefix("synced "))
            // A line the VM stopped in the middle of: of a file a line before names too.
            .filter_map(|file| {
                let mut words = file.split(' ');
                let (writer, index) = (words.next()?.parse().ok()?, words.next()?.parse().ok()?);
                // fxdax: and when, unless cut short (TASK-165.9).
                let at = match (words.next(), words.next()) {
                    (Some("at"), Some(at)) => at.strip_suffix("ms").and_then(|at| at.parse().ok()),
                    _ => None,
                };
                Some(((writer, index), at))
            })
    }

    /// fxdax: the files whose fsync returned within [`CRASH_LOSS_BOUND_MS`] before the guest asked
    /// to be killed, as `output` says: those a daemon killed may lose, short or empty.
    fn synced_at_the_kill(output: &str) -> std::collections::BTreeSet<(u8, usize)> {
        let killing = output.lines().find_map(|line| line.strip_prefix(KILLING));
        let killing = killing.and_then(|at| at.strip_suffix("ms"));
        let Some(killing) = killing.and_then(|at| at.parse::<u128>().ok()) else {
            return std::collections::BTreeSet::new();
        };
        let recent = |at: u128| at + CRASH_LOSS_BOUND_MS >= killing;
        synced(output).filter(|(_, at)| at.is_some_and(recent)).map(|(file, _)| file).collect()
    }

    /// fxdax: of the files each writer's fsync returned for, in its order, those that stayed, with
    /// what was written, followed by those lost: a commit stays only with those before it (doc-12:
    /// up to 50 ms of them go with a daemon). One lost may be there, short or empty, its creation
    /// durable, its data not, if its fsync returned just before the kill (`recent`, TASK-165.9).
    /// Returns how many went.
    fn assert_kept_prefix(
        fs: &Filesystem,
        synced: &[(u8, usize)],
        recent: &std::collections::BTreeSet<(u8, usize)>,
    ) -> usize {
        let stayed = |writer, index| kept(fs, writer, index, recent.contains(&(writer, index)));
        let mut lost = 0;
        for &(writer, index) in synced {
            let lost_before = |&(other, before): &(u8, usize)| {
                other == writer && before < index && !stayed(other, before)
            };
            let earlier_lost = synced.iter().any(lost_before);
            if stayed(writer, index) {
                assert!(!earlier_lost, "writer {writer}'s file {index} stayed, an earlier one lost");
            } else {
                lost += 1;
            }
        }
        lost
    }

    /// Whether writer `writer`'s file `index` is there, as written: not if it isn't there, nor, if
    /// `recent` ([`assert_kept_prefix`]), if it holds what was written cut short.
    fn kept(fs: &Filesystem, writer: u8, index: usize, recent: bool) -> bool {
        let name = format!("synced-{writer}-{index}");
        let Ok((file, attr)) = block_on(fs.root().lookup(name.as_bytes())) else {
            return false;
        };
        let mut contents = vec![0; attr.size as usize];
        let read = block_on(file.read(0, &mut contents)).unwrap();
        contents.truncate(read);
        let written = durable(writer, index);
        if recent && contents.len() < written.len() && written.starts_with(&contents) {
            return false;
        }
        assert!(contents == written, "{name} holds other data");
        true
    }

    /// A line the VM stopped in the middle of, or the kernel's restart message after the last,
    /// names no file (TASK-79.60).
    #[test]
    fn lines_cut_short_name_no_synced_file() {
        let output = "synced 1 2\nsynced 3\nsynced 3 x\nreboot: Restarting system\nsynced 4 5";
        assert_eq!(synced_files(output), vec![(1, 2), (4, 5)]);
    }

    /// fxdax: the files whose fsync returned within [`CRASH_LOSS_BOUND_MS`] before the guest asked
    /// to be killed, as it says, are those a daemon killed may leave short; none if it says not
    /// when, or a line cut short says not when (TASK-165.9).
    #[test]
    fn files_synced_just_before_the_kill_are_recent() {
        let output = "synced 1 7 at 500ms\nsynced 2 3 at 750ms\nsynced 1 8 at 801ms\n\
                      synced 2 4 at 125\nkilling at 1300ms\nkill me";
        assert_eq!(synced_files(output), vec![(1, 7), (2, 3), (1, 8), (2, 4)]);
        assert_eq!(synced_at_the_kill(output), [(1, 8)].into());
        assert_eq!(synced_at_the_kill("synced 1 7 at 1000ms\nkill me"), [].into());
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
            15
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
            let window = match self.dax {
                _ if self.stray => STRAY_WINDOW,
                false => 0,
                true => crate::common::dax_window(crate::common::Window::Large)?,
            };
            // fxdax: on a daemon of its own, which goes on running till its test's end otherwise.
            let daemon = if self.daemon_killed {
                let socket = std::env::temp_dir()
                    .join(format!("fxdax-crash-{}.sock", std::process::id()));
                let daemon = spawn_daemon(&test_setup, &socket, "extentd", &[], None)?;
                vm.extent_store_served("extents", &path, CAPACITY, window, &socket);
                Some(daemon)
            } else {
                vm.extent_store("extents", &path, CAPACITY, window)?;
                None
            };
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
                match &daemon {
                    Some(daemon) => vm.kill_daemon_on(KILL_ME, daemon.id()),
                    // FcExitCode::AccessRefused, as a vCPU's KVM_RUN keeps failing (TASK-70).
                    None if self.stray => vm.stops_with(159),
                    None if self.killed => vm.kill_on(KILL_ME),
                    None => {}
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
                // fxdax: the access never came back, and the daemon serves on (TASK-77).
                if self.stray {
                    let [stray] = (output.lines())
                        .filter(|line| line.starts_with(STRAY))
                        .collect::<Vec<_>>()[..]
                    else {
                        return TestOutcome::Fail(format!("no stray access: {output:?}"));
                    };
                    assert_eq!(output.lines().last(), Some(stray), "the stray access returned");
                    let socket = std::env::var_os("FXDAX_EXTENTD").unwrap();
                    let other = test_setup.tmp_dir.join("after-the-stray");
                    fxdax::extentd::RemoteExtentStore::connect(socket, other, 1 << 24).unwrap();
                }
                if self.daemon_killed {
                    crate::common::say_daemon_stats(&test_setup, "extentd");
                }
                // The store as the crash left it, for a failure to be looked into: checking it
                // recovers it, and mounting it replays its journal.
                crate::common::copy_store(&path, &test_setup.tmp_dir.join("extents-at-crash"))
                    .unwrap();
                let storage = block_on(Storage::open(&path).unwrap().check()).unwrap();
                let fs = block_on(Filesystem::mount(storage)).unwrap();
                // fxdax: the rest of the test's checks count on what was told of being there.
                if self.daemon_killed {
                    let lost = assert_kept_prefix(&fs, &synced, &synced_at_the_kill(&output));
                    eprintln!("{lost} of {} files told of went with the daemon", synced.len());
                    block_on(fs.close()).unwrap();
                    return TestOutcome::Pass;
                }
                assert_synced(&fs, &synced);
                // fxdax: what the directory's default ACL handed down came with the creations a
                // journal flush made durable, their inodes never written back (TASK-79.29).
                {
                    let acl = |node: &fxdax::Node, name: &[u8]| block_on(node.getxattr(name)).unwrap();
                    let (dir, _) = block_on(fs.root().lookup(INHERITS.as_bytes())).unwrap();
                    assert_eq!(acl(&dir, b"system.posix_acl_default"), default_acl());
                    let (file, attr) = block_on(dir.lookup(INHERITS_FILE.as_bytes())).unwrap();
                    assert_eq!(acl(&file, b"system.posix_acl_access"), inherited_acl(6, 6, 4));
                    assert_eq!(attr.perm, 0o664, "the inheriting file's mode");
                    let (child, attr) = block_on(dir.lookup(INHERITS_DIR.as_bytes())).unwrap();
                    assert_eq!(acl(&child, b"system.posix_acl_access"), default_acl());
                    assert_eq!(acl(&child, b"system.posix_acl_default"), default_acl());
                    assert_eq!(attr.perm, 0o775, "the inheriting directory's mode");
                }
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
                // fxdax: a page synced, then stored to through a mapping, and written back again
                // and again as the VM stops: it holds what was synced, but for the byte stored,
                // which it may or may not hold. It held zeros (TASK-79.11).
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
    use crate::guest_fxfs::{evict, is_dax, read_in_sealed};

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

    /// fxdax: how many of the window's pages are private, copying on write (doc-13).
    fn private_pages() -> u64 {
        crate::guest_fxfs::stats("extents")["dax_private_pages"]
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
    /// calls failed. Fxfs's Rust's runtime's allocations, rt.c's (fxfs_rt_*), its allocator's and
    /// its executor's, are made not to fail (__GFP_NOFAIL), which the injection skips: one failing
    /// panicked the kernel, or wrote through NULL (TASK-60.1, GCE r260, r262).
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
        // What report() maps, before the writers' allocations fail: its mmap failed (GCE r268).
        quiet();
        for attr in ATTRS {
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
        // As many files as a writer makes at most, in the seconds its allocations fail.
        const FILES: usize = 4096;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (1..=2u8)
            .map(|writer| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    // Its memory, as much as it needs, before its allocations fail: a large one,
                    // mapped, failed, and the agent, init, aborted (GCE r269).
                    let mut bytes = vec![0; (0..FILES).map(durable_len).max().unwrap()];
                    let mut name = String::with_capacity(64);
                    // Its allocations, alone, fail.
                    fs::write("/proc/thread-self/make-it-fail", "1").unwrap();
                    let mut failed = 0u64;
                    for index in 0..FILES {
                        if stop.load(std::sync::atomic::Ordering::Acquire) {
                            break;
                        }
                        let data = &mut bytes[..durable_len(index)];
                        durable_into(writer, index, data);
                        name.clear();
                        std::fmt::Write::write_fmt(
                            &mut name,
                            format_args!("{MNT}/synced-{writer}-{index}"),
                        )
                        .unwrap();
                        let attempt = || -> std::io::Result<()> {
                            let mut file = fs::File::create(&name)?;
                            file.write_all(data)?;
                            file.sync_all()?;
                            fs::File::open(MNT)?.sync_all()
                        };
                        match attempt() {
                            Ok(()) => report(&format!("synced {writer} {index}")),
                            Err(_) => failed += 1,
                        }
                    }
                    // Writing it allocates, which may fail until it is written (GCE r255).
                    while let Err(err) = fs::write("/proc/thread-self/make-it-fail", "0") {
                        assert_eq!(err.raw_os_error(), Some(libc::ENOMEM), "make-it-fail: {err}");
                    }
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
        // No window space lost to an UNMAP that failed: its request's allocation can't
        // (TASK-44.22).
        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
        assert!(stats.lines().any(|line| line == "window_lost_bytes 0"), "{stats}");
        // The DAX window's latency histograms, each 24 buckets' counts (TASK-111.6).
        for name in ["window_map_requests", "window_unmaps", "dax_faults"] {
            let line = stats.lines().find_map(|line| line.strip_prefix(&format!("{name}_hist ")));
            let buckets = line.unwrap_or_else(|| panic!("{name}_hist: {stats}")).split(',');
            assert_eq!(buckets.map(|count| count.parse::<u64>().unwrap()).count(), 24, "{name}");
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

    /// fxdax: when the crash test's writers began ([`since_writers_began`]).
    static WRITERS_BEGAN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

    /// fxdax: the milliseconds since the crash test's writers began.
    fn since_writers_began() -> u128 {
        WRITERS_BEGAN.get().expect("the writers began").elapsed().as_millis()
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
            report_out(&format!("synced {writer} {index} at {}ms", since_writers_began()));
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
    /// begun. Quiet is read under stdout's lock, which the killed crash test sets it and says
    /// KILL_ME under: no line, nor the write it says begins, comes after that (GCE r261).
    fn report(line: &str) {
        while_loud(|stdout| {
            writeln!(stdout, "{line}").unwrap();
            stdout.flush().unwrap();
        });
    }

    /// Runs `say` under stdout's lock, unless the VM is about to stop ([`report`]).
    fn while_loud(say: impl Fn(&mut std::io::StdoutLock)) {
        loop {
            let mut stdout = std::io::stdout().lock();
            if !quiet().load(std::sync::atomic::Ordering::Acquire) {
                say(&mut stdout);
                return;
            }
            drop(stdout);
            std::thread::park();
        }
    }

    /// [`report`]s `line` for the host to hold before the caller goes on: of a write, whose bytes
    /// the store may hold as the VM stops, as it begins.
    fn report_out(line: &str) {
        while_loud(|_| crate::guest_init::say_out(line));
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
        report_out(&format!("resized 0 {first}"));
        let mut size = first;
        for generation in 1u32.. {
            while PAUSED.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let next = resized_size(generation);
            report_out(&format!("resizing {generation} {next}"));
            file.set_len(next as u64).unwrap();
            if next > size {
                file.write_all_at(&full[size..next], size as u64).unwrap();
            }
            file.sync_all().unwrap();
            size = next;
            report_out(&format!("resized {generation} {next}"));
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
            report_out(&format!("rewrote {file} {page} 0"));
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
            report_out(&format!("rewriting {file} {page} {generation}"));
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
            report_out(&format!("rewrote {file} {page} {generation}"));
        }
        unreachable!()
    }

    /// fxdax: [`INHERITS`], its default ACL durable, and what inherits it, made durable by a
    /// journal flush for another inode's sake: nothing writes theirs back before the VM stops.
    fn inherit_acls() {
        let inherits = format!("{MNT}/{INHERITS}");
        fs::create_dir(&inherits).unwrap();
        let path = CString::new(inherits.as_str()).unwrap();
        let acl = default_acl();
        let name = c"system.posix_acl_default";
        let set = unsafe {
            libc::setxattr(path.as_ptr(), name.as_ptr(), acl.as_ptr().cast(), acl.len(), 0)
        };
        assert_eq!(set, 0, "setxattr: {}", std::io::Error::last_os_error());
        fs::File::open(&inherits).unwrap().sync_all().unwrap();
        fs::File::create(format!("{inherits}/{INHERITS_FILE}")).unwrap();
        fs::create_dir(format!("{inherits}/{INHERITS_DIR}")).unwrap();
        fs::File::open(&inherits).unwrap().sync_all().unwrap();
    }

    /// Stops the VM mid-workload: with two writers fsyncing, and without syncing (reboot(2)
    /// doesn't); or, if `killed`, has its host kill its process there ([`KILL_ME`]); or, if `stray`, touches the window where
    /// nothing is mapped ([`TestFxfsGuest::crash_stray`]).
    fn crash(dax: bool, killed: bool, stray: bool) -> ! {
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
        inherit_acls();
        let mapped = format!("{MNT}/{MAPPED_SYNCED}");
        if dax {
            let mut file = fs::File::create(&mapped).unwrap();
            file.write_all(&mapped_synced()).unwrap();
            file.sync_all().unwrap();
            fs::File::open(MNT).unwrap().sync_all().unwrap();
            // fxdax: created here, it is a page-cache inode until read in again (doc-13,
            // TASK-164.3): read in again, DAX, its store is the window's.
            drop(file);
            read_in_sealed(&mapped);
        }
        // fxdax: unless its process is to be killed, whose host the guest asks to (TASK-119.6).
        let stopper = (!killed).then(Stopper::fork);
        // fxdax: what the writers say is timed from here, as the VM's stop is (TASK-165.9).
        WRITERS_BEGAN.get_or_init(std::time::Instant::now);
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
                // The writers quiet first, under stdout's lock (`report`): a write the host
                // doesn't hear of, said after KILL_ME, on which it kills the VM, would reach the
                // store, which the checks would find written by no write said (GCE r261).
                let mut stdout = std::io::stdout().lock();
                quiet().store(true, std::sync::atomic::Ordering::Release);
                let access = stray.then(|| stray_access(crash_ms));
                match &access {
                    Some(access) => writeln!(stdout, "{STRAY}{access}").unwrap(),
                    None => {
                        // fxdax: when, for the files synced just before, which a daemon killed may
                        // lose ([`synced_at`], TASK-165.9).
                        writeln!(stdout, "{KILLING}{}ms", since_writers_began()).unwrap();
                        writeln!(stdout, "{KILL_ME}").unwrap();
                    }
                }
                stdout.flush().unwrap();
                drop(stdout);
                if let Some(access) = access {
                    // As long as the console's drained, the line reaches the host before the VM stops.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    fs::write("/sys/module/fxfs/parameters/dax_stray", access).unwrap();
                    println!("the stray access returned");
                }
                loop {
                    std::thread::park();
                }
            }
        }
    }

    /// fxdax: a load or store (fxfs.dax_stray) of the window where nothing is mapped, past what the
    /// writers reach, within a page or across two, as `crash_ms` picks.
    fn stray_access(crash_ms: u64) -> String {
        let op = if crash_ms & 1 == 0 { "load" } else { "store" };
        // But for the last page, which a word across two would pass the end of.
        let page = (crash_ms * 7919) % (STRAY_WINDOW - STRAY_WITHIN - 4096) / 4096;
        let within = if crash_ms & 2 == 0 { 0 } else { 4092 };
        format!("{op} {}", STRAY_WITHIN + page * 4096 + within)
    }

    /// fxdax: what stops the crash test's VM, a process of its own, forked before the writers
    /// begin: a writer's fault may wait holding its process's memory map lock, which the agent's
    /// threads then waited for, the stop's among them, which said nothing and stopped no VM (GCE
    /// r221); nor does it say anything through the agent's
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
        /// Forks the process that stops the VM, once told to.
        fn fork() -> Self {
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

    /// fxdax: stores a byte to the page at `path`, synced, through a mapping, as the VM is about
    /// to stop, with the writers waiting (TASK-79.11): the store makes the page private, one page
    /// more (doc-13), and a thread of its own stores the byte again and writes the page back, again
    /// and again, so that the VM stops with a writeback of it under way, or just done. The store
    /// holds what was synced, but for the byte, either way: a writeback that wrote the extent's
    /// page, not the window's, or committed before its bytes, leaves other data.
    fn store_mapped(path: &str) {
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
        let before = private_pages();
        unsafe { map.cast::<u8>().add(MAPPED_STORE).write_volatile(0xee) };
        assert_eq!(private_pages(), before + 1, "the store's private pages");
        let page = map as usize;
        std::thread::spawn(move || {
            let page = page as *mut u8;
            loop {
                unsafe { page.add(MAPPED_STORE).write_volatile(0xee) };
                let synced = unsafe { libc::msync(page.cast(), 4096, libc::MS_SYNC) };
                assert_eq!(synced, 0, "msync: {}", std::io::Error::last_os_error());
            }
        });
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "stored mapped").unwrap();
        stdout.flush().unwrap();
    }

    /// The device's count of the requests of `kind` (as `device_requests` names them).
    fn requests(kind: &str) -> u64 {
        device_requests()
            .lines()
            .find_map(|line| line.strip_prefix(kind)?.strip_prefix(' ')?.parse().ok())
            .unwrap_or_else(|| panic!("no count of {kind} requests"))
    }

    /// A file's window ranges outlive its inode. fxdax: a file created here is a page-cache inode
    /// until read in again (doc-13, TASK-164.3): read in, DAX, its first read maps it, and read in
    /// again after that, it takes no MAP and no READ. Fails if its ranges went with the inode the
    /// first read mapped them for.
    fn read_maps() {
        let path = format!("{MNT}/mapped-read");
        let data = pattern((3 << 20) + 5, 0x3c);
        fs::write(&path, &data).unwrap();
        evict(&path);
        assert!(is_dax(&path), "{path} is not DAX");
        assert_eq!(fs::read(&path).unwrap(), data, "{path} read in again");
        evict(&path);
        // What earlier writes left to do in the background, done, before counting.
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        let (reads, maps) = (requests("read"), requests("map"));
        assert_eq!(fs::read(&path).unwrap(), data);
        assert_eq!(requests("read"), reads, "reading the file read from the device");
        assert_eq!(requests("map"), maps, "reading the file mapped its extents");
        fs::remove_file(&path).unwrap();
    }

    /// Small files share an extent, which the window maps once for them all. fxdax: files created
    /// here are page-cache inodes until read in again (doc-13, TASK-164.3): their writes take no
    /// MAP, and their writeback packs them in an extent, sealed, which, read in again, DAX, they
    /// share: their reads take one MAP of its region, and no READ; evicted and read in again, they
    /// take neither. Fails if a created file got S_DAX, if the files read in again didn't, or if
    /// they didn't share a region (a MAP each).
    fn small_files_share() {
        let dir = format!("{MNT}/small");
        fs::create_dir(&dir).unwrap();
        let files: Vec<(String, Vec<u8>)> =
            (0..32).map(|i| (format!("{dir}/file-{i}"), pattern(100 + i, i as u8))).collect();
        // What earlier writes left to do in the background, done, before counting.
        fs::write(format!("{dir}/seed"), b"seed").unwrap();
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        let (reads, maps) = (requests("read"), requests("map"));
        for (path, data) in &files {
            fs::write(path, data).unwrap();
        }
        // Their writeback, which maps nothing.
        unsafe { libc::sync() };
        unsafe { libc::sync() };
        assert_eq!(requests("map"), maps, "the small files' MAPs");
        assert_eq!(requests("read"), reads, "writing small files read from the device");
        assert!(!is_dax(&files[0].0), "a small file created here is DAX");
        for (path, _) in &files {
            evict(path);
        }
        // Read in again, DAX, they map the extent their writeback packed them in, sealed, once for
        // them all.
        let (reads, maps) = (requests("read"), requests("map"));
        for (path, data) in &files {
            assert_eq!(&fs::read(path).unwrap(), data, "{path}");
        }
        assert!(is_dax(&files[0].0), "a small file read in again is not DAX");
        assert_eq!(requests("map"), maps + 1, "reading small files in again: their MAPs");
        assert_eq!(requests("read"), reads, "reading them in again read from the device");
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

    /// The kernel's text is where vmlinux links it, unrandomized: Firecracker boots the ELF at its
    /// 64-bit entry, past the decompressor, which alone randomizes it (KASLR, RANDOMIZE_BASE), so
    /// that the symbols the build's vmlinux has symbolize host profiles of the guest and its stacks
    /// (TASK-131.6). On x86_64, `__START_KERNEL_map` and `PHYSICAL_START`, 16 MiB; arm64's
    /// configuration randomizes nothing.
    fn kernel_text_unrandomized() {
        if cfg!(target_arch = "x86_64") {
            let symbols = fs::read_to_string("/proc/kallsyms").unwrap();
            let text = (symbols.lines())
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    let address = fields.next()?;
                    (fields.nth(1)? == "_text").then(|| u64::from_str_radix(address, 16).unwrap())
                })
                .expect("_text in /proc/kallsyms");
            assert_eq!(text, 0xffff_ffff_8100_0000, "the kernel's text randomized: {text:#x}");
        }
    }

    /// TASK-111.7: the filesystem says the commit it was built of, in /proc/fs/fxfs/build, as
    /// scripts/build-commit.sh said for the tree the kernel was built from: a commit, `-dirty` if
    /// a tracked file differed from it.
    fn build_commit_shown() {
        let build = fs::read_to_string("/proc/fs/fxfs/build").unwrap();
        let commit = (build.strip_prefix("commit ").and_then(|line| line.strip_suffix('\n')))
            .unwrap_or_else(|| panic!("/proc/fs/fxfs/build: {build:?}"));
        let commit = commit.strip_suffix("-dirty").unwrap_or(commit);
        assert_eq!(commit.len(), 40, "/proc/fs/fxfs/build: {build:?}");
        assert!(commit.bytes().all(|byte| byte.is_ascii_hexdigit()), "{build:?}");
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
        // and the next for what the first's seal left.
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
                crash(self.dax, self.killed, self.stray);
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
            kernel_text_unrandomized();
            build_commit_shown();
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
