// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what extentd asks of a disk it shares with many VMs, and with the bucket (TASK-310): a
//! sandbox host's CI, whose daemon wrote some 2.3 times what its guests wrote, its framings stalled
//! for seconds behind its own write-back. The case's daemon, its own, keeps its stores, its file
//! bucket and its part cache in the case's directory, one disk, as such a host's daemon keeps its
//! stores, and its bucket server its objects, on one. VMs boot FXDAX_SHARED_DISK_AT_ONCE at a time
//! (8), FXDAX_SHARED_DISK_VMS in all (16), each on a new store; each guest writes a large file,
//! FXDAX_SHARED_DISK_LARGE_MIB (768), of bytes that don't compress, and fsyncs it; then
//! FXDAX_SHARED_DISK_FILES files (3,000) of 8 KiB, synced at once (syncfs), or each fsynced with
//! FXDAX_SHARED_DISK_FSYNC=1, commits small enough for their records to hold their data; reads them back, its
//! caches dropped, checking each; deletes them, and syncs again. With a DAX window or without.
//!
//! The host says, as `shared-disk:` lines on its standard error, the case's log: what the guests
//! wrote; what the daemon wrote, as its kernel counts it (/proc/PID/io's `write_bytes`, and of
//! those `cancelled_write_bytes`, gone before written back), and by kind, as its stats say
//! (`written_*_bytes`); its framings past extentd's turn, which its log warns of, how many, the
//! longest and their sum; the time the host's tasks stalled on I/O (/proc/pressure/io's `some`
//! and `full`); and the case's disk's writes and busy time (its block device's stat). A
//! measurement: it fails only if a VM does, or its daemon's stores lose half their commits'
//! durability within 50 ms, doc-12's median.

use macros::{guest, host};

pub struct TestFxfsGuestSharedDisk {
    pub dax: bool,
}

