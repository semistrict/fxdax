// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the case copy-on-write DAX is for (doc-13, TASK-164.5), the user's 400 GB file, scaled to
//! a GCE n2-standard-4 host: a file four times the guest's memory, in a base, mapped `MAP_SHARED`
//! read-write by a VM on a fork of it, read all over at random, then 5% of its pages written, at
//! random, and synced. A base VM writes the file, through its page cache; the host forks the base;
//! a VM on the fork, through a DAX window (fxfs-guest-bigmap-dax), its DAX writes copying on write
//! (fxfs-guest-bigmap-dax-cow, `fxfs.dax_cow=1`), or without a window (fxfs-guest-bigmap), maps the
//! file whole, and:
//!
//! 1. reads a word of each page once, in a random order (`read-cold`), then of pages at random for
//!    FXDAX_BIGMAP_READ_SECS (10, `read-warm`);
//! 2. stores a word to FXDAX_BIGMAP_WRITE_PERCENT (5) of its pages, in a random order, each store
//!    timed, the first to a page faulting (`written`);
//! 3. msyncs it (`synced`);
//! 4. checks each page's words through the mapping, and the written pages' through read(2)
//!    (`end`).
//!
//! The guest says each step's throughput, faults and stores' latencies on its console, as
//! `bigmap: NAME VALUE`, and asks the host to measure its memory and the host's after each
//! (`crate::measure`). scripts/pgbench-gate.py gates them (docs/measurements/dax-cow-gate.md).
//! FXDAX_BIGMAP_MIB sizes the file (4096), FXDAX_BIGMAP_MEMORY_MIB the VMs' memory (1024).

use macros::{guest, host};

pub struct TestFxfsGuestBigmap {
    /// Whether the VM mapping the file has a DAX window.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    /// Whether its DAX writes copy on write (`fxfs.dax_cow=1`).
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax_cow: bool,
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::measure::{HostMemory, LAST, Requests};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::{CString, OsStr};
    use std::path::Path;

