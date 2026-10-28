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
//! against on the same host (TASK-91.17).

use macros::{guest, host};

pub struct TestFxfsGuestPgbench {
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    #[cfg_attr(feature = "host", allow(dead_code))]
    pub probe: Probe,
    /// Whether PostgreSQL fsyncs, as it does by default.
    #[cfg_attr(feature = "host", allow(dead_code))]
    pub fsync: bool,
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

/// pgbench's scale: 100,000 accounts a unit.
#[cfg_attr(feature = "host", allow(dead_code))]
const SCALE: u64 = 50;

/// The pgbench runs after the load, and each one's seconds, as the sandbox team's.
#[cfg_attr(feature = "host", allow(dead_code))]
const RUNS: usize = 5;
#[cfg_attr(feature = "host", allow(dead_code))]
const RUN_SECS: u64 = 60;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::ffi::{CString, OsStr};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// The stores' capacity, and the fork's VM's DAX window if it has one, as the sandbox team's.
    const CAPACITY: u64 = 16 << 30;
    const WINDOW: u64 = 2 << 30;

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

    /// How many VMs load and run pgbench, one after another, each on a fork of the base of its own,
    /// all served by the one daemon: FXDAX_PGBENCH_VMS, or 1. The sandbox team's runs had one
    /// daemon serve many, whose memory reached its cap from the third on, where most of their
    /// crashes came (TASK-102, TASK-103).
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
RUN apk add --no-cache postgresql postgresql-contrib perf
";

