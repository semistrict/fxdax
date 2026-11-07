// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The sandbox team's load (fxdax's TASK-57, TASK-62): PostgreSQL's pgbench makes its database, at
//! scale 50, on a fork of a base store, through a DAX window, its branches in a bucket 25 ms away,
//! as theirs was. It broke Fxfs twice: a kernel panic in a truncate (TASK-58), then the primary
//! key's parallel build reading back other data than its workers wrote (TASK-59). A base VM makes
//! the base's filesystem; the host forks it through a daemon of its own; a VM as theirs boots on
//! the fork, makes the database, and checks it: every account there once. Then pgbench runs on it,
//! as theirs did, where Fxfs's journal shut down on a write to a sealed extent (TASK-91.1). After
//! each run, the database is checked whole (its balances, amcheck, its pages' checksums), and
//! again once it has recovered from a stop as if it crashed (TASK-119.4). With
//! a DAX window (fxfs-guest-pgbench-dax) or without (fxfs-guest-pgbench); and without, its first
//! run probed (TASK-91.14): profiled, the guest's time by symbol and the callers of its hot
//! spinlock on the console (fxfs-guest-pgbench-profile; with DAX, fxfs-guest-pgbench-dax-profile,
//! TASK-91.16); or, as the sandbox team's CPU-bound run,
//! with synchronous_commit off, its locks' contention, on a kernel that counts it
//! (fxfs-guest-pgbench-locks, scripts/build-guest-kernel.sh's FXDAX_GUEST_KERNEL_LOCK_STAT). Or,
//! without DAX, with PostgreSQL's fsync off (fxfs-guest-pgbench-nofsync): the load as fast as it
//! goes when nothing waits for the store's commits, which the load with fsync on is measured
//! against on the same host (TASK-91.17). And on ext4 with DAX on writable virtio-pmem
//! (pgbench-ext4-dax-pmem), a copy of a base image of its own: a static DAX device, storage as
//! memory, which the sandbox team measure the DAX window against (TASK-131.5).
//!
//! fxdax: and for copy-on-write DAX (doc-13, TASK-164.5), on a database the base's VM makes, which
//! each VM runs pgbench on a fork of, its files read in, not created, which DAX maps, every first
//! store to a page a copy on the host (fxfs-guest-pgbench-forked, -dax-forked); and so, each VM
//! kept running once its runs are done, the host's memory measured as each is added
//! (`crate::measure`): what a VM more costs the host (fxfs-guest-pgbench-density, -dax-density).
//! FXDAX_PGBENCH_RUNS and FXDAX_PGBENCH_RUN_SECS set the runs, and
//! FXDAX_PGBENCH_DENSITY_MEMORY_MIB the density cases' VMs' memory (1024).

use macros::{guest, host};

pub struct TestFxfsGuestPgbench {
    #[cfg_attr(feature = "guest", allow(dead_code))]
    storage: Storage,
    #[cfg_attr(feature = "host", allow(dead_code))]
    probe: Probe,
    /// Whether PostgreSQL fsyncs, as it does by default.
    #[cfg_attr(feature = "host", allow(dead_code))]
    fsync: bool,
    #[cfg_attr(feature = "guest", allow(dead_code))]
    database: Database,
}

impl TestFxfsGuestPgbench {
    /// The case on `storage`: each VM making its database, PostgreSQL fsyncing, no run probed.
    pub fn on(storage: Storage) -> Self {
        let database = Database::Made;
        Self { storage, probe: Probe::None, fsync: true, database }
    }

    /// The case with pgbench's first run probed for `probe`.
    pub fn probed(self, probe: Probe) -> Self {
        Self { probe, ..self }
    }

    /// The case with PostgreSQL's fsync off.
    pub fn without_fsync(self) -> Self {
        Self { fsync: false, ..self }
    }

    /// fxdax: the case with its VMs' databases from `database` (TASK-164.5).
    pub fn with_database(self, database: Database) -> Self {
        Self { database, ..self }
    }
}

/// What the database is stored on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// The guest's Fxfs, on a fork of a base store a daemon of the case's own serves.
    Fxfs,
    /// The same, through a DAX window.
    FxfsDax,
    /// fxdax: ext4 with DAX on writable virtio-pmem, on a copy of a base image (TASK-131.5).
    Ext4DaxPmem,
}

/// What a run of pgbench's first is probed for (TASK-91.14).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    None,
    /// The guest's CPUs sampled, with their call chains.
    Profile,
    /// Its locks' contention, with synchronous_commit off, which the guest's kernel counts.
    Locks,
}

/// fxdax: where each VM's database comes from, and how the VMs run (TASK-164.5).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Database {
    /// Each VM makes its own, on a fork of an empty base, one VM after another.
    Made,
    /// The base's VM makes it; each VM runs pgbench on a fork of the base, one after another: on
    /// files read in, not created, which a DAX window maps.
    Forked,
    /// As `Forked`, but each VM, its runs done, keeps running until the last VM's are: the host's
    /// memory measured before the first and as each is done (`crate::measure`), what each VM more
    /// costs the host.
    Density,
}

/// pgbench's scale: 100,000 accounts a unit.
const SCALE: u64 = 50;

/// The pgbench runs a VM makes, FXDAX_PGBENCH_RUNS, or the sandbox team's five.
fn runs() -> usize {
    count("FXDAX_PGBENCH_RUNS", 5).max(1) as usize
}

/// Each run's seconds: FXDAX_PGBENCH_RUN_SECS, or the sandbox team's 60.
fn run_secs() -> u64 {
    count("FXDAX_PGBENCH_RUN_SECS", 60).max(1)
}

