// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Followers' torture test (fxdax's backlog doc-8 testing 6, TASK-42.7), as Linux's rcutorture: a
//! writer VM changes its filesystem for a while, syncing as it goes, and two follower VMs follow
//! it, read-only, while their readers check what they read, each against what can be:
//!
//! - a token renamed within a directory, `token.N` to `token.N+1`: a listing of the directory holds
//!   exactly one, whose N never goes back (a rename seen half applied shows none, or two);
//! - blocks of a file the writer rewrites one at a time, each holding its generation, its index and
//!   a checksum: a read of one, through read(2) and through a mapping, is whole, and its generation
//!   never goes back; as is one by a read(2) into a private mapping of the file itself, whose copy
//!   faults the page it copies to in from the follower: a fill inside the read(2)'s;
//! - a file the writer truncates, extends and rewrites, read through a mapping (as the kernel reads
//!   it on a reader's behalf, through /proc/self/mem: a page past the end fails the read, not the
//!   reader), each page whole;
//! - files the writer creates and deletes: one a follower opens before its deletion reads whole
//!   after it, as POSIX has it.
//!
//! Each of the writer's writes is written back before its next operation, so that its operations
//! reach its store in the order it makes them, as the history checker's model has it (below).
//!
//! Once the writer is done, it writes what it left (the token's N, the blocks' generations), and
//! the followers, once they find it, must hold exactly that.
//!
//! The VMs send the host their histories, which it checks once they are done with TASK-42.1's
//! checker (`follower_history`): the writer's operations, each with the files it changed, and the
//! store's latest commit as each of its syncs returned; and its followers' reads, each with the
//! commits its follower may have been as of (fs/fxfs's FXFS_IOC_FOLLOW_EPOCHS). Each reader must
//! find what a prefix of the writer's operations those commits may hold left, a prefix no shorter
//! than its last read's. A reader records each read that found what it last found of the file
//! otherwise, and the others every RECORD_EVERY.
//!
//! The kernel's command line may widen
//! the protocol's windows (fs/fxfs's `follow_apply_delay_us`, `follow_iomap_delay_us`), and no VM's
//! kernel may complain (a debug kernel's lockdep, KCSAN and DEBUG_ATOMIC_SLEEP, or the follower's
//! own WARN_ONs). A mutant, with one window widened and the rule that closes it switched off, must
//! be caught: its followers report what is stale, and its history is wrong.

use macros::{guest, host};

pub struct TestFxfsGuestFollowTorture {
    /// Whether the devices have DAX windows.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    /// The followers' rule switched off, if any: the mutation they must catch.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub mutant: Option<Mutant>,
}

/// A rule of the followers' that a mutant of the torture test switches off, with the window it
/// closes widened (doc-8, testing 6).
#[derive(Clone, Copy)]
#[cfg_attr(feature = "guest", allow(dead_code))]
pub enum Mutant {
    /// Their batches' drops and settling: the window between a batch applied and settled.
    Invalidation,
    /// Their batches' drops and settling, with the window between a DAX fault's extents and its
    /// mapping widened instead: a fault maps what a batch changed after, which nothing drops.
    InvalidationFaults,
    /// Their readers' fill barrier (TASK-42.10): the window between a batch dropping what the
    /// kernel keeps of what it changes and applying it, and between its apply and its attributes
    /// set. A reader's fault maps an extent of the state before a batch after the batch let its
    /// commit go: extentd refuses the window's map of the extent released (doc-8, testing 1), and
    /// the follower's VMM, which can't resolve the fault, aborts; or a reader finds a name the
    /// batch half applied; what its readers found is a wrong history.
    Barrier,
    /// Their read(2)s' copies without page faults: a copy that faults in a page of a follower's
    /// file, filling it under the barrier the read(2) holds, waits behind a batch waiting for the
    /// read(2): the followers hang.
    Nested,
}

/// The vsock port the writer says it is ready on, which the host waits for before the followers
/// start.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5054;

/// The vsock port each VM sends its history on, which the host checks (TASK-42.1's checker,
/// `follower_history`), one record a line: the writer's operations (`op AFTER NAME=OBJECT:CONTENTS
/// ...`, `NAME=-` for none) and syncs (`sync`, which the host answers with the store's latest
/// commit's epoch); a follower's reads (`read READER FROM TO NAME FOUND`) and reads of files it
/// held open (`held READER FROM TO OBJECT FOUND`), FOUND a number, `-` for nothing, or `!WHAT` for
/// what isn't whole, made as of a commit from FROM to TO.
const HISTORY_PORT: u32 = 5056;

/// A victim's contents once made, and before it is written: nothing.
#[cfg_attr(feature = "host", allow(dead_code))]
const EMPTY: u64 = u64::MAX;

