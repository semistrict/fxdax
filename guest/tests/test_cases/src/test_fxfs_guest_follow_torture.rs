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
//!   never goes back;
//! - a file the writer truncates, extends and rewrites, read through a mapping (as the kernel reads
//!   it on a reader's behalf, through /proc/self/mem: a page past the end fails the read, not the
//!   reader), each page whole;
//! - files the writer creates and deletes: one a follower opens before its deletion reads whole
//!   after it, as POSIX has it.
//!
//! Once the writer is done, it writes what it left (the token's N, the blocks' generations), and
//! the followers, once they find it, must hold exactly that. The kernel's command line may widen
//! the protocol's windows (fs/fxfs's `follow_apply_delay_us`, `follow_iomap_delay_us`); the mutant
//! (`follow_skip_invalidate`) must be caught: its followers report what is stale.

use macros::{guest, host};

pub struct TestFxfsGuestFollowTorture {
    /// Whether the devices have DAX windows.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    /// Whether the followers skip their invalidation: the mutation they must catch.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub mutant: bool,
}

/// The vsock port the writer says it is ready on, which the host waits for before the followers
/// start.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5054;

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
    use std::ffi::CString;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    const CAPACITY: u64 = 512 << 20;
    const FOLLOWERS: [&str; 2] = ["follower1", "follower2"];

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
            uds: Option<&Path>,
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
            // The windows the protocol closes, widened; or, the mutant, no invalidation.
            if self.mutant {
                vm.env(c"fxfs.follow_skip_invalidate=1");
            } else {
                vm.env(c"fxfs.follow_apply_delay_us=200");
                vm.env(c"fxfs.follow_iomap_delay_us=50");
            }
            if let Some(uds) = uds {
                vm.vsock(uds);
            }
            vm.spawn_api(root, &test_setup.test_case, role, &Vm::api_socket(role))
        }

        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            let store = dir.join("store");
            let uds: PathBuf =
                std::env::temp_dir().join(format!("fxdax-torture-{}.vsock", std::process::id()));
            let listener = UnixListener::bind(format!("{}_{PORT}", uds.display()))?;
            let writer = self.spawn(test_setup, &root, "writer", &store, Some(&uds))?;
            let (stream, _) = listener.accept()?;
            let mut said = String::new();
            BufReader::new(stream).read_line(&mut said)?;
            anyhow::ensure!(said == "ready\n", "the writer said {said:?}");
            let followers = FOLLOWERS
                .iter()
                .map(|role| self.spawn(test_setup, &root, role, &store, None))
                .collect::<anyhow::Result<Vec<_>>>()?;
            for firecracker in std::iter::once(writer).chain(followers) {
                let (status, console) = firecracker.wait()?;
                crate::vm::split_console(std::fs::File::open(&console)?)?;
                anyhow::ensure!(status.success(), "a VM exited {status}");
            }
            Ok(())
        }
    }

    impl Test for TestFxfsGuestFollowTorture {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            seconds() + 180
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
            let expected = if self.mutant {
                "OK writer\nSTALE follower1\nSTALE follower2\n"
            } else {
                "OK writer\nOK follower1\nOK follower2\n"
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
    use std::collections::BTreeMap;
    use std::ffi::CString;
    use std::fs;
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
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

    fn connect() -> fs::File {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::{FromRawFd as _, IntoRawFd as _};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        // SAFETY: the socket's descriptor, which the file owns from here on.
        unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) }
    }

    /// The writer: changes its filesystem for `seconds()`, syncing every few changes, then
    /// writes what it left, in `done`.
    fn writer() {
        fs::create_dir(path("tok")).unwrap();
        fs::write(path("tok/token.0"), b"").unwrap();
        let blocks = fs::OpenOptions::new().create(true).write(true).open(path("blocks")).unwrap();
        let mut generations = vec![0u64; BLOCKS];
        for (index, generation) in generations.iter().enumerate() {
            blocks.write_all_at(&page(*generation, index as u64), (index * BLOCK) as u64).unwrap();
        }
        let shape = fs::OpenOptions::new().create(true).write(true).open(path("shape")).unwrap();
        sync();
        connect().write_all(b"ready\n").unwrap();

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
            // A block, rewritten.
            generation += 1;
            let index = (step * 7) % BLOCKS as u64;
            generations[index as usize] = generation;
            blocks.write_all_at(&page(generation, index), index * BLOCK as u64).unwrap();
            // The shaped file: truncated, extended, rewritten.
            let pages = 1 + (step * 13) % SHAPE_PAGES;
            shape.set_len(pages * BLOCK as u64).unwrap();
            let at = (step * 5) % pages;
            shape.write_all_at(&page(generation, at), at * BLOCK as u64).unwrap();
            // A victim, made, and an older one deleted.
            fs::write(path(&format!("victim.{victims}")), victim(victims)).unwrap();
            if victims >= 3 {
                fs::remove_file(path(&format!("victim.{}", victims - 3))).unwrap();
            }
            victims += 1;
            if step % 4 == 0 {
                sync();
            }
        }
        // What it left, last: the followers hold it once they find it.
        let left: Vec<String> = std::iter::once(token)
            .chain(generations.iter().copied())
            .map(|n| n.to_string())
            .collect();
        fs::write(path("done.new"), left.join(" ")).unwrap();
        fs::rename(path("done.new"), path("done")).unwrap();
        sync();
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
    fn token(checks: &Checks) -> Option<u64> {
        let names: Vec<String> = fs::read_dir(path("tok"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        match &names[..] {
            [name] => name.strip_prefix("token.").and_then(|n| n.parse().ok()),
            names => {
                checks.fail(format!("the token's directory listed {names:?}"));
                None
            }
        }
    }

    fn tokens(checks: Arc<Checks>) {
        let mut last = 0;
        while checks.running() {
            if let Some(n) = token(&checks) {
                if n < last {
                    checks.fail(format!("the token went back from {last} to {n}"));
                }
                last = n;
            }
        }
    }

    /// How many mapped reads of a block in a row may find it torn before it fails: a read of a
    /// mapping copies no one version of a page, and one racing the invalidation that replaces the
    /// page reads part of each. A page left torn stays so.
    const TORN_READS: usize = 8;

    /// Reads the blocks, through read(2) or the mapping `mapped`: each whole, its generation
    /// never going back. A read(2) copies one page, which a follower replaces but never changes,
    /// so it is never torn; a mapped read may be, as the page is replaced, and is read again.
    fn blocks(checks: Arc<Checks>, mapped: Option<&'static [u8]>) {
        let file = fs::File::open(path("blocks")).unwrap();
        let mut last = vec![0u64; BLOCKS];
        let mut buf = vec![0u8; BLOCK];
        while checks.running() {
            for (index, last) in last.iter_mut().enumerate() {
                let mut reads = 0;
                let found = loop {
                    match mapped {
                        Some(mapped) => {
                            buf.copy_from_slice(&mapped[index * BLOCK..(index + 1) * BLOCK])
                        }
                        None => file.read_exact_at(&mut buf, (index * BLOCK) as u64).unwrap(),
                    }
                    reads += 1;
                    match read_page(&buf) {
                        None if mapped.is_some() && reads < TORN_READS => continue,
                        found => break found,
                    }
                };
                match found {
                    Some((generation, at)) if at == index as u64 => {
                        if generation < *last {
                            checks.fail(format!(
                                "block {index} went back from generation {last} to {generation}"
                            ));
                        }
                        *last = generation;
                    }
                    found => checks.fail(format!(
                        "block {index} is torn ({found:?}, {}, {reads} reads): {}",
                        if mapped.is_some() { "mapped" } else { "read" },
                        describe(&buf)
                    )),
                }
            }
        }
    }

    /// Reads the shaped file's pages through a mapping of it, as the kernel reads them for this
    /// process (/proc/self/mem): a page past its end, truncated meanwhile, fails that read alone.
    fn shapes(checks: Arc<Checks>) {
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
                if mem.read_exact_at(&mut buf, at).is_err() {
                    continue;
                }
                // Zeros: a page extended over, or not rewritten since.
                if buf.iter().all(|byte| *byte == 0) {
                    continue;
                }
                if !matches!(read_page(&buf), Some((_, index)) if index == page) {
                    checks.fail(format!("the shaped file's page {page} is torn"));
                }
            }
        }
        // SAFETY: The mapping made above.
        unsafe { libc::munmap(addr, len) };
    }

    /// Opens victims as they appear, and reads each whole once the writer deletes it.
    fn victims(checks: Arc<Checks>) {
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
                let mut kept = Vec::new();
                file.read_to_end(&mut kept).unwrap();
                if kept != victim(n) {
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

    /// Whether the follower holds what the writer left: through listings, read(2) and `mapped`.
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
        Ok(())
    }

    fn follower(role: &str) {
        let checks = Arc::new(Checks { failures: Mutex::default(), stop: AtomicBool::new(false) });
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
        let readers: Vec<std::thread::JoinHandle<()>> = vec![
            {
                let c = checks.clone();
                std::thread::spawn(move || tokens(c))
            },
            {
                let c = checks.clone();
                std::thread::spawn(move || blocks(c, None))
            },
            {
                let c = checks.clone();
                std::thread::spawn(move || blocks(c, Some(mapped)))
            },
            {
                let c = checks.clone();
                std::thread::spawn(move || shapes(c))
            },
            {
                let c = checks.clone();
                std::thread::spawn(move || victims(c))
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

    /// Says where a VM is stuck, if it runs on past what the test gives it: its blocked tasks' and
    /// its CPUs' stacks (sysrq w and l), and the kernel's log, to standard error, once. A VM that
    /// hangs says nothing else before the host gives up on it.
    fn start_watchdog() {
        let after = Duration::from_secs(seconds()) + CONVERGES_WITHIN + Duration::from_secs(30);
        std::thread::spawn(move || {
            std::thread::sleep(after);
            eprintln!("watchdog: still running after {after:?}");
            for key in [b"w", b"l"] {
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