    /// A VM named `name`, of `memory` MiB, on the store at `store`, which the daemon at `extentd`
    /// serves, run in Firecracker: its guest's role what its name begins with (`load-1`, a load's),
    /// its API socket and files named after it, each VM's its own, as one's socket outlives it.
    fn run(
        test_setup: &TestSetup,
        root: &Path,
        name: &str,
        store: &Path,
        extentd: &Path,
        memory: u32,
        window: u64,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(vcpus()?, memory)?;
        vm.guest_kernel()?;
        vm.extent_store_served("extents", store, CAPACITY, window, extentd);
        let role = name.split('-').next().unwrap_or(name);
        vm.env(&CString::new(format!("FXDAX_PGBENCH_ROLE={role}"))?);
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

    impl TestFxfsGuestPgbench {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            // A daemon of its own, whose bucket answers 25 ms late, as the sandbox team's RustFS
            // did, or as FXDAX_REMOTE_LATENCY_MS says; its socket's path short, as Unix sockets'
            // must be.
            let sockets = tempdir::TempDir::new("fxdax-pgbench")?;
            let socket = sockets.path().join("extentd.sock");
            let latency = std::env::var_os("FXDAX_REMOTE_LATENCY_MS");
            let stats = dir.join("extentd-stats.txt");
            let env = [
                ("FXDAX_REMOTE_LATENCY_MS", latency.as_deref().unwrap_or(OsStr::new("25"))),
                ("FXDAX_EXTENTD_STATS", stats.as_os_str()),
            ];
            // As much memory as the sandbox team's daemon has: with its default, 1 GiB, the guest's
            // working set, a 2 GiB window's, didn't fit, and faults paged in and unpacked again
            // what was evicted, slower as the database grew.
            let memory = Some(8 << 30);
            let _daemon =
                crate::common::spawn_daemon(test_setup, &socket, "extentd", &env, memory)?;
            let (base, fork) = (dir.join("base"), dir.join("fork"));
            wait(run(test_setup, &root, "base", &base, &socket, 512, 0)?)?;
            let window = if self.dax { window()? } else { 0 };
            for vm in 0..vms() {
                let (fork, samples, name) = match vm {
                    0 => (fork.clone(), dir.join("samples.txt"), "load".to_owned()),
                    vm => (
                        dir.join(format!("fork-{vm}")),
                        dir.join(format!("samples-{vm}.txt")),
                        format!("load-{vm}"),
                    ),
                };
                crate::common::control(&socket)?.fork_store(&base, &fork)?;
                let firecracker =
                    run(test_setup, &root, &name, &fork, &socket, load_memory()?, window)?;
                let stop = Arc::new(AtomicBool::new(false));
                let sampler = sample(
                    socket.clone(),
                    fork.clone(),
                    stats.clone(),
                    samples,
                    firecracker.pid(),
                    stop.clone(),
                );
                let loaded = wait(firecracker);
                stop.store(true, Ordering::Relaxed);
                sampler.join().expect("the sampler panicked")?;
                loaded.with_context(|| format!("VM {} of {}", vm + 1, vms()))?;
            }
            Ok(())
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
            1200 * vms() as u64
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
            let each = format!("OK load {}\nOK runs {RUNS}\nOK checked\n", SCALE * 100_000);
            let expected = format!("OK base\n{}", each.repeat(vms()));
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
    use std::ffi::CString;
    use std::fs;
    use std::process::{Command, Output, Stdio};

    const MNT: &str = "/mnt/fxfs";

    fn mount() {
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

    /// fxdax: the database is whole (TASK-119.4). Each of pgbench's transactions adds a delta to an
    /// account, its teller and its branch, and logs it in the history: each table's balances sum
    /// to the history's deltas. And, with amcheck, every tuple of the tables is sound, and their
    /// indexes hold them all; a page whose checksum fails fails the query reading it.
    fn check_data(socket: &str) {
        let sums = psql(
            socket,
            "select (select sum(abalance) from pgbench_accounts), \
             (select sum(tbalance) from pgbench_tellers), \
             (select sum(bbalance) from pgbench_branches), \
             (select coalesce(sum(delta), 0) from pgbench_history)",
        );
        let sums: Vec<&str> = sums.split('|').collect();
        assert_eq!(sums.len(), 4, "{sums:?}");
        assert_eq!(sums, [sums[3]; 4], "accounts, tellers, branches and history's sums");
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
            .args(["sleep", &(RUN_SECS - 5).to_string()])
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

    impl Test for TestFxfsGuestPgbench {
        fn in_guest(self: Box<Self>) {
            // Every message on the console: a task hung says where, its stack logged at KERN_INFO.
            fs::write("/proc/sys/kernel/printk", "7").unwrap();
            mount();
            match std::env::var("FXDAX_PGBENCH_ROLE").unwrap().as_str() {
                // The base's filesystem, made as it mounts, with a directory for the database.
                "base" => {
                    fs::create_dir_all(format!("{MNT}/pgdata")).unwrap();
                    unmount();
                    println!("OK base");
                }
                "load" => {
                    let data = format!("{MNT}/pgdata");
                    let socket = "/run/postgresql";
                    fs::create_dir_all(socket).unwrap();
                    Command::new("chown")
                        .args(["postgres:postgres", &data, socket])
                        .status()
                        .unwrap();
                    // fxdax: every page checksummed, which reading it checks (TASK-119.4).
                    as_postgres(&format!("initdb -D {data} -A trust --data-checksums"));
                    let fsync = if self.fsync { "" } else { " -c fsync=off" };
                    let options = format!(
                        "-c shared_buffers=256MB -k {socket} -c listen_addresses=''{fsync}"
                    );
                    // The server's output to a log of its own: it would hold `as_postgres`'s pipe
                    // open, which waits for its end.
                    let start = || {
                        as_postgres(&format!(
                            "pg_ctl -D {data} -l {data}/server.log -o \"{options}\" -w start"
                        ))
                    };
                    start();
                    as_postgres(&format!("createdb -h {socket} bench"));
                    as_postgres(&format!("pgbench -h {socket} -i -q -s {SCALE} bench"));
                    // Every account once: the primary key's build found one twice (TASK-59).
                    let count = "select count(*), count(distinct aid) from pgbench_accounts";
                    let counted = as_postgres(&format!("psql -h {socket} -At -c '{count}' bench"));
                    let accounts = SCALE * 100_000;
                    assert_eq!(
                        String::from_utf8_lossy(&counted.stdout).trim(),
                        format!("{accounts}|{accounts}")
                    );
                    println!("OK load {accounts}");
                    // Then as the sandbox team's runs, each of which must end well: pgbench says
                    // so (its tps, on the console, to compare), and what PostgreSQL wrote so far,
                    // its WAL and its relations' pages, to set beside what the device was written
                    // (TASK-91.12).
                    let written = "select (select wal_bytes from pg_stat_wal), \
                        (select sum(writes * op_bytes) from pg_stat_io)";
                    for i in 0..RUNS {
                        let probe = if i == 0 { self.probe } else { Probe::None };
                        let profiler = (probe == Probe::Profile).then(profile);
                        let options = if probe == Probe::Locks {
                            count_locks(true);
                            "PGOPTIONS='-c synchronous_commit=off' "
                        } else {
                            ""
                        };
                        let run = as_postgres(&format!(
                            "{options}pgbench -h {socket} -c 4 -j 2 -T {RUN_SECS} bench"
                        ));
                        if let Some(profiler) = profiler {
                            report(profiler);
                        }
                        if probe == Probe::Locks {
                            count_locks(false);
                        }
                        eprint!("{}", String::from_utf8_lossy(&run.stdout));
                        let written =
                            as_postgres(&format!("psql -h {socket} -At -c '{written}' bench"));
                        eprint!(
                            "postgres wal|pages written: {}",
                            String::from_utf8_lossy(&written.stdout)
                        );
                        check_data(socket);
                    }
                    println!("OK runs {RUNS}");
                    // fxdax: stopped as if it had crashed, with no checkpoint, it recovers from
                    // its WAL, as the filesystem kept it, to a whole database (TASK-119.4).
                    as_postgres(&format!("pg_ctl -D {data} -w -m immediate stop"));
                    start();
                    check_data(socket);
                    as_postgres(&format!("pg_ctl -D {data} -w -m fast stop"));
                    // Every page's checksum, the indexes' too, with the server stopped.
                    as_postgres(&format!("pg_checksums --check -D {data}"));
                    println!("OK checked");
                    unmount();
                }
                role => panic!("no role {role}"),
            }
        }
    }
}
