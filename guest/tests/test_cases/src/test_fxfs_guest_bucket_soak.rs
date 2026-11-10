// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: guests writing all the while against a degraded bucket (TASK-330.3): the case's daemon,
//! its own, keeps its stores' branches in a file bucket in the case's directory, which degrades as
//! the case says, through a file of faults (`FXDAX_REMOTE_FAULTS`, `extent_remote::faults`):
//! answering, then slow (300 ms a request), then full (its writes failing with `ENOSPC`), then out
//! of service (every request failing, 503), then answering again, 300 ms a request, each a
//! fraction of FXDAX_BUCKET_SOAK_SECS (1,800: the qualification's; 300 for a short run).
//! FXDAX_BUCKET_SOAK_VMS
//! VMs (4), each on a store of its own, write files of 16 MiB that don't compress and fsync each,
//! deleting each once 16 newer are written, at the rate that writes past every store's lag bound
//! while the bucket fails, until the run's end; then sync, every commit uploaded, and exit. As
//! `fxdax::extentd::tests::soak` does with VM processes of its own in CI, in seconds a phase, this
//! does with guests' kernels and their Fxfs, the daemon a process of its own. It fails unless:
//!
//! - the daemon's anonymous memory stays within its budget, every sample of it (its stats'
//!   `process:` line, `over-budget`), its upload memory set to 1.5 GiB, as much as its uploads hold
//!   at once (TASK-317);
//! - each failing phase's failures show in its stats line (`failed-NAME`, TASK-314);
//! - the guests' commits wait at the lag bound, and none fails: each guest's fsyncs, and its sync
//!   at the end, succeed, the longest waiting through the bucket's failure;
//! - every commit made before the bucket answers again is uploaded within doc-12's bound of it
//!   answering: its next retry's 5 s, the upload interval's second, and the time the guests took
//!   to write what waited (the control API's `SyncStore` of each store, from then);
//! - no VM is let go (TASK-249): each runs to its end, exiting 0, and the daemon's log lets none go.
//!
//! scripts/budgets.py checks the daemon's stats file beside its log too, as every run's daemons'.

use macros::{guest, host};

pub struct TestFxfsGuestBucketSoak;