    /// The environment's `name`, a count, or `default`.
    fn setting(name: &str, default: u64) -> anyhow::Result<u64> {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(value.parse()?),
            _ => Ok(default),
        }
    }

    /// The file's size, in bytes: FXDAX_BIGMAP_MIB MiB, or 4 GiB, four times the guest's memory.
    fn file_bytes() -> anyhow::Result<u64> {
        Ok(setting("FXDAX_BIGMAP_MIB", 4096)?.max(1) << 20)
    }

    /// The VMs' memory, in MiB: FXDAX_BIGMAP_MEMORY_MIB, or 1024.
    fn memory() -> anyhow::Result<u32> {
        Ok(u32::try_from(setting("FXDAX_BIGMAP_MEMORY_MIB", 1024)?)?)
    }

    /// A VM, its guest in role `role` (`base` or `map`), on the store at `store`, of `capacity`
    /// bytes, which the daemon at `extentd` serves, with a DAX window of `window` bytes if not 0.
    fn machine(
        role: &str,
        store: &Path,
        extentd: &Path,
        capacity: u64,
        window: u64,
    ) -> anyhow::Result<Vm> {
        let mut vm = Vm::new(2, memory()?)?;
        vm.guest_kernel()?;
        vm.extent_store_served("extents", store, capacity, window, extentd);
        vm.env(&CString::new(format!("FXDAX_BIGMAP_ROLE={role}"))?);
        vm.env(&CString::new(format!("FXDAX_BIGMAP_BYTES={}", file_bytes()?))?);
        for var in ["FXDAX_BIGMAP_READ_SECS", "FXDAX_BIGMAP_WRITE_PERCENT"] {
            if let Ok(value) = std::env::var(var) {
                vm.env(&CString::new(format!("{var}={value}"))?);
            }
        }
        Ok(vm)
    }

    /// Boots `vm`, named `name`, its API socket and files named after it.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        name: &str,
        vm: Vm,
    ) -> anyhow::Result<Firecracker> {
        vm.spawn_api(root, &test_setup.test_case, name, &Vm::api_socket(name))
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}");
        Ok(())
    }

    impl TestFxfsGuestBigmap {
        /// The base's VM writes the file; the VM on a fork of the base maps it, asking the host
        /// to measure after each step, until its last, a daemon of the case's own serving both.
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(test_setup)?;
            let dir = &test_setup.tmp_dir;
            let bytes = file_bytes()?;
            // Its socket's path short, as Unix sockets' must be; its metrics, which say what its
            // memory holds.
            let sockets = tempdir::TempDir::new("fxdax-bigmap")?;
            let socket = sockets.path().join("extentd.sock");
            let metrics = crate::measure::metrics_address()?;
            let metrics_at = metrics.to_string();
            let env = [("FXDAX_EXTENTD_METRICS", OsStr::new(&metrics_at))];
            // The file whole in its memory, as a host keeps a base's pages for its forks, and room.
            let memory = Some(bytes + (2 << 30));
            let daemon = crate::common::spawn_daemon(test_setup, &socket, "extentd", &env, memory)?;
            // The file, what the fork writes again of it, and room.
            let capacity = 2 * bytes + (4 << 30);
            let base = dir.join("base");
            wait(boot(test_setup, &root, "base", machine("base", &base, &socket, capacity, 0)?)?)?;
            let fork = dir.join("fork");
            crate::common::control(&socket)?.fork_store(&base, &fork)?;
            // Room for the file twice over, which window reclaim then needn't take back.
            let window = if self.dax { 2 * bytes } else { 0 };
            let mut machine = machine("map", &fork, &socket, capacity, window)?;
            if self.dax_cow {
                crate::measure::ask_dax_cow(&mut machine)?;
            }
            let requests = Requests::new("map")?;
            machine.vsock(requests.uds());
            let host = HostMemory::new(daemon.id(), metrics);
            host.say(0, "base", None, "")?;
            let mut firecracker = boot(test_setup, &root, "map", machine)?;
            loop {
                let request = requests.next(&mut firecracker)?;
                host.say(1, &request.phase, Some(firecracker.pid()), &request.guest)?;
                let last = request.phase == LAST;
                request.answer()?;
                if last {
                    break;
                }
            }
            wait(firecracker)
        }
    }

    impl Test for TestFxfsGuestBigmap {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            3600
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8_lossy(&stdout);
            let expected = "OK base\nOK checked\n";
            if output != expected {
                return TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"));
            }
            TestOutcome::Pass
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use crate::measure::{LAST, measure};
    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";
    const FILE: &str = "/mnt/fxfs/bigmap";
    const PAGE: usize = 4096;
    /// The words of a page.
    const WORDS: usize = PAGE / 8;
    const MIB: f64 = (1 << 20) as f64;

    /// The word `word` of page `page` as the base writes it.
    fn based(page: usize, word: usize) -> u64 {
        ((page as u64) << 16) | word as u64
    }

    /// The second word of page `page` as the fork writes it.
    fn written(page: usize) -> u64 {
        !based(page, 1)
    }

    /// The environment's `name`, a count, or `default`.
    fn count(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => {
                value.parse().unwrap_or_else(|_| panic!("{name} is a count, not {value:?}"))
            }
            _ => default,
        }
    }

    fn mount() {
        fs::create_dir_all(MNT).unwrap();
        let (source, target, fstype) = (c"extents", CString::new(MNT).unwrap(), c"fxfs");
        // SAFETY: Each is a string, as the call asks.
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        // SAFETY: A string, as the call asks.
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// Says a result on the console, which scripts/pgbench-gate.py reads.
    fn say(name: &str, value: f64) {
        eprintln!("bigmap: {name} {value:.3}");
    }

    /// The page faults the guest took so far, and of them the major ones (/proc/vmstat).
    fn faults() -> (u64, u64) {
        let vmstat = fs::read_to_string("/proc/vmstat").unwrap();
        let count = |name: &str| {
            (vmstat.lines())
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')?.parse::<u64>().ok())
                .unwrap_or_else(|| panic!("/proc/vmstat has no {name}"))
        };
        (count("pgfault"), count("pgmajfault"))
    }

    /// Says the faults taken since `then`, as `faults()` had them, per page of `pages`, as
    /// `NAME_faults_per_page` and `NAME_major_faults_per_page`.
    fn say_faults(name: &str, then: (u64, u64), pages: usize) {
        let now = faults();
        say(&format!("{name}_faults_per_page"), (now.0 - then.0) as f64 / pages as f64);
        say(&format!("{name}_major_faults_per_page"), (now.1 - then.1) as f64 / pages as f64);
    }

    /// A pseudo-random generator, xorshift64*: the same sequence each run.
    struct Random(u64);

    impl Random {
        fn draw(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        /// A number below `n`.
        fn below(&mut self, n: usize) -> usize {
            (self.draw() % n as u64) as usize
        }

        /// The numbers below `n`, shuffled.
        fn shuffled(&mut self, n: usize) -> Vec<u32> {
            let mut numbers: Vec<u32> = (0..u32::try_from(n).expect("few pages")).collect();
            for at in (1..n).rev() {
                numbers.swap(at, self.below(at + 1));
            }
            numbers
        }
    }

    /// The file mapped whole, shared, read-write.
    struct Mapping {
        words: *mut u64,
        bytes: usize,
    }

    impl Mapping {
        fn new(file: &fs::File, bytes: usize) -> Self {
            // SAFETY: A new mapping of the file, which only this uses.
            let at = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };
            assert_ne!(at, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            // Read at random, as the page cache's readahead then knows.
            // SAFETY: The mapping just made.
            let advised = unsafe { libc::madvise(at, bytes, libc::MADV_RANDOM) };
            assert_eq!(advised, 0, "madvise: {}", std::io::Error::last_os_error());
            Self { words: at.cast(), bytes }
        }

        /// The word `word` of page `page`.
        fn read(&self, page: usize, word: usize) -> u64 {
            assert!((page * WORDS + word) * 8 < self.bytes);
            // SAFETY: Within the mapping, as asserted.
            unsafe { self.words.add(page * WORDS + word).read_volatile() }
        }

        /// Stores `value` at the word `word` of page `page`.
        fn write(&self, page: usize, word: usize, value: u64) {
            assert!((page * WORDS + word) * 8 < self.bytes);
            // SAFETY: Within the mapping, as asserted.
            unsafe { self.words.add(page * WORDS + word).write_volatile(value) }
        }

        /// Writes back what was stored, and waits for it (msync's MS_SYNC).
        fn sync(&self) {
            // SAFETY: The mapping.
            let synced = unsafe { libc::msync(self.words.cast(), self.bytes, libc::MS_SYNC) };
            assert_eq!(synced, 0, "msync: {}", std::io::Error::last_os_error());
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: The mapping, used no more.
            assert_eq!(unsafe { libc::munmap(self.words.cast(), self.bytes) }, 0, "munmap");
        }
    }

    /// The base's file, of `bytes`, each of its words `based`, written and synced.
    fn make_base(bytes: usize) {
        let file = fs::File::create(FILE).unwrap();
        let mut chunk = vec![0u8; 1 << 20];
        for at in (0..bytes).step_by(chunk.len()) {
            let first = at / PAGE;
            for (word, slot) in chunk.chunks_exact_mut(8).enumerate() {
                slot.copy_from_slice(&based(first + word / WORDS, word % WORDS).to_le_bytes());
            }
            let len = chunk.len().min(bytes - at);
            file.write_all_at(&chunk[..len], at as u64)
                .unwrap_or_else(|err| panic!("writing the file at {at}: {err}"));
        }
        file.sync_all().unwrap();
    }

    /// Maps the base's file of `bytes`, reads it all over, writes some of it, syncs it, and checks
    /// it, saying how each step went, and asking the host to measure after each.
    fn map(bytes: usize) {
        crate::measure::dax_cow();
        let pages = bytes / PAGE;
        let file = fs::OpenOptions::new().read(true).write(true).open(FILE).unwrap();
        assert_eq!(file.metadata().unwrap().len(), bytes as u64, "the base's file's size");
        say("file_bytes", bytes as f64);
        measure("mounted");
        let mapping = Mapping::new(&file, bytes);
        let mut random = Random(0x9e37_79b9_7f4a_7c15);

        // Each page once, at random: each read the page's first.
        let order = random.shuffled(pages);
        let (then, start) = (faults(), Instant::now());
        for &page in &order {
            let page = page as usize;
            assert_eq!(mapping.read(page, 0), based(page, 0), "page {page}, cold");
        }
        say("read_cold_mib_s", bytes as f64 / MIB / start.elapsed().as_secs_f64());
        say_faults("read_cold", then, pages);
        drop(order);
        measure("read-cold");

        // Pages at random, for a while.
        let secs = Duration::from_secs(count("FXDAX_BIGMAP_READ_SECS", 10).max(1));
        let (mut reads, start) = (0u64, Instant::now());
        while start.elapsed() < secs {
            for _ in 0..4096 {
                let page = random.below(pages);
                assert_eq!(mapping.read(page, 0), based(page, 0), "page {page}, warm");
            }
            reads += 4096;
        }
        let read = (reads * PAGE as u64) as f64;
        say("read_warm_mib_s", read / MIB / start.elapsed().as_secs_f64());
        measure("read-warm");

        // Some of the pages, at random, each store timed.
        let percent = count("FXDAX_BIGMAP_WRITE_PERCENT", 5).clamp(1, 100) as usize;
        let order = random.shuffled(pages);
        let chosen = &order[..(pages * percent / 100).max(1)];
        let mut is_written = vec![false; pages];
        let mut took = Vec::with_capacity(chosen.len());
        let (then, start) = (faults(), Instant::now());
        for &page in chosen {
            let page = page as usize;
            let store = Instant::now();
            mapping.write(page, 1, written(page));
            took.push(store.elapsed());
            is_written[page] = true;
        }
        say("write_pages", chosen.len() as f64);
        say("write_pages_s", chosen.len() as f64 / start.elapsed().as_secs_f64());
        say_faults("write", then, chosen.len());
        took.sort();
        let micros = |at: usize| took[at.min(took.len() - 1)].as_secs_f64() * 1e6;
        say("store_p50_us", micros(took.len() / 2));
        say("store_p99_us", micros(took.len() * 99 / 100));
        say("store_max_us", micros(took.len() - 1));
        measure("written");

        let start = Instant::now();
        mapping.sync();
        say("msync_s", start.elapsed().as_secs_f64());
        measure("synced");

        // Each page as the base wrote it, but for what this wrote.
        for (page, &was) in is_written.iter().enumerate() {
            assert_eq!(mapping.read(page, 0), based(page, 0), "page {page}'s first word");
            let second = if was { written(page) } else { based(page, 1) };
            assert_eq!(mapping.read(page, 1), second, "page {page}'s second word");
        }
        drop(mapping);
        // And through the filesystem, without the mapping.
        let mut word = [0u8; 8];
        for &page in chosen {
            let page = page as usize;
            file.read_exact_at(&mut word, (page * PAGE + 8) as u64).unwrap();
            assert_eq!(u64::from_le_bytes(word), written(page), "page {page} read");
        }
        measure(LAST);
    }

    impl Test for TestFxfsGuestBigmap {
        fn in_guest(self: Box<Self>) {
            let bytes = count("FXDAX_BIGMAP_BYTES", 0) as usize;
            assert!(bytes > 0 && bytes % PAGE == 0, "FXDAX_BIGMAP_BYTES is {bytes}");
            mount();
            match std::env::var("FXDAX_BIGMAP_ROLE").unwrap().as_str() {
                "base" => {
                    make_base(bytes);
                    unmount();
                    println!("OK base");
                }
                "map" => {
                    map(bytes);
                    unmount();
                    println!("OK checked");
                }
                role => panic!("no role {role}"),
            }
        }
    }
}