/// The environment's `name`, a count, or `default`, unset or empty.
fn count(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => {
            value.parse().unwrap_or_else(|_| panic!("{name} is a count, not {value:?}"))
        }
        _ => default,
    }
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::ffi::{CString, OsStr};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// The stores' capacity, and the fork's VM's DAX window if it has one, as the sandbox team's.
    const CAPACITY: u64 = 16 << 30;
    const WINDOW: u64 = 2 << 30;
    /// The ext4 images' size: the database's, its WAL's and the runs' history's, with room.
    const PMEM_SIZE: u64 = 8 << 30;

    /// The fork's VM's DAX window: FXDAX_PGBENCH_WINDOW_MIB's, or [`WINDOW`]. The sandbox team's
    /// runs decay more in 8 GiB than in 2.
    fn window() -> anyhow::Result<u64> {
        Ok(setting("FXDAX_PGBENCH_WINDOW_MIB", WINDOW >> 20)? << 20)
    }

    /// The VMs' vCPUs: FXDAX_PGBENCH_VCPUS, or 2.
    fn vcpus() -> anyhow::Result<u8> {
        Ok(u8::try_from(setting("FXDAX_PGBENCH_VCPUS", 2)?)?)
    }

    /// The loading VM's memory, in MiB: FXDAX_PGBENCH_MEMORY_MIB, or 4096. Less of it makes the
    /// guest short of memory as the database loads, where the sandbox team's 2 GiB DAX VM failed
    /// (TASK-99).
    fn load_memory() -> anyhow::Result<u32> {
        Ok(u32::try_from(setting("FXDAX_PGBENCH_MEMORY_MIB", 4096)?)?)
    }

    /// fxdax: the memory of a density case's VMs, in MiB: FXDAX_PGBENCH_DENSITY_MEMORY_MIB, or
    /// 1024, as many of them as a GCE n2-standard-4 host holds at once (TASK-164.5).
    fn density_memory() -> anyhow::Result<u32> {
        Ok(u32::try_from(setting("FXDAX_PGBENCH_DENSITY_MEMORY_MIB", 1024)?)?)
    }

    /// How many VMs run pgbench, one after another, each on a fork of the base of its own, all
    /// served by the one daemon: FXDAX_PGBENCH_VMS, or 1. The sandbox team's runs had one daemon
    /// serve many, whose memory reached its cap from the third on, where most of their crashes
    /// came (TASK-102, TASK-103).
    fn vms() -> usize {
        setting("FXDAX_PGBENCH_VMS", 1).map_or(1, |vms| vms.max(1) as usize)
    }

    /// The environment's `name`, a count, or `default`.
    fn setting(name: &str, default: u64) -> anyhow::Result<u64> {
        match std::env::var(name) {
            Ok(value) => Ok(value.parse()?),
            Err(_) => Ok(default),
        }
    }

    const CONTAINERFILE: &str = "\
FROM alpine:3.22
RUN apk add --no-cache postgresql postgresql-contrib perf e2fsprogs
";

    /// Where a VM's filesystem is.
    enum Device<'a> {
        /// The store at `store`, which the daemon at `extentd` serves, with a DAX window of
        /// `window` bytes if not 0.
        Store { store: &'a Path, extentd: &'a Path, window: u64 },
        /// The image at `image`, on writable virtio-pmem, which the guest mounts ext4 on, with DAX.
        Pmem(&'a Path),
    }

    /// A VM of `memory` MiB on `device`, its guest in role `role`: `base`, which makes the base's
    /// filesystem, and its database too with FXDAX_PGBENCH_BASE_DATABASE; `load`, which makes the
    /// database and runs pgbench on it; or `run`, which runs pgbench on the base's.
    fn machine(role: &str, device: Device, memory: u32) -> anyhow::Result<Vm> {
        let mut vm = Vm::new(vcpus()?, memory)?;
        vm.guest_kernel()?;
        match device {
            Device::Store { store, extentd, window } => {
                vm.extent_store_served("extents", store, CAPACITY, window, extentd);
            }
            Device::Pmem(image) => {
                let device = vm.pmem("pgbench", image)?;
                vm.env(&CString::new(format!("FXDAX_PGBENCH_PMEM={device}"))?);
            }
        }
        vm.env(&CString::new(format!("FXDAX_PGBENCH_ROLE={role}"))?);
        // Every run's commits not waiting for the journal: what's left is writes' own cost.
        if std::env::var_os("FXDAX_PGBENCH_ASYNC").is_some() {
            vm.env(c"FXDAX_PGBENCH_ASYNC=1");
        }
        // fxdax: the runs, as the host has them (TASK-164.5).
        for var in ["FXDAX_PGBENCH_RUNS", "FXDAX_PGBENCH_RUN_SECS"] {
            if let Ok(value) = std::env::var(var) {
                vm.env(&CString::new(format!("{var}={value}"))?);
            }
        }
        Ok(vm)
    }

    /// Boots `vm`, named `name`, in Firecracker: its API socket and files named after it, each
    /// VM's its own, as one's socket outlives it. fxdax: its root image too: a density case's VMs
    /// run at once, and each VM's boot made the one image anew under those running on it, whose
    /// guests' ext4 then failed its checksums (r632, TASK-164.5).
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        name: &str,
        mut vm: Vm,
    ) -> anyhow::Result<Firecracker> {
        vm.root_image(&format!("{name}.rootfs.ext4"));
        vm.spawn_api(root, &test_setup.test_case, name, &Vm::api_socket(name))
    }

    /// Samples, every 2 s until `stop`, what the daemon at `socket` did and has left to upload of
    /// the store at `store`: its stats file, `stats`, and the store's upload lag, appended to
    /// `samples` with the seconds since it started: to see where a run's time goes (TASK-91.2).
    /// And the CPU time of each thread of the VM's process, `vmm`, so far: what the window's
    /// changes cost the VMM, its 'extentd client' thread's system time; and KVM's counts of the
    /// VM's exits, faults and flushes, what they cost the guest (TASK-91.16).
    fn sample(
        socket: PathBuf,
        store: PathBuf,
        stats: PathBuf,
        samples: PathBuf,
        vmm: u32,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<anyhow::Result<()>> {
        std::thread::spawn(move || {
            use std::io::Write;
            let control = crate::common::control(&socket)?;
            let mut out = std::fs::File::create(samples)?;
            let started = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(2));
                let stat = control.stat_store(&store)?;
                writeln!(
                    out,
                    "t={} pending_upload_bytes={} oldest_pending_ms={}",
                    started.elapsed().as_secs(),
                    stat.pending_upload_bytes,
                    stat.oldest_pending_ms
                )?;
                out.write_all(&std::fs::read(&stats).unwrap_or_default())?;
                // Gone once the VM ends, which its last sample before then sums up.
                if let Some(threads) = thread_times(vmm) {
                    writeln!(out, "vmm-threads: {threads}")?;
                }
                if let Some(kvm) = kvm_stats(vmm) {
                    writeln!(out, "kvm: {kvm}")?;
                }
            }
            Ok(())
        })
    }

    /// fxdax: with FXDAX_HOST_GUEST_PROFILE=DELAY:SECONDS, profiles the guest from the host, as
    /// `perf kvm --guest` does, for SECONDS from DELAY seconds on, its symbols the guest kernel's
    /// vmlinux's (FXDAX_GUEST_KERNEL), where its text is as linked (KASLR off, TASK-131.6); the
    /// report, the guest's time by symbol, in `report`, which `check` fails unless it names
    /// Fxfs's own. perf records every guest until stopped with SIGINT, by `timeout`: given a
    /// command to run, it would pass that command its own options too.
    fn profile_guest(report: PathBuf) -> Option<std::thread::JoinHandle<anyhow::Result<()>>> {
        let asked = std::env::var("FXDAX_HOST_GUEST_PROFILE").ok()?;
        Some(std::thread::spawn(move || {
            let (delay, seconds) = asked.split_once(':').context("DELAY:SECONDS")?;
            let (delay, seconds): (u64, u64) = (delay.parse()?, seconds.parse()?);
            let vmlinux = std::env::var("FXDAX_GUEST_KERNEL").context("FXDAX_GUEST_KERNEL")?;
            let guest = format!("--guestvmlinux={vmlinux}");
            let data = report.with_extension("data");
            std::thread::sleep(Duration::from_secs(delay));
            let recorded = Command::new("timeout")
                .args(["--signal=INT", &seconds.to_string()])
                .args(["perf", "kvm", "--guest", &guest, "record", "-a", "-o"])
                .arg(&data)
                .output()?;
            anyhow::ensure!(
                data.exists(),
                "perf kvm record: {}: {}",
                recorded.status,
                String::from_utf8_lossy(&recorded.stderr)
            );
            let reported = Command::new("perf")
                .args(["kvm", "--guest", &guest, "report", "--stdio", "--sort", "sym", "-i"])
                .arg(&data)
                .args(["--percent-limit", "0.01"])
                .output()?;
            std::fs::remove_file(&data)?;
            anyhow::ensure!(
                reported.status.success(),
                "perf kvm report: {}: {}",
                reported.status,
                String::from_utf8_lossy(&reported.stderr)
            );
            let symbols = Command::new("nm").args(["-n", "--defined-only", &vmlinux]).output()?;
            anyhow::ensure!(symbols.status.success(), "nm {vmlinux}: {}", symbols.status);
            let text = symbolize(
                &String::from_utf8_lossy(&reported.stdout),
                &String::from_utf8_lossy(&symbols.stdout),
            );
            std::fs::write(&report, text)?;
            Ok(())
        }))
    }

    /// `report`'s guest samples (perf kvm report's `[g] 0xADDRESS` lines, which perf leaves
    /// unresolved, as it found no relocation of the guest's kernel to apply its vmlinux with),
    /// each given the symbol of `nm`'s, the guest kernel's vmlinux's symbols in address order, it
    /// falls in, as the kernel's text is where vmlinux links it; summed by symbol, most first, a
    /// line each, its percentage and name.
    fn symbolize(report: &str, nm: &str) -> String {
        let symbols: Vec<(u64, &str)> = (nm.lines())
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let address = u64::from_str_radix(fields.next()?, 16).ok()?;
                Some((address, fields.nth(1)?))
            })
            .collect();
        let mut by_symbol = std::collections::HashMap::<&str, f64>::new();
        for line in report.lines() {
            let mut fields = line.split_whitespace();
            let (Some(percent), Some("[g]"), Some(address)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Some(percent), Some(address)) = (
                percent.strip_suffix('%').and_then(|percent| percent.parse::<f64>().ok()),
                address.strip_prefix("0x").and_then(|hex| u64::from_str_radix(hex, 16).ok()),
            ) else {
                continue;
            };
            let at = symbols.partition_point(|&(start, _)| start <= address);
            let name = at.checked_sub(1).map_or("?", |at| symbols[at].1);
            *by_symbol.entry(name).or_default() += percent;
        }
        let mut sorted: Vec<_> = by_symbol.into_iter().collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
        sorted.iter().map(|(name, percent)| format!("{percent:6.2}% {name}\n")).collect()
    }

    /// KVM's counts so far for the VM of process `pid`, as its debugfs directory has them, each
    /// `name=N` separated by spaces, the names in order: its exits by reason, its page faults,
    /// its TLB flushes (TASK-91.16); or none without debugfs, or once the VM is gone.
    fn kvm_stats(pid: u32) -> Option<String> {
        let prefix = format!("{pid}-");
        let dir = std::fs::read_dir("/sys/kernel/debug/kvm")
            .ok()?
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))?;
        let mut stats = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(dir.path()).ok()?.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let Ok(count) = text.trim().parse::<u64>() else { continue };
            stats.insert(entry.file_name().to_string_lossy().into_owned(), count);
        }
        let stats: Vec<String> =
            stats.iter().map(|(name, count)| format!("{name}={count}")).collect();
        Some(stats.join(" "))
    }

    /// The user and system CPU seconds of process `pid`'s threads so far, summed by name, as
    /// `name user=U sys=S` separated by `; `, the names in order; or none once it is gone.
    fn thread_times(pid: u32) -> Option<String> {
        // SAFETY: sysconf has no preconditions.
        let ticks = unsafe { nix::libc::sysconf(nix::libc::_SC_CLK_TCK) } as f64;
        let mut times = std::collections::BTreeMap::<String, (u64, u64)>::new();
        for task in std::fs::read_dir(format!("/proc/{pid}/task")).ok()?.flatten() {
            let Ok(stat) = std::fs::read_to_string(task.path().join("stat")) else { continue };
            // `pid (comm) state ...`: the name may hold spaces and parentheses, the rest not.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
            // utime and stime, the stat's 14th and 15th fields; the state is its 3rd.
            let (Some(user), Some(sys)) = (fields.get(11), fields.get(12)) else { continue };
            let (Ok(user), Ok(sys)) = (user.parse::<u64>(), sys.parse::<u64>()) else { continue };
            let summed = times.entry(stat[open + 1..close].to_owned()).or_default();
            *summed = (summed.0 + user, summed.1 + sys);
        }
        let times: Vec<String> = times
            .iter()
            .map(|(name, (user, sys))| {
                format!("{name} user={:.2} sys={:.2}", *user as f64 / ticks, *sys as f64 / ticks)
            })
            .collect();
        Some(times.join("; "))
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}");
        Ok(())
    }

    /// What VM `vm` of those run one after another names its `what`: the first `what` itself, the
    /// next `what-1`, and so on.
    fn nth(what: &str, vm: usize) -> String {
        match vm {
            0 => what.to_owned(),
            vm => format!("{what}-{vm}"),
        }
    }

    /// The base's store, and the daemon serving it and its forks.
    struct Served<'a> {
        test_setup: &'a TestSetup,
        root: &'a Path,
        /// The daemon's VM socket.
        socket: PathBuf,
        /// Its stats file (FXDAX_EXTENTD_STATS).
        stats: PathBuf,
        base: PathBuf,
        /// The DAX window of the VMs on forks of the base, if not 0.
        window: u64,
    }

    /// A VM running on a fork of the base, and what samples it.
    struct Running {
        firecracker: Firecracker,
        stop: Arc<AtomicBool>,
        sampler: std::thread::JoinHandle<anyhow::Result<()>>,
        profiler: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    }

    impl Running {
        /// Waits for the VM to end, passing on what its guest said, then for what sampled it.
        fn finish(self) -> anyhow::Result<()> {
            let ended = wait(self.firecracker);
            self.stop.store(true, Ordering::Relaxed);
            self.sampler.join().expect("the sampler panicked")?;
            if let Some(profiler) = self.profiler {
                profiler.join().expect("the profiler panicked")?;
            }
            ended
        }
    }

    impl TestFxfsGuestPgbench {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(test_setup)?;
            match self.storage {
                Storage::Fxfs | Storage::FxfsDax => self.run_stores(test_setup, &root),
                Storage::Ext4DaxPmem => Self::run_images(test_setup, &root),
            }
        }

        /// The base's VM makes ext4 on an image of its own, then each VM loads and runs pgbench on
        /// a copy of it, made as it boots, as bench-fork-ext4-dax-pmem's forks are.
        fn run_images(test_setup: &TestSetup, root: &Path) -> anyhow::Result<()> {
            // Under `extents`, which what keeps a run's results leaves out (test-linux-host.sh).
            let images = test_setup.tmp_dir.join("extents");
            std::fs::create_dir_all(&images)?;
            let base = images.join("base.img");
            std::fs::File::create(&base)?.set_len(PMEM_SIZE)?;
            wait(boot(test_setup, root, "base", machine("base", Device::Pmem(&base), 512)?)?)?;
            for vm in 0..vms() {
                let image = images.join(format!("{}.img", nth("load", vm)));
                let status =
                    Command::new("cp").arg("--sparse=always").arg(&base).arg(&image).status()?;
                anyhow::ensure!(status.success(), "copying the base's image: {status}");
                let machine = machine("load", Device::Pmem(&image), load_memory()?)?;
                let loaded = wait(boot(test_setup, root, &nth("load", vm), machine)?);
                std::fs::remove_file(&image)?;
                loaded.with_context(|| format!("VM {} of {}", vm + 1, vms()))?;
            }
            Ok(())
        }

        /// The base's VM makes the guest's Fxfs on a store, and the database too if the VMs run on
        /// its; then each VM runs pgbench on a fork of it, making its database first if not, a
        /// daemon of the case's own serving them all.
        fn run_stores(&self, test_setup: &TestSetup, root: &Path) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            // A daemon of its own, whose bucket answers 25 ms late, as the sandbox team's RustFS
            // did, or as FXDAX_REMOTE_LATENCY_MS says; its socket's path short, as Unix sockets'
            // must be.
            let sockets = tempdir::TempDir::new("fxdax-pgbench")?;
            let socket = sockets.path().join("extentd.sock");
            let given = std::env::var_os("FXDAX_REMOTE_LATENCY_MS");
            let bucket = std::env::var_os("FXDAX_EXTENTD_REMOTE");
            let latency = added_latency(given.as_deref(), bucket.as_deref());
            let stats = dir.join("extentd-stats.txt");
            // fxdax: its metrics, which say what its memory holds (`crate::measure`).
            let metrics = crate::measure::metrics_address()?;
            let metrics_at = metrics.to_string();
            let env = [
                ("FXDAX_REMOTE_LATENCY_MS", latency),
                ("FXDAX_EXTENTD_STATS", stats.as_os_str()),
                ("FXDAX_EXTENTD_METRICS", OsStr::new(&metrics_at)),
            ];
            // As much memory as the sandbox team's daemon has: with its default, 1 GiB, the guest's
            // working set, a 2 GiB window's, didn't fit, and faults paged in and unpacked again
            // what was evicted, slower as the database grew.
            let memory = Some(8 << 30);
            let daemon = crate::common::spawn_daemon(test_setup, &socket, "extentd", &env, memory)?;
            let base = dir.join("base");
            let forked = self.database != Database::Made;
            // fxdax: the base's VM, making the database, as much memory as a VM loading it needs.
            let memory = if forked { load_memory()? } else { 512 };
            let device = Device::Store { store: &base, extentd: &socket, window: 0 };
            let mut machine = machine("base", device, memory)?;
            if forked {
                machine.env(c"FXDAX_PGBENCH_BASE_DATABASE=1");
            }
            wait(boot(test_setup, root, "base", machine)?)?;
            let window = if self.storage == Storage::FxfsDax { window()? } else { 0 };
            let served = Served { test_setup, root, socket, stats, base, window };
            if self.database == Database::Density {
                let host = crate::measure::HostMemory::new(daemon.id(), metrics);
                return self.run_held(&served, &host);
            }
            let role = if forked { "run" } else { "load" };
            for vm in 0..vms() {
                let running = self.start(&served, role, vm, load_memory()?, None)?;
                running.finish().with_context(|| format!("VM {} of {}", vm + 1, vms()))?;
            }
            crate::common::say_daemon_stats(test_setup, "extentd");
            crate::common::check_durable_median(test_setup, "extentd")
        }

        /// fxdax: the VMs of a density case (TASK-164.5), each on a fork of the base, booted one
        /// after another, and kept running once its runs are done, its guest waiting for the
        /// host's word, until the last's are: the host's memory said before the first VM boots
        /// and as each is done, by `host`, what each VM more costs it; then let go, in order.
        fn run_held(
            &self,
            served: &Served,
            host: &crate::measure::HostMemory,
        ) -> anyhow::Result<()> {
            host.say(0, "base", None, "")?;
            let mut held = Vec::new();
            for vm in 0..vms() {
                let requests = crate::measure::Requests::new(&nth("run", vm))?;
                let memory = density_memory()?;
                let mut running = self.start(served, "run", vm, memory, Some(&requests))?;
                let request = (requests.next(&mut running.firecracker))
                    .with_context(|| format!("VM {} of {}", vm + 1, vms()))?;
                host.say(vm + 1, &request.phase, Some(running.firecracker.pid()), &request.guest)?;
                held.push((running, request, requests));
            }
            for (vm, (running, request, _requests)) in held.into_iter().enumerate() {
                request.answer()?;
                running.finish().with_context(|| format!("VM {} of {}", vm + 1, vms()))?;
            }
            Ok(())
        }

        /// Boots VM `vm`, of `memory` MiB, its guest in role `role`, on a fork of the base of its
        /// own, sampling it; its guest asks the host to measure it at `requests`, if given.
        fn start(
            &self,
            served: &Served,
            role: &str,
            vm: usize,
            memory: u32,
            requests: Option<&crate::measure::Requests>,
        ) -> anyhow::Result<Running> {
            let dir = &served.test_setup.tmp_dir;
            let fork = dir.join(nth("fork", vm));
            crate::common::control(&served.socket)?.fork_store(&served.base, &fork)?;
            let device =
                Device::Store { store: &fork, extentd: &served.socket, window: served.window };
            let mut machine = machine(role, device, memory)?;
            if let Some(requests) = requests {
                machine.vsock(requests.uds());
                machine.env(c"FXDAX_PGBENCH_MEASURE=1");
            }
            let firecracker = boot(served.test_setup, served.root, &nth(role, vm), machine)?;
            let stop = Arc::new(AtomicBool::new(false));
            let sampler = sample(
                served.socket.clone(),
                fork,
                served.stats.clone(),
                dir.join(format!("{}.txt", nth("samples", vm))),
                firecracker.pid(),
                stop.clone(),
            );
            let profiler = profile_guest(dir.join(format!("{}.txt", nth("perf-kvm", vm))));
            Ok(Running { firecracker, stop, sampler, profiler })
        }
    }

    impl Test for TestFxfsGuestPgbench {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        fn timeout_secs(&self) -> u64 {
            // fxdax: a base's VM making the database, as each VM does otherwise.
            let base = if self.database == Database::Made { 0 } else { 600 };
            base + (runs() as u64 * run_secs() + 900) * vms() as u64
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            let loaded = format!("OK load {}\n", SCALE * 100_000);
            let ran = format!("OK runs {}\nOK checked\n", runs());
            let expected = match self.database {
                Database::Made => format!("OK base\n{}", format!("{loaded}{ran}").repeat(vms())),
                Database::Forked | Database::Density => {
                    format!("{loaded}OK base\n{}", ran.repeat(vms()))
                }
            };
            if output != expected {
                return TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"));
            }
            // The guest's profile from the host, if asked for, symbolized (TASK-131.6).
            if std::env::var_os("FXDAX_HOST_GUEST_PROFILE").is_some() {
                let report = test_setup.tmp_dir.join("perf-kvm.txt");
                let text = std::fs::read_to_string(&report).unwrap_or_default();
                if !text.contains("fxfs_") {
                    return TestOutcome::Fail(format!("{}: no Fxfs symbol: {text}", report.display()));
                }
            }
            TestOutcome::Pass
        }
    }

    /// fxdax: the milliseconds the case's daemon adds to each of its bucket's requests:
    /// FXDAX_REMOTE_LATENCY_MS's, `given`, if set; otherwise 25, the sandbox team's RustFS's, to a
    /// bucket here, `bucket` naming one in files or none (the daemon's is then in its memory,
    /// `crate::common::spawn_daemon`), and none to a real one, which is as far as it is. Added to
    /// GCS's, the forked case's restart after its crash, its reads of what the fork inherited
    /// ranged GETs one after another, took more than pg_ctl's 900 s (GCE r740, TASK-209).
    fn added_latency<'a>(given: Option<&'a OsStr>, bucket: Option<&'a OsStr>) -> &'a OsStr {
        let here = bucket.and_then(OsStr::to_str).is_none_or(|url| {
            url.is_empty() || url.starts_with("memory:") || url.starts_with("file:")
        });
        given.unwrap_or(OsStr::new(if here { "25" } else { "" }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The 25 ms stand for a bucket as far as the sandbox team's: added to one in memory or
        /// in files, not to a real one, which is as far as it is.
        #[test]
        fn latency_is_added_to_local_buckets_alone() {
            let at = |given: Option<&str>, bucket: Option<&str>| {
                added_latency(given.map(OsStr::new), bucket.map(OsStr::new)).to_owned()
            };
            assert_eq!(at(None, None), "25");
            assert_eq!(at(None, Some("")), "25");
            assert_eq!(at(None, Some("memory:///e2e")), "25");
            assert_eq!(at(None, Some("file:///dev/shm/bucket")), "25");
            assert_eq!(at(None, Some("gs://bucket/prefix/run")), "");
            assert_eq!(at(None, Some("s3://bucket/prefix")), "");
            assert_eq!(at(Some("50"), Some("gs://bucket/prefix")), "50");
            assert_eq!(at(Some("0"), None), "0");
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
    use std::process::{Command, Output, Stdio};

    const MNT: &str = "/mnt/fxfs";

    /// Mounts the database's filesystem at MNT: the guest's Fxfs on the store tagged `extents`,
    /// made as it mounts; or, on the virtio-pmem device FXDAX_PGBENCH_PMEM names, ext4 with DAX,
    /// which the base (`role`) makes, and which must map its files' data in place (TASK-131.5).
    fn mount(role: &str) {
        fs::create_dir_all(MNT).unwrap();
        let (source, fstype, data) = match std::env::var("FXDAX_PGBENCH_PMEM") {
            Ok(device) => {
                if role == "base" {
                    let status = Command::new("mkfs.ext4").args(["-q", "-F", &device]).status();
                    assert!(status.expect("mkfs.ext4").success(), "mkfs.ext4 {device}");
                }
                (device, "ext4", "dax=always")
            }
            Err(_) => ("extents".to_owned(), "fxfs", ""),
        };
        let (source, target, fstype, options) = (
            CString::new(source).unwrap(),
            CString::new(MNT).unwrap(),
            CString::new(fstype).unwrap(),
            CString::new(data).unwrap(),
        );
        let ret = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                0,
                options.as_ptr().cast(),
            )
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
        // ext4 on a device without DAX mounts without it, saying so in the kernel's log alone.
        if !data.is_empty() {
            let mounts = fs::read_to_string("/proc/mounts").unwrap();
            let mounted = mounts.lines().find(|line| line.split(' ').nth(1) == Some(MNT));
            assert!(mounted.is_some_and(|line| line.contains(data)), "{MNT}: {mounted:?}");
        }
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// Runs `command` as the user postgres, which PostgreSQL runs as, and returns its output,
    /// which must say it succeeded; what it says on stderr, and when it started and how long it
    /// took, go to the console, to follow a slow run.
    fn as_postgres(command: &str) -> Output {
        let start = std::time::Instant::now();
        // The guest's uptime, to set beside the host's samples of the daemon.
        let uptime = fs::read_to_string("/proc/uptime").unwrap_or_default();
        let uptime = uptime.split(' ').next().unwrap_or_default();
        eprintln!("pgbench: at {uptime} s: {command}");
        let output = Command::new("su")
            .args(["postgres", "-s", "/bin/sh", "-c", command])
            .stderr(Stdio::inherit())
            .output()
            .expect("su");
        eprintln!("pgbench: {:.1?}, {}", start.elapsed(), output.status);
        // What the filesystem waited on so far (TASK-91.7), on one line.
        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap_or_default();
        eprintln!("fxfs stats: {}", stats.lines().collect::<Vec<_>>().join(" "));
        // And the device's interrupts and the requests completed without one (TASK-91.15).
        eprintln!("device completions: {}", device_completions().join(" "));
        // And the guest's CPU time so far, in USER_HZ ticks: user nice system idle iowait irq
        // softirq steal (proc(5)), its system time what spinning for requests took (TASK-91.13).
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        eprintln!("guest cpu: {}", stat.lines().next().unwrap_or_default());
        assert!(
            output.status.success(),
            "{command}: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        );
        output
    }

    /// What `query` returns, through psql at `socket`, unaligned, its rows on lines.
    fn psql(socket: &str, query: &str) -> String {
        let output =
            as_postgres(&format!("psql -h {socket} -v ON_ERROR_STOP=1 -At -c \"{query}\" bench"));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// fxdax: the database is whole (TASK-119.4), after a run that began with each table's
    /// balances summing to `before`: each of pgbench's transactions adds a delta to an account,
    /// its teller and its branch, and logs it in the history, which pgbench empties as a run begins:
    /// each table's balances sum to `before` and the history's deltas. And, with amcheck, every
    /// tuple of the tables is sound, and their indexes hold them all; a page whose checksum fails
    /// fails the query reading it. Returns the balances' sum.
    fn check_data(socket: &str, before: i64) -> i64 {
        let sums = psql(
            socket,
            "select (select coalesce(sum(abalance), 0) from pgbench_accounts), \
             (select coalesce(sum(tbalance), 0) from pgbench_tellers), \
             (select coalesce(sum(bbalance), 0) from pgbench_branches), \
             (select coalesce(sum(delta), 0) from pgbench_history)",
        );
        let sums: Vec<i64> = sums.split('|').map(|sum| sum.parse().unwrap()).collect();
        assert_eq!(sums.len(), 4, "{sums:?}");
        let after = before + sums[3];
        assert_eq!(
            sums[..3],
            [after; 3],
            "accounts, tellers and branches' sums, from {before}, and the history's {}",
            sums[3]
        );
        psql(socket, "create extension if not exists amcheck");
        let tables = "array['pgbench_accounts', 'pgbench_branches', 'pgbench_tellers', \
                      'pgbench_history']::regclass[]";
        let corrupt = psql(
            socket,
            &format!("select count(*) from unnest({tables}) as t(rel), verify_heapam(rel)"),
        );
        assert_eq!(corrupt, "0", "tuples verify_heapam finds corrupt");
        let indexes = psql(
            socket,
            &format!(
                "select count(*) from (select bt_index_check(indexrelid, true) from pg_index \
                 where indrelid = any({tables})) as checked"
            ),
        );
        assert_eq!(indexes, "3", "the pgbench tables' indexes checked");
        after
    }

    /// The extent device's `completions` attribute's lines.
    fn device_completions() -> Vec<String> {
        let driver = "/sys/bus/virtio/drivers/virtio_extent";
        let device = fs::read_dir(driver)
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("virtio"));
        let text = device
            .and_then(|device| fs::read_to_string(device.path().join("completions")).ok())
            .unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }

    /// Has the kernel count its locks' contention from now on, from nothing (`on`), or stop, and
    /// prints the most contended (/proc/lock_stat, CONFIG_LOCK_STAT).
    fn count_locks(on: bool) {
        if on {
            fs::write("/proc/lock_stat", "0").expect("a kernel counting locks (LOCK_STAT)");
            fs::write("/proc/sys/kernel/lock_stat", "1").unwrap();
            return;
        }
        fs::write("/proc/sys/kernel/lock_stat", "0").unwrap();
        let stat = fs::read_to_string("/proc/lock_stat").unwrap();
        eprintln!("locks: the most contended");
        for line in stat.lines().take(120) {
            eprintln!("{line}");
        }
    }

    /// The guest's CPUs sampled, with their call chains, for most of a run.
    fn profile() -> std::process::Child {
        Command::new("perf")
            .args(["record", "-e", "cpu-clock", "-a", "-g", "-o", "/tmp/perf.data", "--"])
            .args(["sleep", &run_secs().saturating_sub(5).max(1).to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("perf record")
    }

    /// What `profiler` sampled, on the console: the guest's time by command and by symbol, and the
    /// call chains into the paravirtual spinlock's slow path, the round 6 report's 20-42%
    /// (TASK-91.14).
    fn report(profiler: std::process::Child) {
        let status = profiler.wait_with_output().expect("perf record").status;
        assert!(status.success(), "perf record: {status}");
        let perf = |args: &[&str]| {
            let output = Command::new("perf")
                .args(["report", "-i", "/tmp/perf.data", "--stdio", "--no-children"])
                .args(args)
                .output()
                .expect("perf report");
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        };
        eprintln!("profile: by command");
        perf(&["--sort", "comm", "-g", "none", "--percent-limit", "0.5"]);
        eprintln!("profile: by symbol");
        perf(&["--sort", "symbol", "-g", "none", "--percent-limit", "0.3"]);
        eprintln!("profile: the spinlock's callers");
        let lock = "__pv_queued_spin_lock_slowpath";
        perf(&["--sort", "symbol", "-S", lock, "-g", "caller,0.5,callee", "--percent-limit", "1"]);
    }

    /// The database's directory, and the server's socket's.
    const DATA: &str = "/mnt/fxfs/pgdata";
    const SOCKET: &str = "/run/postgresql";

    impl TestFxfsGuestPgbench {
        /// Starts the server on the database at [`DATA`]: its output to a log of its own, as it
        /// would hold `as_postgres`'s pipe open, which waits for its end.
        fn start(&self) {
            let fsync = if self.fsync { "" } else { " -c fsync=off" };
            let options =
                format!("-c shared_buffers=256MB -k {SOCKET} -c listen_addresses=''{fsync}");
            as_postgres(&format!(
                "pg_ctl -D {DATA} -l {DATA}/server.log -o \"{options}\" -w -t 900 start"
            ));
        }

        /// Makes the database in [`DATA`], with its server, and loads it, every account once.
        fn load(&self) {
            // fxdax: every page checksummed, which reading it checks (TASK-119.4).
            as_postgres(&format!("initdb -D {DATA} -A trust --data-checksums"));
            self.start();
            as_postgres(&format!("createdb -h {SOCKET} bench"));
            as_postgres(&format!("pgbench -h {SOCKET} -i -q -s {SCALE} bench"));
            // Every account once: the primary key's build found one twice (TASK-59).
            let query = "select count(*), count(distinct aid) from pgbench_accounts";
            let counted = as_postgres(&format!("psql -h {SOCKET} -At -c '{query}' bench"));
            let accounts = SCALE * 100_000;
            assert_eq!(
                String::from_utf8_lossy(&counted.stdout).trim(),
                format!("{accounts}|{accounts}")
            );
            println!("OK load {accounts}");
        }

        /// Runs pgbench on the database, its server running, as the sandbox team's runs, each of
        /// which must end well, checking it after each; then has it recover from a crash, and
        /// checks it again. fxdax: with FXDAX_PGBENCH_MEASURE, asks the host to measure it once its
        /// runs are done, and waits for the host's word (`crate::measure`, TASK-164.5).
        fn run(&self) {
            // pgbench says each run's tps, on the console, to compare, and PostgreSQL what it wrote
            // so far, its WAL and its relations' pages, to set beside what the device was written
            // (TASK-91.12).
            let written = "select (select wal_bytes from pg_stat_wal), \
                (select sum(writes * op_bytes) from pg_stat_io)";
            // The balances' sum before the last run, and after it.
            let (mut before, mut after) = (0, 0);
            for i in 0..runs() {
                let probe = if i == 0 { self.probe } else { Probe::None };
                let profiler = (probe == Probe::Profile).then(profile);
                if probe == Probe::Locks {
                    count_locks(true);
                }
                let asynchronous = std::env::var_os("FXDAX_PGBENCH_ASYNC").is_some();
                let options = if probe == Probe::Locks || asynchronous {
                    "PGOPTIONS='-c synchronous_commit=off' "
                } else {
                    ""
                };
                let secs = run_secs();
                let run =
                    as_postgres(&format!("{options}pgbench -h {SOCKET} -c 4 -j 2 -T {secs} bench"));
                if let Some(profiler) = profiler {
                    report(profiler);
                }
                if probe == Probe::Locks {
                    count_locks(false);
                }
                eprint!("{}", String::from_utf8_lossy(&run.stdout));
                let written = as_postgres(&format!("psql -h {SOCKET} -At -c '{written}' bench"));
                eprint!("postgres wal|pages written: {}", String::from_utf8_lossy(&written.stdout));
                before = after;
                after = check_data(SOCKET, before);
            }
            println!("OK runs {}", runs());
            // fxdax: what this VM costs the host, its server running still (TASK-164.5).
            if std::env::var_os("FXDAX_PGBENCH_MEASURE").is_some() {
                crate::measure::measure("ran");
            }
            // fxdax: stopped as if it had crashed, with no checkpoint, it recovers from its WAL,
            // as the filesystem kept it, to a whole database (TASK-119.4).
            as_postgres(&format!("pg_ctl -D {DATA} -w -m immediate stop"));
            self.start();
            assert_eq!(check_data(SOCKET, before), after, "the balances recovered");
            as_postgres(&format!("pg_ctl -D {DATA} -w -m fast stop"));
            // Every page's checksum, the indexes' too, with the server stopped.
            as_postgres(&format!("pg_checksums --check -D {DATA}"));
            println!("OK checked");
        }
    }

    /// Makes the server's socket's directory, and gives it and the database's to PostgreSQL.
    fn prepare() {
        fs::create_dir_all(SOCKET).unwrap();
        let status = Command::new("chown").args(["postgres:postgres", DATA, SOCKET]).status();
        assert!(status.expect("chown").success(), "chown {DATA} {SOCKET}");
    }

    impl Test for TestFxfsGuestPgbench {
        fn in_guest(self: Box<Self>) {
            // Every message on the console: a task hung says where, its stack logged at KERN_INFO.
            fs::write("/proc/sys/kernel/printk", "7").unwrap();
            let role = std::env::var("FXDAX_PGBENCH_ROLE").unwrap();
            mount(&role);
            match role.as_str() {
                // The base's filesystem, made as it mounts, with a directory for the database;
                // fxdax: or with the database, made, loaded and stopped cleanly, which each VM
                // runs pgbench on a fork of (TASK-164.5).
                "base" => {
                    fs::create_dir_all(DATA).unwrap();
                    if std::env::var_os("FXDAX_PGBENCH_BASE_DATABASE").is_some() {
                        prepare();
                        self.load();
                        as_postgres(&format!("pg_ctl -D {DATA} -w -m fast stop"));
                    }
                    unmount();
                    println!("OK base");
                }
                // Makes the database, then runs pgbench on it.
                "load" => {
                    prepare();
                    self.load();
                    self.run();
                    unmount();
                }
                // fxdax: runs pgbench on the base's database.
                "run" => {
                    prepare();
                    self.start();
                    self.run();
                    unmount();
                }
                role => panic!("no role {role}"),
            }
        }
    }
}