/// How many blocks the generations' file holds, and the bytes of a block.
#[cfg_attr(feature = "host", allow(dead_code))]
const BLOCKS: usize = 64;
#[cfg_attr(feature = "host", allow(dead_code))]
const BLOCK: usize = 4096;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use follower_history::{File, HeldRead, History, Read};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const CAPACITY: u64 = 512 << 20;
    const FOLLOWERS: [&str; 2] = ["follower1", "follower2"];
    /// How long after its writer is done a follower has hung, if it still runs: past the minute it
    /// has to hold what its writer left, and its watchdog's report of where it is stuck.
    const HUNG_AFTER: std::time::Duration = std::time::Duration::from_secs(100);

    /// How long the writer changes its filesystem: FXDAX_FOLLOW_SECONDS, for a soak; 20 by default.
    fn seconds() -> u64 {
        std::env::var("FXDAX_FOLLOW_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(20)
    }

    impl TestFxfsGuestFollowTorture {
        fn spawn(
            &self,
            test_setup: &TestSetup,
            root: &Path,
            role: &str,
            store: &Path,
            uds: &Path,
        ) -> anyhow::Result<Firecracker> {
            let mut vm = Vm::new(2, 768)?;
            vm.guest_kernel()?;
            vm.root_image(&format!("{role}.rootfs.ext4"));
            let window = if self.dax { 1 << 28 } else { 0 };
            if role == "writer" {
                vm.extent_store("extents", store, CAPACITY, window)?;
            } else {
                vm.extent_store_following("extents", store, window)?;
            }
            vm.env(&CString::new(format!("FXDAX_FOLLOW={role}"))?);
            vm.env(&CString::new(format!("FXDAX_FOLLOW_SECONDS={}", seconds()))?);
            // The windows the protocol closes, widened; or a mutant's, the rule closing it off.
            match self.mutant {
                None => {
                    vm.env(c"fxfs.follow_apply_delay_us=200");
                    vm.env(c"fxfs.follow_iomap_delay_us=50");
                }
                Some(Mutant::Invalidation) => {
                    vm.env(c"fxfs.follow_apply_delay_us=200");
                    vm.env(c"fxfs.follow_skip_invalidate=1");
                }
                Some(Mutant::InvalidationFaults) => {
                    vm.env(c"fxfs.follow_iomap_delay_us=50");
                    vm.env(c"fxfs.follow_skip_invalidate=1");
                }
                Some(Mutant::Barrier) => {
                    vm.env(c"fxfs.follow_apply_delay_us=200");
                    vm.env(c"fxfs.follow_skip_barrier=1");
                }
                Some(Mutant::Nested) => {
                    vm.env(c"fxfs.follow_apply_delay_us=200");
                    vm.env(c"fxfs.follow_read_faults=1");
                }
            }
            vm.vsock(uds);
            vm.spawn_api(root, &test_setup.test_case, role, &Vm::api_socket(role))
        }

        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            let store = dir.join("store");
            let uds = |role: &str| -> PathBuf {
                std::env::temp_dir()
                    .join(format!("fxdax-torture-{}-{role}.vsock", std::process::id()))
            };
            let history = Arc::new(Mutex::new(History::new()));
            let listener = UnixListener::bind(format!("{}_{PORT}", uds("writer").display()))?;
            let records = |role: &str| root.parent().unwrap().join(format!("history-{role}.txt"));
            // Each VM's history, which its guest sends first, accepted here, as its VM may end
            // without sending it (`Firecracker::accept`), then taken on a thread of its own.
            let listen =
                |role: &str| UnixListener::bind(format!("{}_{HISTORY_PORT}", uds(role).display()));
            let writing = listen("writer")?;
            let mut writer = self.spawn(test_setup, &root, "writer", &store, &uds("writer"))?;
            let writing = writer.accept(&writing)?;
            let mut recording =
                vec![histories(writing, &records("writer"), history.clone(), Some(store.clone()))?];
            let stream = writer.accept(&listener)?;
            let mut said = String::new();
            BufReader::new(stream).read_line(&mut said)?;
            anyhow::ensure!(said == "ready\n", "the writer said {said:?}");
            let followers = FOLLOWERS
                .iter()
                .map(|role| {
                    Ok((listen(role)?, self.spawn(test_setup, &root, role, &store, &uds(role))?))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let followers = FOLLOWERS
                .iter()
                .zip(followers)
                .map(|(role, (following, mut follower))| {
                    let following = follower.accept(&following)?;
                    recording.push(histories(following, &records(role), history.clone(), None)?);
                    Ok(follower)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            self.ended("writer", writer)?;
            // The followers' time to end runs from the writer's end.
            let hung_after = std::time::Instant::now() + HUNG_AFTER;
            for (role, mut follower) in FOLLOWERS.iter().zip(followers) {
                while follower.running()? && std::time::Instant::now() < hung_after {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if follower.running()? {
                    let console = follower.kill()?;
                    crate::vm::split_console(std::fs::File::open(&console)?)?;
                    println!("HUNG {role}");
                } else {
                    self.ended(role, follower)?;
                }
            }
            for recorded in recording {
                recorded.join().expect("a history's thread panicked")?;
            }
            for role in std::iter::once("writer").chain(FOLLOWERS) {
                let _ = std::fs::remove_file(format!("{}_{HISTORY_PORT}", uds(role).display()));
            }
            let _ = std::fs::remove_file(format!("{}_{PORT}", uds("writer").display()));
            let history = history.lock().unwrap();
            eprintln!("history: {} reads checked", history.reads().len());
            match history.check() {
                Ok(()) => println!("OK history"),
                Err(wrong) => {
                    eprintln!("history: {wrong}");
                    println!("WRONG history");
                }
            }
            Ok(())
        }
    }

    impl TestFxfsGuestFollowTorture {
        /// Takes `role`'s VM's output, once it ends: it must end well, and, but for a mutant, its
        /// kernel complain of nothing.
        fn ended(&self, role: &str, firecracker: Firecracker) -> anyhow::Result<()> {
            let (status, console) = firecracker.wait()?;
            crate::vm::split_console(std::fs::File::open(&console)?)?;
            // A mutant's follower may be stopped by what its mutation broke (Mutant::Barrier).
            if self.mutant.is_some() && role != "writer" && !status.success() {
                eprintln!("{role}'s VM exited {status}");
                println!("ABORTED {role}");
                return Ok(());
            }
            anyhow::ensure!(status.success(), "{role}'s VM exited {status}");
            Ok(())
        }
    }

    /// Takes the history a VM sends on `stream`, from its vsock, into `history`, and to the file at
    /// `records` as it was sent, on a thread of its own, until the VM is done: the writer's,
    /// answering its syncs with the latest commit of the store at `store`, or a follower's. Whole
    /// lines alone: a VM ended at once may have sent half of its last.
    fn histories(
        stream: std::os::unix::net::UnixStream,
        records: &Path,
        history: Arc<Mutex<History<String, u64>>>,
        store: Option<PathBuf>,
    ) -> anyhow::Result<std::thread::JoinHandle<anyhow::Result<()>>> {
        let mut records = std::io::BufWriter::new(std::fs::File::create(records)?);
        Ok(std::thread::spawn(move || {
            let control = match &store {
                Some(_) => {
                    let extentd = std::env::var_os("FXDAX_EXTENTD").expect("FXDAX_EXTENTD set");
                    Some(crate::common::control(Path::new(&extentd))?)
                }
                None => None,
            };
            let mut answers = stream.try_clone()?;
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            while {
                line.clear();
                stream.read_line(&mut line)? > 0
            } {
                let Some(line) = line.strip_suffix('\n') else { break };
                writeln!(records, "{line}")?;
                record(line, &history, &mut || {
                    let (control, store) = (control.as_ref().unwrap(), store.as_ref().unwrap());
                    let epoch = control.committed(store)?;
                    writeln!(answers, "{epoch}")?;
                    Ok(epoch)
                })
                .map_err(|err| err.context(format!("the history's line {line:?}")))?;
            }
            records.flush()?;
            Ok(())
        }))
    }

    /// Takes the record `line` into `history`, a sync's epoch from `synced`.
    fn record(
        line: &str,
        history: &Mutex<History<String, u64>>,
        synced: &mut dyn FnMut() -> anyhow::Result<u64>,
    ) -> anyhow::Result<()> {
        let words: Vec<&str> = line.split(' ').collect();
        let number = |word: &str| -> anyhow::Result<u64> { Ok(word.parse()?) };
        let found = |word: &str| -> anyhow::Result<Result<Option<u64>, String>> {
            Ok(match word {
                "-" => Ok(None),
                _ if word.starts_with('!') => Err(word[1..].replace('_', " ")),
                _ => Ok(Some(number(word)?)),
            })
        };
        match words[..] {
            ["op", after, ref changes @ ..] => {
                let changes = changes
                    .iter()
                    .map(|change| {
                        let (name, file) = change.split_once('=').context("a change's name")?;
                        let file = match file {
                            "-" => None,
                            file => {
                                let (object_id, contents) =
                                    file.split_once(':').context("a file's object")?;
                                Some(File {
                                    object_id: number(object_id)?,
                                    contents: number(contents)?,
                                })
                            }
                        };
                        Ok((name.to_owned(), file))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                history.lock().unwrap().operation(number(after)?, changes);
            }
            ["sync"] => {
                let epoch = synced()?;
                history.lock().unwrap().synced(epoch);
            }
            ["read", reader, from, to, name, what] => {
                let read = Read {
                    reader: number(reader)? as usize,
                    epochs: number(from)?..=number(to)?,
                    name: name.to_owned(),
                    found: found(what)?,
                };
                history.lock().unwrap().read(read);
            }
            ["held", reader, from, to, object_id, what] => {
                let found = found(what)?.and_then(|found| found.ok_or_else(|| "nothing".into()));
                let read = HeldRead {
                    reader: number(reader)? as usize,
                    epochs: number(from)?..=number(to)?,
                    object_id: number(object_id)?,
                    found,
                };
                history.lock().unwrap().held_read(read);
            }
            _ => anyhow::bail!("not a record"),
        }
        Ok(())
    }

    impl Test for TestFxfsGuestFollowTorture {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            seconds() + 180
        }

        /// A mutant's kernel complains of what its mutation broke.
        fn checks_console(&self) -> bool {
            self.mutant.is_none()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            let expected = match self.mutant {
                None => "OK writer\nOK follower1\nOK follower2\nOK history\n",
                Some(Mutant::Nested) => "OK writer\nHUNG follower1\nHUNG follower2\nOK history\n",
                // How a mutant's followers fail depends on the schedule: a reader finds a page
                // stale (STALE), a fault maps an extent a batch let go of, which extentd refuses
                // and the VMM aborts on (ABORTED), or a reader finds what it can't, and stops (no
                // line). Caught, none passes, and what they read is a wrong history.
                Some(Mutant::Invalidation | Mutant::InvalidationFaults | Mutant::Barrier) => {
                    let lines: Vec<&str> = output.lines().collect();
                    let caught = lines.first() == Some(&"OK writer")
                        && lines.last() == Some(&"WRONG history")
                        && !lines.iter().any(|line| line.starts_with("OK follower"));
                    return match caught {
                        true => TestOutcome::Pass,
                        false => TestOutcome::Fail(format!(
                            "expected the writer OK, no follower OK and a wrong history, got \
                             {output:?}"
                        )),
                    };
                }
            };
            if output == expected {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"))
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::collections::{BTreeMap, HashMap};
    use std::ffi::CString;
    use std::fs;
    use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileExt, MetadataExt};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";
    /// How long followers take at most to hold what the writer left, once it is done.
    const CONVERGES_WITHIN: Duration = Duration::from_secs(60);
    /// The shaped file's largest size, in pages.
    const SHAPE_PAGES: u64 = 256;

    fn path(name: &str) -> String {
        format!("{MNT}/{name}")
    }

    fn seconds() -> u64 {
        std::env::var("FXDAX_FOLLOW_SECONDS").unwrap().parse().unwrap()
    }

    fn mount() {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        let ret = unsafe { libc::umount(target.as_ptr()) };
        assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
    }

    fn sync() {
        let dir = fs::File::open(MNT).unwrap();
        assert_eq!(unsafe { libc::syncfs(dir.as_raw_fd()) }, 0, "syncfs");
    }

    /// A checksum of `data`: FNV-1a.
    fn checksum(data: &[u8]) -> u64 {
        data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// A page holding `generation` and `index`, filled from them, its checksum in its last 8 bytes.
    fn page(generation: u64, index: u64) -> Vec<u8> {
        let mut page = vec![0u8; BLOCK];
        page[..8].copy_from_slice(&generation.to_le_bytes());
        page[8..16].copy_from_slice(&index.to_le_bytes());
        for (at, byte) in page[16..BLOCK - 8].iter_mut().enumerate() {
            *byte = (generation as u8) ^ (index as u8) ^ (at as u8);
        }
        let sum = checksum(&page[..BLOCK - 8]);
        page[BLOCK - 8..].copy_from_slice(&sum.to_le_bytes());
        page
    }

    /// A page's generation and index, if it is whole: its checksum matches.
    fn read_page(page: &[u8]) -> Option<(u64, u64)> {
        let sum = u64::from_le_bytes(page[BLOCK - 8..].try_into().unwrap());
        (checksum(&page[..BLOCK - 8]) == sum).then(|| {
            let generation = u64::from_le_bytes(page[..8].try_into().unwrap());
            (generation, u64::from_le_bytes(page[8..16].try_into().unwrap()))
        })
    }

    /// What a page that isn't whole holds: zeros, as a hole reads; or the generation and index its
    /// header says, and which of its 4 KiB's 512-byte sectors match that header's body, as a page
    /// written over while it was copied would not.
    fn describe(page: &[u8]) -> String {
        if page.iter().all(|byte| *byte == 0) {
            return "zeros".to_owned();
        }
        let generation = u64::from_le_bytes(page[..8].try_into().unwrap());
        let index = u64::from_le_bytes(page[8..16].try_into().unwrap());
        let whole = self::page(generation, index);
        let sectors: String = page
            .chunks(512)
            .zip(whole.chunks(512))
            .map(|(read, header)| if read == header { '=' } else { 'x' })
            .collect();
        format!("generation {generation}, index {index}, sectors {sectors}")
    }

    /// A victim file's content: what it reads, whole, deleted or not.
    fn victim(n: u64) -> Vec<u8> {
        (0..3 * BLOCK as u64).map(|at| (n ^ at) as u8).collect()
    }

    /// A stream to the host's vsock `port`.
    fn connect(port: u32) -> fs::File {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::{FromRawFd as _, IntoRawFd as _};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, port)).expect("connecting to the host");
        // SAFETY: the socket's descriptor, which the file owns from here on.
        unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) }
    }

    /// What a VM tells the host's history checker (HISTORY_PORT), one record a line; and, the
    /// writer's, the host's answers.
    struct History {
        records: Mutex<BufWriter<fs::File>>,
        answers: Mutex<BufReader<fs::File>>,
    }

    impl History {
        fn connect() -> Self {
            let stream = connect(HISTORY_PORT);
            let answers = Mutex::new(BufReader::new(stream.try_clone().unwrap()));
            Self { records: Mutex::new(BufWriter::new(stream)), answers }
        }

        fn record(&self, record: std::fmt::Arguments) {
            let mut records = self.records.lock().unwrap();
            records.write_fmt(record).unwrap();
            records.write_all(b"\n").unwrap();
        }

        /// The writer's operation, which began with the store at `after`, and changed the files
        /// named in `changes`: to `(object, contents)`, or none.
        fn op(&self, after: u64, changes: &[(String, Option<(u64, u64)>)]) {
            let changes: Vec<String> = changes
                .iter()
                .map(|(name, file)| match file {
                    Some((object, contents)) => format!("{name}={object}:{contents}"),
                    None => format!("{name}=-"),
                })
                .collect();
            self.record(format_args!("op {after} {}", changes.join(" ")));
        }

        /// The writer's sync, returned: the epoch of the store's latest commit, as the host says.
        fn synced(&self) -> u64 {
            self.record(format_args!("sync"));
            self.flush();
            let mut epoch = String::new();
            self.answers.lock().unwrap().read_line(&mut epoch).unwrap();
            epoch.trim_end().parse().unwrap()
        }

        fn flush(&self) {
            self.records.lock().unwrap().flush().unwrap();
        }
    }

    /// The objects of the history's files that aren't files of the writer's: the blocks', and the
    /// shaped file's pages'.
    const BLOCK_OBJECTS: u64 = 1 << 62;
    const SHAPE_OBJECTS: u64 = 1 << 61;

    /// The commits a follower's state is as of (fs/fxfs's FXFS_IOC_FOLLOW_EPOCHS): every batch up
    /// to `settled`'s invalidated, none past `applied`'s applied.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Epochs {
        settled: u64,
        applied: u64,
    }

    nix::ioctl_read!(follow_epochs, b'x', 0x42, Epochs);

    /// A follower's batches, and how long they waited for its fill barrier (fs/fxfs's
    /// FXFS_IOC_FOLLOW_STATS).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Stats {
        batches: u64,
        barrier_wait_ns: u64,
        barrier_wait_max_ns: u64,
    }

    nix::ioctl_read!(follow_stats, b'x', 0x43, Stats);

    fn stats(dir: &fs::File) -> Stats {
        let mut stats = Stats::default();
        // SAFETY: The ioctl writes a `Stats`.
        unsafe { follow_stats(dir.as_raw_fd(), &mut stats) }.expect("FXFS_IOC_FOLLOW_STATS");
        stats
    }

    /// The commits the follower whose directory `dir` is is as of now.
    fn epochs(dir: &fs::File) -> Epochs {
        let mut epochs = Epochs::default();
        // SAFETY: The ioctl writes an `Epochs`.
        unsafe { follow_epochs(dir.as_raw_fd(), &mut epochs) }.expect("FXFS_IOC_FOLLOW_EPOCHS");
        epochs
    }

    /// What a reader found of a file: its contents, nothing, or, not whole, what it found.
    type Found = Result<Option<u64>, String>;

    /// How often, at most, a reader records a read that found what it last found.
    const RECORD_EVERY: Duration = Duration::from_millis(1);

    /// A follower's reader `id`, which records its reads in `history`: each that found what it
    /// last found of the name otherwise, and the others every RECORD_EVERY, with the commits its
    /// follower may have been as of as it read.
    struct Reader {
        id: usize,
        history: Arc<History>,
        dir: fs::File,
        last: HashMap<String, Found>,
        recorded: Instant,
    }

    impl Reader {
        fn new(id: usize, history: Arc<History>) -> Self {
            let dir = fs::File::open(MNT).unwrap();
            Self { id, history, dir, last: HashMap::new(), recorded: Instant::now() }
        }

        /// Reads the file `name` with `read`, recording what it found.
        fn read(&mut self, name: &str, read: impl FnOnce() -> Found) -> Found {
            let before = epochs(&self.dir);
            let found = read();
            if self.last.get(name) != Some(&found) || self.recorded.elapsed() >= RECORD_EVERY {
                let after = epochs(&self.dir);
                let (id, from, to) = (self.id, before.settled, after.applied);
                let what = show(&found);
                self.history.record(format_args!("read {id} {from} {to} {name} {what}"));
                self.last.insert(name.to_owned(), found.clone());
                self.recorded = Instant::now();
            }
            found
        }

        /// Reads the file held open whose object is `object` with `read`, recording what it found.
        fn held(&mut self, object: u64, read: impl FnOnce() -> Found) -> Found {
            let before = epochs(&self.dir);
            let found = read();
            let after = epochs(&self.dir);
            let (id, from, to, what) = (self.id, before.settled, after.applied, show(&found));
            self.history.record(format_args!("held {id} {from} {to} {object} {what}"));
            found
        }
    }

    /// What a reader found, as a record says it.
    fn show(found: &Found) -> String {
        match found {
            Ok(None) => "-".to_owned(),
            Ok(Some(contents)) => contents.to_string(),
            Err(what) => format!("!{}", what.replace(' ', "_")),
        }
    }

    /// Writes back `file`'s `len` bytes at `offset`, written, before the writer's next operation:
    /// so that its operations reach its store's journal in the order it makes them, as the history
    /// checker's model has it, a prefix of them in each commit. Written only to the page cache,
    /// without DAX, a write reaches the journal as it is written back, after the operations made
    /// meanwhile: a size changed since commits before a page written before it, and a file deleted
    /// before its writeback never had what it held (TASK-42.12). A writeback, not a sync: it waits
    /// for no commit.
    fn written(file: &fs::File, offset: u64, len: usize) {
        let flags = libc::SYNC_FILE_RANGE_WAIT_BEFORE
            | libc::SYNC_FILE_RANGE_WRITE
            | libc::SYNC_FILE_RANGE_WAIT_AFTER;
        // SAFETY: a system call on the file's descriptor, which it holds open.
        let ret =
            unsafe { libc::sync_file_range(file.as_raw_fd(), offset as i64, len as i64, flags) };
        assert_eq!(ret, 0, "sync_file_range: {}", std::io::Error::last_os_error());
    }

    /// The writer: changes its filesystem for `seconds()`, syncing every few changes, then
    /// writes what it left, in `done`.
    fn writer() {
        let history = History::connect();
        // The store's epoch, as of its last sync: each operation since begins after it.
        let mut after = 0;
        fs::create_dir(path("tok")).unwrap();
        fs::write(path("tok/token.0"), b"").unwrap();
        let token_object = fs::metadata(path("tok/token.0")).unwrap().ino();
        history.op(after, &[("token".to_owned(), Some((token_object, 0)))]);
        let blocks = fs::OpenOptions::new().create(true).write(true).open(path("blocks")).unwrap();
        let mut generations = vec![0u64; BLOCKS];
        for (index, generation) in generations.iter().enumerate() {
            let offset = (index * BLOCK) as u64;
            blocks.write_all_at(&page(*generation, index as u64), offset).unwrap();
            written(&blocks, offset, BLOCK);
            let block = (BLOCK_OBJECTS | index as u64, *generation);
            history.op(after, &[(format!("block.{index}"), Some(block))]);
        }
        let shape = fs::OpenOptions::new().create(true).write(true).open(path("shape")).unwrap();
        let mut shape_pages = 0;
        // The shaped file's pages' generations, 0 for zeros.
        let mut shaped: Vec<u64> = Vec::new();
        sync();
        after = history.synced();
        connect(PORT).write_all(b"ready\n").unwrap();

        let (mut token, mut generation, mut victims) = (0u64, 0u64, 0u64);
        let deadline = Instant::now() + Duration::from_secs(seconds());
        let mut step = 0u64;
        while Instant::now() < deadline {
            step += 1;
            // The token, renamed within its directory.
            fs::rename(
                path(&format!("tok/token.{token}")),
                path(&format!("tok/token.{}", token + 1)),
            )
            .unwrap();
            token += 1;
            history.op(after, &[("token".to_owned(), Some((token_object, token)))]);
            // A block, rewritten.
            generation += 1;
            let index = (step * 7) % BLOCKS as u64;
            generations[index as usize] = generation;
            blocks.write_all_at(&page(generation, index), index * BLOCK as u64).unwrap();
            written(&blocks, index * BLOCK as u64, BLOCK);
            let block = (BLOCK_OBJECTS | index, generation);
            history.op(after, &[(format!("block.{index}"), Some(block))]);
            // The shaped file: truncated, extended, rewritten. Pages it is extended over are
            // zeros.
            let pages = 1 + (step * 13) % SHAPE_PAGES;
            shape.set_len(pages * BLOCK as u64).unwrap();
            let resized: Vec<_> = (pages.min(shape_pages)..pages.max(shape_pages))
                .map(|at| (format!("shape.{at}"), (at < pages).then_some((SHAPE_OBJECTS | at, 0))))
                .collect();
            history.op(after, &resized);
            shape_pages = pages;
            shaped.resize(pages as usize, 0);
            let at = (step * 5) % pages;
            shape.write_all_at(&page(generation, at), at * BLOCK as u64).unwrap();
            written(&shape, at * BLOCK as u64, BLOCK);
            shaped[at as usize] = generation;
            history.op(after, &[(format!("shape.{at}"), Some((SHAPE_OBJECTS | at, generation)))]);
            // A victim, made, written, and an older one deleted.
            let name = format!("victim.{victims}");
            let mut made = fs::File::create(path(&name)).unwrap();
            let object = made.metadata().unwrap().ino();
            history.op(after, &[(name.clone(), Some((object, EMPTY)))]);
            let contents = victim(victims);
            made.write_all(&contents).unwrap();
            written(&made, 0, contents.len());
            drop(made);
            history.op(after, &[(name, Some((object, victims)))]);
            if victims >= 3 {
                fs::remove_file(path(&format!("victim.{}", victims - 3))).unwrap();
                history.op(after, &[(format!("victim.{}", victims - 3), None)]);
            }
            victims += 1;
            if step % 4 == 0 {
                sync();
                after = history.synced();
            }
        }
        // What it left, last: the followers hold it once they find it.
        let left: Vec<String> = std::iter::once(token)
            .chain(generations.iter().copied())
            .map(|n| n.to_string())
            .collect();
        let shaped: Vec<String> = shaped.iter().map(|n| n.to_string()).collect();
        fs::write(path("done.shape"), shaped.join(" ")).unwrap();
        fs::write(path("done.new"), left.join(" ")).unwrap();
        fs::rename(path("done.new"), path("done")).unwrap();
        sync();
        history.synced();
        history.flush();
        drop((blocks, shape));
        unmount();
        println!("OK writer");
    }

    /// What a follower's readers found wrong, and whether to stop.
    struct Checks {
        failures: Mutex<Vec<String>>,
        stop: AtomicBool,
    }

    impl Checks {
        fn fail(&self, failure: String) {
            let mut failures = self.failures.lock().unwrap();
            if failures.len() < 20 {
                failures.push(failure);
            }
        }

        fn running(&self) -> bool {
            !self.stop.load(Ordering::Acquire)
        }
    }

    /// The token's N, as one listing of its directory has it: exactly one token.
    fn listed() -> Found {
        let names: Vec<String> = fs::read_dir(path("tok"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        match &names[..] {
            [name] => match name.strip_prefix("token.").and_then(|n| n.parse().ok()) {
                Some(n) => Ok(Some(n)),
                None => Err(format!("the token's directory listed {name}")),
            },
            names => Err(format!("the token's directory listed {names:?}")),
        }
    }

    fn token(checks: &Checks) -> Option<u64> {
        listed().map_err(|failure| checks.fail(failure)).ok().flatten()
    }

    fn tokens(checks: Arc<Checks>, mut reader: Reader) {
        let mut last = 0;
        while checks.running() {
            match reader.read("token", listed) {
                Ok(Some(n)) => {
                    if n < last {
                        checks.fail(format!("the token went back from {last} to {n}"));
                    }
                    last = n;
                }
                Ok(None) => checks.fail("the token's directory listed nothing".to_owned()),
                Err(failure) => checks.fail(failure),
            }
        }
    }

    /// How many mapped reads of a block in a row may find it torn before it fails: a read of a
    /// mapping copies no one version of a page, and one racing the invalidation that replaces the
    /// page reads part of each. A page left torn stays so.
    const TORN_READS: usize = 8;

    /// How a reader of the blocks reads them.
    #[derive(Clone, Copy)]
    enum Through {
        /// read(2), into the reader's memory.
        Read,
        /// The file's shared mapping.
        Mapping(&'static [u8]),
        /// read(2), into a private, writable mapping of the blocks' file at this address, its
        /// pages dropped each pass (MADV_DONTNEED): each copy to a page the reader no longer has
        /// faults it in from the follower, a fill inside the read(2)'s, which must wait for no
        /// batch waiting for the read(2) (fs/fxfs's fxfs_vfs_follower_read).
        ReadIntoMapping(usize),
    }

    /// Reads the blocks `through` read(2) or a mapping: each whole, its generation never going
    /// back. A read(2) copies one page, which a follower replaces but never changes, so it is
    /// never torn; a mapped read may be, as the page is replaced, and is read again.
    fn blocks(checks: Arc<Checks>, through: Through, mut reader: Reader) {
        let file = fs::File::open(path("blocks")).unwrap();
        let mut last = vec![0u64; BLOCKS];
        let mut buf = vec![0u8; BLOCK];
        while checks.running() {
            if let Through::ReadIntoMapping(addr) = through {
                // SAFETY: The private mapping this reader alone uses, its pages dropped.
                let dropped = unsafe {
                    libc::madvise(addr as *mut libc::c_void, BLOCKS * BLOCK, libc::MADV_DONTNEED)
                };
                assert_eq!(dropped, 0, "MADV_DONTNEED: {}", std::io::Error::last_os_error());
            }
            for (index, last) in last.iter_mut().enumerate() {
                let found = reader.read(&format!("block.{index}"), || {
                    let mut reads = 0;
                    let offset = (index * BLOCK) as u64;
                    let found = loop {
                        match through {
                            Through::Read => file.read_exact_at(&mut buf, offset).unwrap(),
                            Through::Mapping(mapped) => {
                                buf.copy_from_slice(&mapped[index * BLOCK..(index + 1) * BLOCK])
                            }
                            Through::ReadIntoMapping(addr) => {
                                // SAFETY: The block's page of the private mapping, this
                                // reader's alone.
                                let page = unsafe {
                                    std::slice::from_raw_parts_mut(
                                        (addr + index * BLOCK) as *mut u8,
                                        BLOCK,
                                    )
                                };
                                file.read_exact_at(page, offset).unwrap();
                                buf.copy_from_slice(page);
                            }
                        }
                        reads += 1;
                        match read_page(&buf) {
                            None if matches!(through, Through::Mapping(_))
                                && reads < TORN_READS =>
                            {
                                continue;
                            }
                            found => break found,
                        }
                    };
                    match found {
                        Some((generation, at)) if at == index as u64 => Ok(Some(generation)),
                        found => Err(format!(
                            "block {index} is torn ({found:?}, {}, {reads} reads): {}",
                            match through {
                                Through::Read => "read",
                                Through::Mapping(_) => "mapped",
                                Through::ReadIntoMapping(_) => "read into a mapping",
                            },
                            describe(&buf)
                        )),
                    }
                });
                match found {
                    Ok(Some(generation)) => {
                        if generation < *last {
                            checks.fail(format!(
                                "block {index} went back from generation {last} to {generation}"
                            ));
                        }
                        *last = generation;
                    }
                    Ok(None) => checks.fail(format!("block {index} read nothing")),
                    Err(torn) => checks.fail(torn),
                }
            }
        }
    }

    /// Reads the shaped file's pages through a mapping of it, as the kernel reads them for this
    /// process (/proc/self/mem): a page past its end, truncated meanwhile, fails that read alone.
    fn shapes(checks: Arc<Checks>, mut reader: Reader) {
        let shape = fs::File::open(path("shape")).unwrap();
        let len = SHAPE_PAGES as usize * BLOCK;
        // SAFETY: A shared read-only mapping of the file, which this reads through /proc alone.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                shape.as_raw_fd(),
                0,
            )
        };
        assert_ne!(addr, libc::MAP_FAILED);
        let mem = fs::File::open("/proc/self/mem").unwrap();
        let mut buf = vec![0u8; BLOCK];
        while checks.running() {
            for page in 0..SHAPE_PAGES {
                let at = addr as u64 + page * BLOCK as u64;
                let found = reader.read(&format!("shape.{page}"), || {
                    // Past the file's end, the page is none.
                    if mem.read_exact_at(&mut buf, at).is_err() {
                        return Ok(None);
                    }
                    // Zeros: a page extended over, or not rewritten since.
                    if buf.iter().all(|byte| *byte == 0) {
                        return Ok(Some(0));
                    }
                    match read_page(&buf) {
                        Some((generation, index)) if index == page => Ok(Some(generation)),
                        _ => Err(format!("the shaped file's page {page} is torn")),
                    }
                });
                if let Err(torn) = &found {
                    checks.fail(torn.clone());
                }
            }
        }
        // SAFETY: The mapping made above.
        unsafe { libc::munmap(addr, len) };
    }

    /// Opens victims as they appear, and reads each whole once the writer deletes it.
    fn victims(checks: Arc<Checks>, mut reader: Reader) {
        let mut open: BTreeMap<u64, fs::File> = BTreeMap::new();
        while checks.running() {
            for entry in fs::read_dir(MNT).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                let Some(n) = name.strip_prefix("victim.").and_then(|n| n.parse::<u64>().ok())
                else {
                    continue;
                };
                if open.len() < 8 && !open.contains_key(&n) {
                    // Deleted since it was listed: as a local race may have it.
                    if let Ok(file) = fs::File::open(path(&name)) {
                        open.insert(n, file);
                    }
                }
            }
            let deleted: Vec<u64> = open
                .keys()
                .copied()
                .filter(|n| fs::metadata(path(&format!("victim.{n}"))).is_err())
                .collect();
            for n in deleted {
                let mut file = open.remove(&n).unwrap();
                let object = file.metadata().unwrap().ino();
                let found = reader.held(object, || {
                    let mut kept = Vec::new();
                    file.read_to_end(&mut kept).unwrap();
                    if kept == victim(n) {
                        Ok(Some(n))
                    } else if kept.is_empty() {
                        Ok(Some(EMPTY))
                    } else {
                        Err(format!("victim.{n} read {} bytes otherwise", kept.len()))
                    }
                });
                // Empty, as of a commit between its creation and its data, a compaction's: which
                // of the two it may be the history's check of the read says (TASK-79.58).
                if !matches!(found, Ok(Some(held)) if held == n || held == EMPTY) {
                    checks.fail(format!("victim.{n}, deleted while open, reads otherwise"));
                }
            }
        }
    }

    /// What the writer left, once the follower finds it: the token's N, and the blocks'
    /// generations.
    fn left() -> Option<Vec<u64>> {
        match fs::read_to_string(path("done")) {
            Ok(done) => Some(done.split(' ').map(|n| n.parse().unwrap()).collect()),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => panic!("reading done: {error}"),
        }
    }

    /// Whether the follower holds what the writer left: through listings, read(2) and `mapped`;
    /// and the shaped file's pages, through read(2).
    fn holds(left: &[u64], mapped: Option<&[u8]>, checks: &Checks) -> Result<(), String> {
        let found = token(checks);
        if found != Some(left[0]) {
            return Err(format!("the token is {found:?}, not {}", left[0]));
        }
        let blocks = fs::read(path("blocks")).unwrap();
        for (index, generation) in left[1..].iter().enumerate() {
            let pages = [
                Some(&blocks[index * BLOCK..(index + 1) * BLOCK]),
                mapped.map(|mapped| &mapped[index * BLOCK..(index + 1) * BLOCK]),
            ];
            for page in pages.into_iter().flatten() {
                if read_page(page) != Some((*generation, index as u64)) {
                    return Err(format!("block {index} is not of generation {generation}"));
                }
            }
        }
        let shaped = fs::read_to_string(path("done.shape")).unwrap();
        let shaped: Vec<u64> = shaped.split(' ').map(|n| n.parse().unwrap()).collect();
        let shape = fs::read(path("shape")).unwrap();
        if shape.len() != shaped.len() * BLOCK {
            return Err(format!(
                "the shaped file is {} bytes, not {}",
                shape.len(),
                shaped.len() * BLOCK
            ));
        }
        for (at, (generation, page)) in shaped.iter().zip(shape.chunks(BLOCK)).enumerate() {
            let holds = match generation {
                0 => page.iter().all(|byte| *byte == 0),
                generation => read_page(page) == Some((*generation, at as u64)),
            };
            if !holds {
                return Err(format!(
                    "the shaped file's page {at} is not of generation {generation}"
                ));
            }
        }
        Ok(())
    }

    fn follower(role: &str) {
        let checks = Arc::new(Checks { failures: Mutex::default(), stop: AtomicBool::new(false) });
        let history = Arc::new(History::connect());
        // Each reader of each follower is a reader of the history.
        let first: usize = role.strip_prefix("follower").unwrap().parse::<usize>().unwrap() * 8;
        let reader = |n: usize| Reader::new(first + n, history.clone());
        let blocks_file = fs::File::open(path("blocks")).unwrap();
        // SAFETY: A shared read-only mapping of the blocks, which the readers read alone.
        let mapped: &'static [u8] = unsafe {
            let addr = libc::mmap(
                std::ptr::null_mut(),
                BLOCKS * BLOCK,
                libc::PROT_READ,
                libc::MAP_SHARED,
                blocks_file.as_raw_fd(),
                0,
            );
            assert_ne!(addr, libc::MAP_FAILED);
            std::slice::from_raw_parts(addr as *const u8, BLOCKS * BLOCK)
        };
        // SAFETY: A private, writable mapping of the blocks, which one reader reads into alone.
        let private = unsafe {
            let addr = libc::mmap(
                std::ptr::null_mut(),
                BLOCKS * BLOCK,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE,
                blocks_file.as_raw_fd(),
                0,
            );
            assert_ne!(addr, libc::MAP_FAILED);
            addr as usize
        };
        let readers: Vec<std::thread::JoinHandle<()>> = vec![
            {
                let (c, r) = (checks.clone(), reader(0));
                std::thread::spawn(move || tokens(c, r))
            },
            {
                let (c, r) = (checks.clone(), reader(1));
                std::thread::spawn(move || blocks(c, Through::Read, r))
            },
            {
                let (c, r) = (checks.clone(), reader(2));
                std::thread::spawn(move || blocks(c, Through::Mapping(mapped), r))
            },
            {
                let (c, r) = (checks.clone(), reader(5));
                std::thread::spawn(move || blocks(c, Through::ReadIntoMapping(private), r))
            },
            {
                let (c, r) = (checks.clone(), reader(3));
                std::thread::spawn(move || shapes(c, r))
            },
            {
                let (c, r) = (checks.clone(), reader(4));
                std::thread::spawn(move || victims(c, r))
            },
        ];
        // Until the writer is done, and the follower holds what it left, or doesn't in time.
        let deadline = Instant::now() + Duration::from_secs(seconds()) + CONVERGES_WITHIN;
        let converged = loop {
            if let Some(left) = left() {
                match holds(&left, Some(mapped), &checks) {
                    Ok(()) => break Ok(()),
                    Err(stale) if Instant::now() >= deadline => break Err(stale),
                    Err(_) => {}
                }
            } else if Instant::now() >= deadline {
                break Err("the writer's done never came".to_owned());
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        checks.stop.store(true, Ordering::Release);
        for reader in readers {
            reader.join().unwrap();
        }
        history.flush();
        let stats = stats(&fs::File::open(MNT).unwrap());
        eprintln!(
            "{role}: {} batches, which waited for the fill barrier {} ms all told, {} ms at most",
            stats.batches,
            stats.barrier_wait_ns / 1_000_000,
            stats.barrier_wait_max_ns / 1_000_000
        );
        let failures = checks.failures.lock().unwrap().clone();
        drop(blocks_file);
        match (converged, failures.is_empty()) {
            (Ok(()), true) => println!("OK {role}"),
            (converged, _) => {
                for failure in &failures {
                    eprintln!("{role}: {failure}");
                }
                if let Err(stale) = converged {
                    eprintln!("{role}: {stale}");
                }
                println!("STALE {role}");
            }
        }
    }

    /// Says where a VM is stuck, if it runs on past what the test gives it: its blocked tasks',
    /// its CPUs' and every task's stacks (sysrq w, l and t: a task may wait interruptibly), and the
    /// kernel's log, to standard error, once. A VM that hangs says nothing else before the host
    /// gives up on it.
    fn start_watchdog() {
        let after = Duration::from_secs(seconds()) + CONVERGES_WITHIN + Duration::from_secs(30);
        std::thread::spawn(move || {
            std::thread::sleep(after);
            eprintln!("watchdog: still running after {after:?}");
            for key in [b"w", b"l", b"t"] {
                let _ = fs::write("/proc/sysrq-trigger", key);
            }
            crate::kernel_log::print();
        });
    }

    impl Test for TestFxfsGuestFollowTorture {
        fn in_guest(self: Box<Self>) {
            start_watchdog();
            fs::create_dir_all(MNT).unwrap();
            mount();
            match std::env::var("FXDAX_FOLLOW").unwrap().as_str() {
                "writer" => writer(),
                role => follower(role),
            }
        }
    }
}