const MIB: u64 = 1 << 20;
/// Each store's capacity: the files the guest keeps, and what waits for the bucket, with room.
#[cfg_attr(feature = "guest", allow(dead_code))]
const CAPACITY: u64 = 4096 * MIB;
/// A file's bytes.
#[cfg_attr(feature = "host", allow(dead_code))]
const FILE: usize = 16 << 20;
/// The files a guest keeps: each goes once this many newer are written.
#[cfg_attr(feature = "host", allow(dead_code))]
const KEPT: u64 = 16;

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::{Context, ensure};
    use std::ffi::CString;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime};

    /// The daemon's memory, its extent cache's: the guests' files' worth and more.
    const DAEMON_MEMORY: u64 = 2 << 30;
    /// Its upload memory: as much as its stores upload at once, so that its uploads holding each
    /// object twice, TASK-317's bug, hold more than every other part of its budget may, as
    /// `fxdax::extentd::tests::soak`'s.
    const UPLOAD_MEMORY: u64 = 1536 << 20;
    /// What the guests write while the bucket fails: past every store's lag bound, 1 GiB, so that
    /// their commits stall there, and past the host's upload memory.
    const WRITTEN_WHILE_FAILING: u64 = 6 << 30;
    const VM_MEMORY_MIB: u32 = 1024;

    /// The phases, each a fraction of the run, and what the bucket does through it, as
    /// `extent_remote::faults::Fault`'s words say it, answering again as slowly as before it
    /// failed, each PUT of what waited holding its object as long; and whether it fails.
    const PHASES: [(&str, f64, &str, bool); 5] = [
        ("answering", 0.1, "latency-ms=0 failing=not", false),
        ("slow", 0.2, "latency-ms=300 failing=not", false),
        ("full", 0.3, "latency-ms=0 failing=no-space", true),
        ("out of service", 0.1, "latency-ms=0 failing=unavailable", true),
        ("answering again", 0.3, "latency-ms=300 failing=not", false),
    ];

    /// `var`, a count, or `default`.
    fn count(var: &str, default: u64) -> anyhow::Result<u64> {
        match std::env::var(var) {
            Ok(value) => value.parse().with_context(|| format!("{var} is a count, not {value:?}")),
            Err(_) => Ok(default),
        }
    }

    /// The fields of the line of `kind` of the daemon's stats file at `path`, by name.
    fn stats_line(path: &Path, kind: &str) -> std::collections::BTreeMap<String, u64> {
        let kept = std::fs::read_to_string(path).unwrap_or_default();
        let prefix = format!("{kind}: ");
        let line = kept.lines().find_map(|line| line.strip_prefix(&prefix)).unwrap_or_default();
        (line.split(' '))
            .filter_map(|field| {
                let (name, value) = field.split_once('=')?;
                Some((name.to_owned(), value.parse().ok()?))
            })
            .collect()
    }

    /// The bucket's failures, of every kind, as the stats file says them (`failed-NAME`).
    fn failures(stats: &Path) -> u64 {
        let line = stats_line(stats, "object-store");
        line.iter().filter(|(name, _)| name.starts_with("failed-")).map(|(_, count)| count).sum()
    }

    /// Boots VM `vm` on store `store`, served by the daemon at `socket`, writing at `rate` bytes a
    /// second for `secs` seconds.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        socket: &Path,
        store: &Path,
        (vm, rate, secs): (u64, u64, u64),
    ) -> anyhow::Result<Firecracker> {
        let name = format!("vm-{vm}");
        let mut machine = Vm::new(2, VM_MEMORY_MIB)?;
        machine.guest_kernel()?;
        machine.extent_store_served("extents", store, CAPACITY, 0, socket);
        for (var, value) in [
            ("FXDAX_BUCKET_SOAK_SEED", vm),
            ("FXDAX_BUCKET_SOAK_RATE", rate),
            ("FXDAX_BUCKET_SOAK_SECS", secs),
        ] {
            machine.env(&CString::new(format!("{var}={value}"))?);
        }
        machine.root_image(&format!("{name}.rootfs.ext4"));
        machine.spawn_api(root, &test_setup.test_case, &name, &Vm::api_socket(&name))
    }

    impl TestFxfsGuestBucketSoak {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let secs = count("FXDAX_BUCKET_SOAK_SECS", 1800)?;
            let vms = count("FXDAX_BUCKET_SOAK_VMS", 4)?;
            let run = Duration::from_secs(secs);
            let root = setup_rootfs(test_setup)?;
            let dir = &test_setup.tmp_dir;
            let (faults, stats) = (dir.join("bucket-faults"), dir.join("extentd-stats.txt"));
            std::fs::write(&faults, "")?;
            // Short, as a Unix socket's path is.
            let sockets = tempdir::TempDir::new("fxdax-bucket-soak")?;
            let socket = sockets.path().join("extentd.sock");
            let upload_memory = UPLOAD_MEMORY.to_string();
            let env = [
                ("FXDAX_REMOTE_FAULTS", faults.as_os_str()),
                ("FXDAX_EXTENTD_UPLOAD_MEMORY", upload_memory.as_ref()),
            ];
            let mut daemon = crate::common::spawn_daemon(
                test_setup,
                &socket,
                "extentd",
                &env,
                Some(DAEMON_MEMORY),
            )?;
            let failing: f64 = PHASES.iter().filter(|phase| phase.3).map(|phase| phase.1).sum();
            let rate = (WRITTEN_WHILE_FAILING as f64 / (vms as f64 * failing * secs as f64)) as u64;
            eprintln!("bucket-soak: {secs} s, {vms} VMs writing {} MiB/s each", rate >> 20);
            let stores: Vec<_> = (0..vms).map(|vm| dir.join(format!("store-{vm}"))).collect();
            let mut running = Vec::new();
            for (vm, store) in stores.iter().enumerate() {
                let vm = vm as u64;
                running.push(boot(test_setup, &root, &socket, store, (vm, rate, secs))?);
            }
            let began = Instant::now();
            let mut failed_before = 0;
            let mut syncs = Vec::new();
            for (name, fraction, words, fails) in PHASES {
                std::fs::write(&faults, words)?;
                eprintln!("bucket-soak: at {:?}, the bucket {name}: {words}", began.elapsed());
                if name == "answering again" {
                    // Every store's commits made so far uploaded, each sync says, within the bound.
                    let since = (Instant::now(), SystemTime::now());
                    for store in &stores {
                        let (socket, store) = (socket.clone(), store.clone());
                        syncs.push(std::thread::spawn(move || {
                            let synced = crate::common::control(&socket)?.sync_store(&store);
                            synced.with_context(|| format!("syncing {}", store.display()))?;
                            anyhow::Ok(since.0.elapsed())
                        }));
                    }
                }
                let ends = Instant::now() + run.mul_f64(fraction);
                while Instant::now() < ends {
                    std::thread::sleep(Duration::from_secs(1));
                    let process = stats_line(&stats, "process");
                    ensure!(
                        process.get("over-budget") == Some(&0),
                        "the daemon's anonymous memory past its budget: {process:?}"
                    );
                    for (vm, firecracker) in running.iter_mut().enumerate() {
                        ensure!(firecracker.running()?, "VM {vm} ended before the run did");
                    }
                }
                let failed = failures(&stats);
                if fails {
                    ensure!(failed > failed_before, "the bucket {name}, and no failure said");
                }
                failed_before = failed;
                let process = stats_line(&stats, "process");
                eprintln!(
                    "bucket-soak: {name}: {failed} failures said, anonymous memory {} MiB at most, \
                     its budget {} MiB",
                    process.get("max-anon").copied().unwrap_or(0) >> 20,
                    process.get("anon-budget").copied().unwrap_or(0) >> 20,
                );
            }
            // Within doc-12's bound of the bucket answering again.
            let bound = Duration::from_secs(6)
                + Duration::from_secs_f64(WRITTEN_WHILE_FAILING as f64 / (vms * rate) as f64);
            for (vm, synced) in syncs.into_iter().enumerate() {
                let took = synced.join().unwrap()?;
                eprintln!("bucket-soak: store {vm} uploaded {took:?} after the bucket answered");
                ensure!(took <= bound, "store {vm} uploaded {took:?} after, past {bound:?}");
            }
            for (vm, firecracker) in running.into_iter().enumerate() {
                let (status, console) = firecracker.wait()?;
                let said = crate::vm::without_kernel_lines(&crate::vm::guest_stdout(
                    &std::fs::read(&console)?,
                ));
                ensure!(status.success(), "VM {vm} exited {status}, saying {said:?}");
                let wrote = said.lines().find_map(|line| line.strip_prefix("OK "));
                eprintln!("bucket-soak: VM {vm}: {}", wrote.context("said nothing of its writes")?);
                let image = dir.join(format!("vm-{vm}.rootfs.ext4"));
                std::fs::remove_file(&image)?;
            }
            // Its stats file, written within a second of the last change.
            std::thread::sleep(Duration::from_secs(2));
            let log = std::fs::read_to_string(dir.join("extentd.log"))?;
            let let_go = |line: &&str| line.contains("letting a VM process go");
            let let_go: Vec<&str> = log.lines().filter(let_go).collect();
            ensure!(let_go.is_empty(), "the daemon let VM processes go: {let_go:#?}");
            crate::common::say_daemon_stats(test_setup, "extentd");
            daemon.kill()?;
            daemon.wait()?;
            Ok(())
        }
    }

    impl Test for TestFxfsGuestBucketSoak {
        fn should_run(&self) -> ShouldRun {
            if std::env::var_os("FXDAX_EXTENTD_BIN").is_none() {
                return ShouldRun::No("FXDAX_EXTENTD_BIN does not name the extent daemon");
            }
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            count("FXDAX_BUCKET_SOAK_SECS", 1800).unwrap_or(1800) + 600
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, _stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            TestOutcome::Pass
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";

    /// `var`, a count, or `default`.
    fn count(var: &str, default: u64) -> u64 {
        std::env::var(var).map_or(default, |value| value.parse().unwrap())
    }

    /// Bytes that don't compress, from `seed`: xorshift64*'s.
    fn fill(seed: u64, buf: &mut [u8]) {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        for word in buf.chunks_mut(8) {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let bytes = state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes();
            word.copy_from_slice(&bytes[..word.len()]);
        }
    }

    impl Test for TestFxfsGuestBucketSoak {
        fn in_guest(self: Box<Self>) {
            let seed = count("FXDAX_BUCKET_SOAK_SEED", 0) << 32;
            let rate = count("FXDAX_BUCKET_SOAK_RATE", 4 * MIB);
            let secs = count("FXDAX_BUCKET_SOAK_SECS", 1800);
            fs::create_dir_all(MNT).unwrap();
            let (source, target, fstype) = (
                CString::new("extents").unwrap(),
                CString::new(MNT).unwrap(),
                CString::new("fxfs").unwrap(),
            );
            let ret = unsafe {
                libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
            };
            assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
            let started = Instant::now();
            let run = Duration::from_secs(secs);
            let mut data = vec![0; FILE];
            let (mut files, mut longest) = (0, Duration::ZERO);
            while started.elapsed() < run {
                fill(seed + files, &mut data);
                let mut file = fs::File::create(format!("{MNT}/{files}")).unwrap();
                file.write_all(&data).unwrap();
                let syncing = Instant::now();
                file.sync_all().unwrap();
                longest = longest.max(syncing.elapsed());
                files += 1;
                if files > KEPT {
                    fs::remove_file(format!("{MNT}/{}", files - KEPT - 1)).unwrap();
                }
                // At its rate, from its start: an fsync that waited writes on at once after.
                let due = Duration::from_secs_f64((files * FILE as u64) as f64 / rate as f64);
                if let Some(ahead) = due.checked_sub(started.elapsed()) {
                    std::thread::sleep(ahead.min(run.saturating_sub(started.elapsed())));
                }
            }
            // Every commit uploaded, as at its host's stop (`COMMIT_DURABLE`).
            let dir = fs::File::open(MNT).unwrap();
            let ret = unsafe { libc::syncfs(std::os::fd::AsRawFd::as_raw_fd(&dir)) };
            assert_eq!(ret, 0, "syncfs: {}", std::io::Error::last_os_error());
            drop(dir);
            let ret = unsafe { libc::umount(target.as_ptr()) };
            assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
            println!(
                "OK wrote {} MiB in {files} files, the longest fsync {} ms",
                (files * FILE as u64) >> 20,
                longest.as_millis()
            );
        }
    }
}