const MIB: u64 = 1 << 20;
/// Each store's capacity: the large file and the small files, with room.
#[cfg_attr(feature = "guest", allow(dead_code))]
const CAPACITY: u64 = 4096 * MIB;
/// The daemon's memory, with DAX windows: room for the private pages of the windows of the VMs at once, each half
/// its window's by default, which past the daemon's default, 8 GiB, refused the eighth's window
/// (ENOMEM).
#[cfg(feature = "host")]
const DAX_DAEMON_MEMORY: u64 = 20 << 30;
/// Without: its memory, and its VMs', what a host shared with other loads holds at once, eight
/// VMs at a time: with 1 GiB each and the daemon's 8 GiB, the host's OOM killer took the daemon.
#[cfg(feature = "host")]
const DAEMON_MEMORY: u64 = 4 << 30;
#[cfg(feature = "host")]
const VM_MEMORY_MIB: u32 = 768;
/// A small file's bytes.
#[cfg_attr(feature = "host", allow(dead_code))]
const SMALL: usize = 8 << 10;

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::{Context, ensure};
    use std::ffi::CString;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// `var`, a count, or `default`.
    fn count(var: &str, default: u64) -> anyhow::Result<u64> {
        match std::env::var(var) {
            Ok(value) => value.parse().with_context(|| format!("{var} is a count, not {value:?}")),
            Err(_) => Ok(default),
        }
    }

    /// A process's /proc/PID/io, each field by its name.
    fn process_io(pid: u32) -> anyhow::Result<std::collections::BTreeMap<String, u64>> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/io"))?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let (name, value) = line.split_once(": ")?;
                Some((name.to_owned(), value.trim().parse().ok()?))
            })
            .collect())
    }

    /// The microseconds the host's tasks stalled on I/O so far: /proc/pressure/io's `some` and
    /// `full` totals.
    fn io_pressure() -> anyhow::Result<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/pressure/io")?;
        let total = |kind: &str| -> anyhow::Result<u64> {
            let line = text.lines().find(|line| line.starts_with(kind)).context(kind.to_owned())?;
            let total = line.split(' ').find_map(|field| field.strip_prefix("total="));
            Ok(total.context("total")?.parse()?)
        };
        Ok((total("some")?, total("full")?))
    }

    /// The block device `dir` is on, its stat: sectors written (the 7th field) and the
    /// milliseconds it was busy (the 10th), or none if `dir`'s device has no block device's stat
    /// (tmpfs, ZFS).
    fn disk_stat(dir: &Path) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt as _;
        let dev = std::fs::metadata(dir).ok()?.dev();
        let (major, minor) = (nix::libc::major(dev), nix::libc::minor(dev));
        let stat = std::fs::read_to_string(format!("/sys/dev/block/{major}:{minor}/stat")).ok()?;
        let fields: Vec<u64> = stat.split_whitespace().filter_map(|f| f.parse().ok()).collect();
        Some((*fields.get(6)?, *fields.get(9)?))
    }

    /// The daemon's framings past extentd's turn, as its log `log` warns of them: how many, the
    /// longest's milliseconds, and the milliseconds of all.
    fn slow_framings(log: &str) -> (u64, f64, f64) {
        let took = log.lines().filter_map(|line| {
            let (_, after) = line.split_once("extent store: framing ")?;
            let (_, after) = after.split_once(" took ")?;
            after.split_once(" ms")?.0.parse::<f64>().ok()
        });
        took.fold((0, 0.0, 0.0), |(count, max, sum), ms| (count + 1, max.max(ms), sum + ms))
    }

    /// The `daemon:` line of the daemon's stats file at `path`, its fields by name.
    fn daemon_stats(path: &Path) -> anyhow::Result<std::collections::BTreeMap<String, u64>> {
        let kept = std::fs::read_to_string(path)?;
        let line = kept.lines().find_map(|line| line.strip_prefix("daemon: "));
        Ok((line.context("no daemon line")?.split(' '))
            .filter_map(|field| {
                let (name, value) = field.split_once('=')?;
                Some((name.to_owned(), value.parse().ok()?))
            })
            .collect())
    }

    /// Boots VM `vm` on a new store of its own, served by the daemon at `socket`.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        socket: &Path,
        window: u64,
        vm: u64,
    ) -> anyhow::Result<Firecracker> {
        let name = format!("vm-{vm}");
        let store = test_setup.tmp_dir.join(format!("store-{vm}"));
        let mut machine = Vm::new(2, VM_MEMORY_MIB)?;
        machine.guest_kernel()?;
        machine.extent_store_served("extents", &store, CAPACITY, window, socket);
        for var in
            ["FXDAX_SHARED_DISK_LARGE_MIB", "FXDAX_SHARED_DISK_FILES", "FXDAX_SHARED_DISK_FSYNC"]
        {
            if let Ok(value) = std::env::var(var) {
                machine.env(&CString::new(format!("{var}={value}"))?);
            }
        }
        machine.env(&CString::new(format!("FXDAX_SHARED_DISK_SEED={vm}"))?);
        machine.root_image(&format!("{name}.rootfs.ext4"));
        machine.spawn_api(root, &test_setup.test_case, &name, &Vm::api_socket(&name))
    }

    /// What the VM `firecracker` said it wrote, in bytes, once it ended, and whether its kernel
    /// complained (a soft lockup, its host's disk stalling it): said, a measurement, not failed.
    /// It failed otherwise.
    fn finish(
        firecracker: Firecracker,
        test_setup: &TestSetup,
        vm: u64,
    ) -> anyhow::Result<(u64, bool)> {
        let (status, console) = firecracker.wait()?;
        let said =
            crate::vm::without_kernel_lines(&crate::vm::guest_stdout(&std::fs::read(&console)?));
        let image = test_setup.tmp_dir.join(format!("vm-{vm}.rootfs.ext4"));
        std::fs::remove_file(&image).with_context(|| format!("removing {}", image.display()))?;
        let complained = match crate::vm::complaints(&console) {
            Ok(()) => false,
            Err(err) => {
                eprintln!("shared-disk: VM {vm}'s kernel complained: {err:#}");
                true
            }
        };
        ensure!(status.success(), "VM {vm} exited {status}, saying {said:?}");
        let wrote = said.lines().find_map(|line| line.strip_prefix("OK wrote "));
        let wrote = wrote.with_context(|| format!("VM {vm} said {said:?}"))?;
        Ok((wrote.parse()?, complained))
    }

    impl TestFxfsGuestSharedDisk {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let (vms, at_once) =
                (count("FXDAX_SHARED_DISK_VMS", 16)?, count("FXDAX_SHARED_DISK_AT_ONCE", 8)?);
            ensure!(at_once > 0, "FXDAX_SHARED_DISK_AT_ONCE is 0");
            let root = setup_rootfs(test_setup)?;
            let dir = &test_setup.tmp_dir;
            // Short, as a Unix socket's path is.
            let sockets = tempdir::TempDir::new("fxdax-shared-disk")?;
            let socket = sockets.path().join("extentd.sock");
            // Its bucket and part cache in the case's directory, on the stores' disk
            // (crate::common::spawn_daemon's defaults), unless this process names others.
            // As much as the windows' private pages take, with a window; the default without.
            let memory = Some(if self.dax { DAX_DAEMON_MEMORY } else { DAEMON_MEMORY });
            let mut daemon =
                crate::common::spawn_daemon(test_setup, &socket, "extentd", &[], memory)?;
            // A window the store's size, as its data is: eight at once of the host's memory each
            // were refused (ENOMEM).
            let window = if self.dax { CAPACITY } else { 0 };
            let (io, pressure, disk) = (process_io(daemon.id())?, io_pressure()?, disk_stat(dir));
            let started = Instant::now();
            let (mut running, mut guests, mut next, mut complaints) = (Vec::new(), 0, 0, 0);
            while next < vms || !running.is_empty() {
                while next < vms && (running.len() as u64) < at_once {
                    running.push((next, boot(test_setup, &root, &socket, window, next)?));
                    next += 1;
                }
                std::thread::sleep(Duration::from_millis(100));
                let mut still = Vec::new();
                for (vm, mut firecracker) in running {
                    match firecracker.running()? {
                        true => still.push((vm, firecracker)),
                        false => {
                            let (wrote, complained) = finish(firecracker, test_setup, vm)?;
                            guests += wrote;
                            complaints += u64::from(complained);
                        }
                    }
                }
                running = still;
            }
            let took = started.elapsed();
            let after = process_io(daemon.id())?;
            let (some, full) = io_pressure()?;
            // Its stats file, written within a poll of the last change.
            std::thread::sleep(Duration::from_secs(2));
            let stats = daemon_stats(&dir.join("extentd-stats.txt"))?;
            let log = std::fs::read_to_string(dir.join("extentd.log"))?;
            let (slow, longest, slow_sum) = slow_framings(&log);
            let gib = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
            let delta = |name: &str| {
                after.get(name).copied().unwrap_or(0) - io.get(name).copied().unwrap_or(0)
            };
            let written = delta("write_bytes");
            let cancelled = delta("cancelled_write_bytes");
            eprintln!(
                "shared-disk: vms={vms} at_once={at_once} dax={} secs={:.1} guest_gib={:.2} \
                 extentd_write_gib={:.2} extentd_cancelled_gib={:.2} ratio={:.2} \
                 net_ratio={:.2}",
                self.dax,
                took.as_secs_f64(),
                gib(guests),
                gib(written),
                gib(cancelled),
                written as f64 / guests as f64,
                (written - cancelled) as f64 / guests as f64,
            );
            // By kind, and the rest: the bucket's objects and the part cache, which the daemon
            // writes itself, its bucket a file bucket.
            let kinds = ["raw", "copied", "framed", "own", "log"].map(|kind| {
                (kind, stats.get(&format!("written_{kind}_bytes")).copied().unwrap_or(0))
            });
            let stored: u64 = kinds.iter().map(|(_, bytes)| bytes).sum();
            let kinds = kinds.map(|(kind, bytes)| format!("{kind}_gib={:.2}", gib(bytes)));
            eprintln!(
                "shared-disk: written {} other_gib={:.2} vms_complained={complaints}",
                kinds.join(" "),
                gib(written.saturating_sub(stored)),
            );
            // fxdax: segments whose data was written once, and commits spanning segments
            // (TASK-310, TASK-92.41).
            let once = ["segments_framed_in_place", "commits_spanning_segments"]
                .map(|name| format!("{name}={}", stats.get(name).copied().unwrap_or(0)));
            eprintln!("shared-disk: {}", once.join(" "));
            eprintln!(
                "shared-disk: slow_framings={slow} slowest_ms={longest:.1} slow_framing_ms={slow_sum:.0} \
                 framing_count={} flush_slow={} io_stall_some_s={:.1} io_stall_full_s={:.1}",
                stats.get("framing_count").copied().unwrap_or(0),
                stats.get("flush_slow").copied().unwrap_or(0),
                (some - pressure.0) as f64 / 1e6,
                (full - pressure.1) as f64 / 1e6,
            );
            let lag = [
                "durable_lag_count",
                "durable_lag_within_50ms",
                "durable_lag_p50_ms",
                "durable_lag_p99_ms",
                "durable_lag_max_us",
            ]
            .map(|name| format!("{name}={}", stats.get(name).copied().unwrap_or(0)));
            eprintln!("shared-disk: {}", lag.join(" "));
            match (disk, disk_stat(dir)) {
                (Some((sectors, busy)), Some((sectors_after, busy_after))) => eprintln!(
                    "shared-disk: disk_write_gib={:.2} disk_busy_s={:.1}",
                    gib((sectors_after - sectors) * 512),
                    (busy_after - busy) as f64 / 1000.0
                ),
                _ => eprintln!("shared-disk: {} is on no block device with a stat", dir.display()),
            }
            crate::common::say_daemon_stats(test_setup, "extentd");
            crate::common::check_durable_median(test_setup, "extentd")?;
            daemon.kill()?;
            daemon.wait()?;
            Ok(())
        }
    }

    impl Test for TestFxfsGuestSharedDisk {
        fn should_run(&self) -> ShouldRun {
            if std::env::var_os("FXDAX_EXTENTD_BIN").is_none() {
                return ShouldRun::No("FXDAX_EXTENTD_BIN does not name the extent daemon");
            }
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            900
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
    use std::io::{Read, Write};

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

    fn syncfs() {
        let dir = fs::File::open(MNT).unwrap();
        assert_eq!(unsafe { libc::syncfs(std::os::fd::AsRawFd::as_raw_fd(&dir)) }, 0, "syncfs");
    }

    impl Test for TestFxfsGuestSharedDisk {
        fn in_guest(self: Box<Self>) {
            let seed = count("FXDAX_SHARED_DISK_SEED", 0) << 32;
            let (large_mib, files) =
                (count("FXDAX_SHARED_DISK_LARGE_MIB", 768), count("FXDAX_SHARED_DISK_FILES", 3000));
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
            let mut chunk = vec![0; MIB as usize];
            // The large file, durably.
            let large = format!("{MNT}/large");
            let mut file = fs::File::create(&large).unwrap();
            for at in 0..large_mib {
                fill(seed + at, &mut chunk);
                file.write_all(&chunk).unwrap();
            }
            file.sync_all().unwrap();
            drop(file);
            // The small files, synced at once.
            let small = format!("{MNT}/small");
            fs::create_dir(&small).unwrap();
            let fsync_each = count("FXDAX_SHARED_DISK_FSYNC", 0) != 0;
            let mut bytes = vec![0; SMALL];
            for at in 0..files {
                fill(seed + large_mib + at, &mut bytes);
                let mut file = fs::File::create(format!("{small}/{at}")).unwrap();
                file.write_all(&bytes).unwrap();
                if fsync_each {
                    file.sync_all().unwrap();
                }
            }
            syncfs();
            // Read back, from the device, each checked.
            fs::write("/proc/sys/vm/drop_caches", b"3").unwrap();
            let mut file = fs::File::open(&large).unwrap();
            let mut read = vec![0; MIB as usize];
            for at in 0..large_mib {
                fill(seed + at, &mut chunk);
                file.read_exact(&mut read).unwrap();
                assert!(read == chunk, "the large file's MiB {at} reads otherwise");
            }
            drop(file);
            for at in 0..files {
                fill(seed + large_mib + at, &mut bytes);
                assert!(fs::read(format!("{small}/{at}")).unwrap() == bytes, "small file {at}");
            }
            // And gone.
            fs::remove_file(&large).unwrap();
            fs::remove_dir_all(&small).unwrap();
            syncfs();
            let ret = unsafe { libc::umount(target.as_ptr()) };
            assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
            println!("OK wrote {}", large_mib * MIB + files * SMALL as u64);
        }
    }
}
