// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: sandboxes as LangSmith's CI runs them (TASK-330.1), the shape of host their suites
//! found TASK-310 and TASK-311 on and ours didn't: two sandbox hosts, each with an extent daemon
//! of its own; one disk under both, an ext4 one, holding their stores, their part caches and the
//! bucket, an S3 server's (RustFS, Apache-2.0), whose objects are on that disk too; and many VMs
//! at once, more vCPUs between them than the host has CPUs, building Docker images and exporting
//! them, running pgbench, and working a git repository, each running a sandbox's user's commands
//! all the while.
//!
//! scripts/sandboxes.sh runs it: it starts the S3 server with its data on the disk the case's
//! directory is on, names its bucket (FXDAX_EXTENTD_REMOTE, an `s3://` URL, and the server's
//! address and credentials, AWS_*) and its data's directory (FXDAX_SANDBOX_BUCKET_DIR), which this
//! checks is on the case's disk. FXDAX_SANDBOX_MODE says how much: `short` (the default), eight
//! VMs at once, minutes, for every push to main (.github/workflows/sandboxes.yml); or `long`,
//! LangSmith's controlled load's sixteen, eight at a time, each doing more, by hand on ubu1 before
//! each pin handed to LangSmith (scripts/qualify.sh, TASK-330.5).
//!
//! Each host imports the sandboxes' image once, as a base: an Alpine root with PostgreSQL and git,
//! Docker's static binaries and its buildx plugin, and a BusyBox image to build from; each VM
//! boots on a fork of its host's base, its root Fxfs, as a sandbox does. The VMs, a host's and the
//! other's in turn, take roles in turn:
//!
//! - **builder**: 8 vCPUs, as LangSmith's builders, its root through a DAX window: Docker builds an
//!   image with BuildKit, stages in parallel, loads it, saves it, exports it as a tar of its files,
//!   as fxfs-guest-docker-build-dax does (TASK-311), its builds and their size as the mode says;
//! - **pgbench**: PostgreSQL makes its database on a store of the VM's own, loads it, runs pgbench
//!   with fsync, checks it whole, recovers it from a crash and checks it again, as
//!   fxfs-guest-pgbench does; every other one through a DAX window;
//! - **git**: a repository on a store of the VM's own, made commit by commit, packed, cloned and
//!   checked;
//! - **writer**: LangSmith's own load, which showed TASK-310: a large file that doesn't compress,
//!   fsynced, and small files, synced, read back and deleted, as fxfs-guest-shared-disk does;
//!   every other one through a DAX window.
//!
//! All the while each guest runs a sandbox's commands every second, `echo`, `uname` and `cat` of a
//! file of its root, timing each. It fails, saying each check that failed, if:
//!
//! - a VM's kernel reports anything: the runner fails a case on its consoles' complaints, with
//!   each console's first report (TASK-311's `vm::first_report`); and here a VM that doesn't end
//!   well, saying what its role did, fails;
//! - a daemon lets a VM process go (TASK-249);
//! - a daemon's framing takes longer than a second, [`FRAMING_BUDGET_MS`] (TASK-310: 3.3-9.6 s at
//!   LangSmith with write-behind's double writes);
//! - a sandbox's command takes longer than its budget, [`COMMAND_BUDGET_MS`] (LangSmith's
//!   sandboxes took 1.9 s for `/bin/echo` then);
//! - the hosts' stores write more than [`WRITES_BUDGET`] times what their guests wrote, every guest
//!   byte written half again (LangSmith's pin wrote 2.3 times, TASK-310);
//! - the VMs at once have no more vCPUs than the host has CPUs, or the bucket's data isn't on the
//!   case's disk: the run isn't the topology it says it is.
//!
//! And scripts/budgets.py holds each host's daemon to its design's budgets, its memory and its
//! disk's writes, as it does every run's daemons (TASK-330.2).

use macros::{guest, host};

pub struct TestFxfsGuestSandboxes;

/// What a sandbox VM does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Builder,
    Pgbench,
    Git,
    Writer,
}

impl Role {
    /// The roles, as the VMs take them in turn.
    const ALL: [Role; 4] = [Role::Builder, Role::Pgbench, Role::Git, Role::Writer];

    fn name(self) -> &'static str {
        match self {
            Role::Builder => "builder",
            Role::Pgbench => "pgbench",
            Role::Git => "git",
            Role::Writer => "writer",
        }
    }

    #[cfg_attr(feature = "host", allow(dead_code))]
    fn named(name: &str) -> Option<Role> {
        Self::ALL.into_iter().find(|role| role.name() == name)
    }
}

/// The longest a framing may take: past it, as LangSmith's hosts' framings were (TASK-310), the
/// commits behind it wait as long, and a sandbox's writes with them.
#[cfg_attr(feature = "guest", allow(dead_code))]
const FRAMING_BUDGET_MS: f64 = 1000.0;

/// The most the hosts' stores may write, between them, for each byte their guests wrote: half of
/// it again. The design writes a guest's byte once, raw; again only what a commit copies, or a
/// segment's data that compressing saves an eighth of, framed in a copy (TASK-310, TASK-92.41):
/// main wrote 1.14-1.27 times on these runs, pgbench's commits' copies the most of it. LangSmith's
/// pin wrote every byte twice, raw and framed, 2.3 times in all; put back, 1.83-1.91 here. Of
/// the run, not of a host: a host's ratio follows its roles' mix, one left with pgbench's
/// copies alone past TASK-330.2's 1.3 (1.39), and its VMs failing skew it more.
#[cfg_attr(feature = "guest", allow(dead_code))]
const WRITES_BUDGET: f64 = 1.5;

/// The longest a sandbox's command may take, from its start to its exit, in its guest: what a
/// user waits at a prompt before a sandbox seems hung. LangSmith's took 1.9 s for `/bin/echo`,
/// 6.3 s for `uname` and 11.9 s for `cat` behind TASK-310's framings, and their tests timed out.
#[cfg_attr(feature = "guest", allow(dead_code))]
const COMMAND_BUDGET_MS: u64 = 1000;

/// The commands a sandbox's user runs, as LangSmith's CI timed them: each a process of its own,
/// its binary and the file it reads on the guest's root.
#[cfg_attr(feature = "host", allow(dead_code))]
const COMMANDS: [(&str, &[&str]); 3] = [
    ("echo", &["/bin/echo", "sandbox"]),
    ("uname", &["uname", "-a"]),
    ("cat", &["cat", "/etc/os-release"]),
];

#[host]
mod host {
    use super::*;

    use crate::common::{
        Measured, ProcessCosts, cgroup_memory, control, fxdax_guest_should_run, resident,
        setup_rootfs,
    };
    use crate::test_fxfs_guest_docker::host::{DockerFiles, buildx};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::{Context, bail, ensure};
    use fxdax::import::testing::{layer, oci_layout};
    use std::ffi::CString;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const GIB: u64 = 1 << 30;

    /// The sandboxes' image: Alpine's PostgreSQL and git, beside BusyBox's shell and tools, which
    /// fxfs-guest-pgbench's runs on too.
    const CONTAINERFILE: &str = "\
FROM alpine:3.22
RUN apk add --no-cache postgresql postgresql-contrib git
";

    /// The base's capacity, and so each root's: a builder's layers, caches and archives, with
    /// room, as fxfs-guest-docker-build-dax's root has.
    const ROOT_CAPACITY: u64 = 16 * GIB;
    /// A VM's own store's, for its database, repository or files.
    const DATA_CAPACITY: u64 = 8 * GIB;

    /// How much a run does: FXDAX_SANDBOX_MODE's.
    struct Mode {
        name: &'static str,
        /// The VMs at once, and in all.
        at_once: usize,
        vms: usize,
        /// What each role does, in its guest's environment.
        settings: fn(Role) -> Vec<(&'static str, String)>,
        /// Each role's vCPUs, a builder's as LangSmith's builders have, and its memory, in MiB.
        shape: fn(Role) -> (u8, u32),
        /// Each daemon's memory: its extent cache's bound, and its windows' private pages'.
        daemon_memory: u64,
        /// A builder's root's DAX window, and the window of a VM's own store, when it has one:
        /// what a sandbox maps, not the host's memory, of which eight windows at once were
        /// refused (ENOMEM, fxfs-guest-shared-disk-dax). Each window's private pages, half its
        /// pages, are pinned in its daemon's memory as it is made, out of what the daemon can
        /// evict: a host's builder's and its own stores' with windows at once, half of these.
        windows: (u64, u64),
        timeout_secs: u64,
    }

    /// The short mode: for every push to main, under ten minutes on CI's runner (3.4 min on
    /// GitHub's). Its writers each write what LangSmith's controlled load's VMs did.
    fn short(role: Role) -> Vec<(&'static str, String)> {
        let settings: &[(&str, u64)] = match role {
            Role::Builder => &[("FXDAX_DOCKER_BUILDS", 1), ("FXDAX_DOCKER_BUILD_MIB", 256)],
            Role::Pgbench => &[
                ("FXDAX_PGBENCH_SCALE", 10),
                ("FXDAX_PGBENCH_RUNS", 1),
                ("FXDAX_PGBENCH_RUN_SECS", 60),
            ],
            Role::Git => &[("FXDAX_SANDBOX_GIT_FILES", 1000), ("FXDAX_SANDBOX_GIT_COMMITS", 30)],
            Role::Writer => {
                &[("FXDAX_SHARED_DISK_LARGE_MIB", 768), ("FXDAX_SHARED_DISK_FILES", 3000)]
            }
        };
        settings.iter().map(|(var, value)| (*var, value.to_string())).collect()
    }

    /// The short mode's VMs: 7.5 GiB for eight at once, beside two daemons of 3 GiB, on a CI
    /// runner of 16 GB, of which a run of minutes touches a part. Daemons of 4 GiB filled their
    /// caches to 2.9 GB each 6 minutes in and ran the runner's memory and swap out; at 2 GiB, a
    /// daemon's cache under eight guests' writes left no room to pin a window's pages in (ENOMEM).
    fn short_shape(role: Role) -> (u8, u32) {
        match role {
            Role::Builder => (8, 1536),
            Role::Pgbench => (2, 1024),
            Role::Git => (2, 512),
            Role::Writer => (2, 768),
        }
    }

    /// The long mode's VMs: 5.5 GiB for eight at once, beside two daemons of 2 GiB, RustFS and the
    /// stores' page cache, within the 16 GiB a measurement has on ubu1 (fxjob --bench, TASK-330.5):
    /// a run of a quarter hour touches every page it has, and with the short mode's sizes, its
    /// guests' 7.5 GiB, its daemons' 3 GiB caches full and RustFS's 1 GB, a daemon was killed for
    /// memory at 16 GiB and again at 18 GiB, the hosts' every job's.
    fn long_shape(role: Role) -> (u8, u32) {
        match role {
            Role::Builder => (8, 1024),
            Role::Pgbench => (2, 768),
            Role::Git => (2, 512),
            Role::Writer => (2, 512),
        }
    }

    /// The long mode: before a pin, LangSmith's writers, three builds a builder, pgbench on 5 M
    /// accounts, the sandbox team's, three times.
    fn long(role: Role) -> Vec<(&'static str, String)> {
        let settings: &[(&str, u64)] = match role {
            Role::Builder => &[("FXDAX_DOCKER_BUILDS", 3), ("FXDAX_DOCKER_BUILD_MIB", 512)],
            Role::Pgbench => &[
                ("FXDAX_PGBENCH_SCALE", 50),
                ("FXDAX_PGBENCH_RUNS", 3),
                ("FXDAX_PGBENCH_RUN_SECS", 60),
            ],
            Role::Git => &[("FXDAX_SANDBOX_GIT_FILES", 3000), ("FXDAX_SANDBOX_GIT_COMMITS", 100)],
            Role::Writer => {
                &[("FXDAX_SHARED_DISK_LARGE_MIB", 768), ("FXDAX_SHARED_DISK_FILES", 3000)]
            }
        };
        settings.iter().map(|(var, value)| (*var, value.to_string())).collect()
    }

    fn mode() -> anyhow::Result<Mode> {
        match std::env::var("FXDAX_SANDBOX_MODE").as_deref() {
            Err(_) | Ok("short") => Ok(Mode {
                name: "short",
                at_once: 8,
                vms: 8,
                settings: short,
                shape: short_shape,
                daemon_memory: 3 * GIB,
                windows: (GIB, GIB / 2),
                timeout_secs: 1200,
            }),
            // LangSmith's controlled load's shape (TASK-310): 16 VMs, 8 at a time, each doing more
            // than a short run's; on ubu1 by hand before each pin (TASK-330.5).
            Ok("long") => Ok(Mode {
                name: "long",
                at_once: 8,
                vms: 16,
                settings: long,
                shape: long_shape,
                // Windows of the short mode's, 1 GiB and 512 MiB, were refused by daemons of 2 GiB
                // whose caches eight guests' writes filled (ENOMEM, 2 a host).
                daemon_memory: 2 * GIB,
                windows: (GIB / 2, GIB / 4),
                timeout_secs: 7200,
            }),
            Ok(other) => bail!("FXDAX_SANDBOX_MODE is short or long, not {other:?}"),
        }
    }

    /// A sandbox host: its extent daemon and the base its VMs boot on forks of.
    struct Host {
        name: &'static str,
        daemon: std::process::Child,
        socket: PathBuf,
        base: PathBuf,
    }

    /// A VM of the run, by its number: its role, its host, and whether its own store, if it has
    /// one, is mapped through a DAX window.
    struct Sandbox {
        vm: usize,
        role: Role,
        host: usize,
        dax: bool,
    }

    impl Sandbox {
        /// The `vm`th of a run: the roles in turn, the hosts in turn, each round of the roles' on
        /// the hosts the other way round from the round before, so that each host has each role;
        /// and every other round with DAX windows on the VMs' own stores.
        fn nth(vm: usize) -> Self {
            let (role, round) = (Role::ALL[vm % Role::ALL.len()], vm / Role::ALL.len());
            Self { vm, role, host: (vm + round) % 2, dax: round % 2 == 0 }
        }

        fn name(&self) -> String {
            format!("sandbox-{}-{}", self.vm, self.role.name())
        }
    }

    /// The host's CPUs this process may run on, as its VMs may: its affinity's, within its
    /// cgroup's quota (fxjob's cores, a CI runner's).
    fn host_cpus() -> anyhow::Result<usize> {
        Ok(std::thread::available_parallelism()?.get())
    }

    /// The sandboxes' image, in an OCI layout at `layout`: the root `root` holds, the case's
    /// Containerfile's with the guest agent, as a layer; then Docker's files over it.
    fn image(dir: &Path, root: &Path, layout: &Path) -> anyhow::Result<()> {
        // As root's, as an image's files are: the runner's user unpacked them.
        let tar = std::process::Command::new("tar")
            .args(["--numeric-owner", "--owner=0", "--group=0", "-C"])
            .arg(root)
            .args(["-cf", "-", "."])
            .output()
            .context("running tar")?;
        ensure!(tar.status.success(), "tar of {}: {}", root.display(), tar.status);
        let docker = DockerFiles::new(dir, Some(buildx()?))?;
        oci_layout(layout, "fxdax/sandbox:1", &[tar.stdout, layer(&docker.entries())?])?;
        Ok(())
    }

    /// Boots `sandbox` on a fork of its host's base, as `mode` has its role.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        hosts: &[Host],
        sandbox: &Sandbox,
        mode: &Mode,
    ) -> anyhow::Result<Firecracker> {
        let dir = &test_setup.tmp_dir;
        let host = &hosts[sandbox.host];
        let name = sandbox.name();
        let fork = dir.join(format!("{name}.root"));
        control(&host.socket)?.fork_store(&host.base, &fork)?;
        let (vcpus, memory) = (mode.shape)(sandbox.role);
        let mut machine = Vm::new(vcpus, memory)?;
        machine.guest_kernel()?;
        let (root_window, data_window) = mode.windows;
        let root_window = if sandbox.role == Role::Builder { root_window } else { 0 };
        machine.extent_store_served("root", &fork, ROOT_CAPACITY, root_window, &host.socket);
        machine.fxfs_root("root");
        if sandbox.role != Role::Builder {
            let window = if sandbox.dax { data_window } else { 0 };
            let data = dir.join(format!("{name}.data"));
            machine.extent_store_served("extents", &data, DATA_CAPACITY, window, &host.socket);
        }
        let mut env = vec![
            ("FXDAX_SANDBOX_ROLE", sandbox.role.name().to_owned()),
            ("FXDAX_SHARED_DISK_SEED", sandbox.vm.to_string()),
            ("FXDAX_PGBENCH_ROLE", "load".to_owned()),
        ];
        env.extend((mode.settings)(sandbox.role));
        for (var, value) in env {
            machine.env(&CString::new(format!("{var}={value}"))?);
        }
        machine.spawn_api(root, &test_setup.test_case, &name, &Vm::api_socket(&name))
    }

    /// What a VM's guest said of its commands: each command's runs and its slowest, in ms, from
    /// its `commands:` line, `NAME=RUNS/SLOWEST ...`.
    fn commands(said: &str) -> Option<Vec<(String, u64, u64)>> {
        let line = said.lines().find_map(|line| line.strip_prefix("commands: "))?;
        line.split(' ')
            .map(|field| {
                let (name, counts) = field.split_once('=')?;
                let (runs, slowest) = counts.split_once('/')?;
                Some((name.to_owned(), runs.parse().ok()?, slowest.parse().ok()?))
            })
            .collect()
    }

    /// The framings of the daemon whose log is `log` past extentd's turn, as it warns of them:
    /// each's milliseconds.
    fn slow_framings(log: &str) -> Vec<f64> {
        log.lines()
            .filter_map(|line| {
                let (_, after) = line.split_once("extent store: framing ")?;
                let (_, after) = after.split_once(" took ")?;
                after.split_once(" ms")?.0.parse::<f64>().ok()
            })
            .collect()
    }

    /// What a run found of its VMs as they ended: the checks they failed, each role's longest
    /// VM, which says where a run's time went (TASK-330.5), and each command's slowest.
    #[derive(Default)]
    struct Tally {
        failed: Vec<String>,
        longest: std::collections::BTreeMap<&'static str, f64>,
        slowest: std::collections::BTreeMap<String, u64>,
    }

    /// Ends `sandbox`'s VM, `firecracker`, booted at `booted`: what its guest said, or why it
    /// failed, and how long it ran, into `tally`.
    fn finish(
        firecracker: Firecracker,
        sandbox: &Sandbox,
        booted: Instant,
        tally: &mut Tally,
    ) -> anyhow::Result<()> {
        let name = sandbox.name();
        let (status, console) = firecracker.wait()?;
        let ran = booted.elapsed().as_secs_f64();
        eprintln!("sandboxes: {name}: ran {ran:.1} s");
        let longest = tally.longest.entry(sandbox.role.name()).or_default();
        *longest = longest.max(ran);
        // Its API's socket, which Firecracker leaves.
        let api = Vm::api_socket(&name);
        std::fs::remove_file(&api).with_context(|| format!("removing {}", api.display()))?;
        let said =
            crate::vm::without_kernel_lines(&crate::vm::guest_stdout(&std::fs::read(&console)?));
        let ok = format!("OK sandbox {}", sandbox.role.name());
        if !status.success() || !said.lines().any(|line| line == ok) {
            tally.failed.push(format!("vm: {name} exited {status}, saying {said:?}"));
            return Ok(());
        }
        let Some(commands) = commands(&said) else {
            tally.failed.push(format!("vm: {name} said nothing of its commands: {said:?}"));
            return Ok(());
        };
        let report: Vec<String> = (commands.iter())
            .map(|(command, runs, slowest)| format!("{command}={runs}/{slowest}ms"))
            .collect();
        eprintln!("sandboxes: {name}: commands {}", report.join(" "));
        for (command, _, slowest) in commands {
            let most = tally.slowest.entry(command.clone()).or_default();
            *most = (*most).max(slowest);
            if slowest > COMMAND_BUDGET_MS {
                tally.failed.push(format!(
                    "commands: {name}'s {command} took {slowest} ms, its budget {COMMAND_BUDGET_MS} ms"
                ));
            }
        }
        Ok(())
    }

    /// What's left of the run's memory before the OOM killer ends it: past reclaim, its cgroup's
    /// limit less this, [`OOM_HEADROOM`].
    const OOM_HEADROOM: u64 = GIB;

    /// Whether the run is about to run out of memory, `hosts`' daemons among what holds it: what
    /// its cgroup holds past reclaim within [`OOM_HEADROOM`] of its limit ([`cgroup_memory`]),
    /// and then a failed check saying so, and each daemon's memory.
    fn out_of_memory(hosts: &[Host]) -> Option<String> {
        let (max, held) = cgroup_memory()?;
        if held + OOM_HEADROOM <= max {
            return None;
        }
        let daemons: Vec<String> = (hosts.iter())
            .map(|host| format!("{} {} MiB", host.name, resident(host.daemon.id()) >> 20))
            .collect();
        Some(format!(
            "memory: the run's memory ran out: {} MiB held past reclaim of its {} MiB, its daemons \
             holding {}",
            held >> 20,
            max >> 20,
            daemons.join(" and ")
        ))
    }

    /// Says each check `failed`, and records it in `measured`, by its kind, the words before its
    /// first colon.
    fn say_failures(failed: &[String], measured: &mut Measured) {
        for failure in failed {
            eprintln!("sandboxes: check failed: {failure}");
            let kind = failure.split_once(':').map_or("check", |(kind, _)| kind);
            measured.failed(kind, failure.clone());
        }
    }

    /// What host `host`'s stores wrote, as its daemon's stats file in `dir` counts it (TASK-310):
    /// its guests' bytes, raw, and its stores' in all.
    fn host_writes(dir: &Path, host: &str) -> anyhow::Result<(u64, u64)> {
        let stats = crate::common::daemon_stats(&dir.join(format!("{host}-stats.txt")))?;
        let kind = |kind: &str| stats.get(&format!("written_{kind}_bytes")).copied();
        let raw = kind("raw").context(
            "no written_raw_bytes in its stats: it counts no writes, as before TASK-310",
        )?;
        let written: u64 =
            ["raw", "copied", "framed", "own", "log"].into_iter().filter_map(kind).sum();
        eprintln!(
            "sandboxes: {host}: guest_gib={:.2} stores_written_gib={:.2} ratio={:.2} \
             segments_framed_in_place={}",
            raw as f64 / GIB as f64,
            written as f64 / GIB as f64,
            written as f64 / raw.max(1) as f64,
            stats.get("segments_framed_in_place").copied().unwrap_or(0),
        );
        Ok((raw, written))
    }

    impl TestFxfsGuestSandboxes {
        /// Runs the sandboxes, saying what it measured in `measured` and the checks they failed,
        /// as it fails then.
        fn run(&self, test_setup: &TestSetup, measured: &mut Measured) -> anyhow::Result<()> {
            let mode = mode()?;
            let dir = &test_setup.tmp_dir;
            let mut tally = Tally::default();
            // The topology: the bucket an S3 server's, its data on the case's disk.
            let remote = std::env::var("FXDAX_EXTENTD_REMOTE").unwrap_or_default();
            ensure!(
                remote.starts_with("s3://"),
                "FXDAX_EXTENTD_REMOTE is {remote:?}, not an S3 server's bucket: scripts/sandboxes.sh \
                 starts one"
            );
            let bucket_dir = std::env::var_os("FXDAX_SANDBOX_BUCKET_DIR")
                .context("FXDAX_SANDBOX_BUCKET_DIR does not name the bucket's data")?;
            {
                use std::os::unix::fs::MetadataExt as _;
                let (disk, bucket) =
                    (std::fs::metadata(dir)?.dev(), std::fs::metadata(&bucket_dir)?.dev());
                if disk != bucket {
                    tally.failed.push(format!(
                        "topology: the bucket's data, {bucket_dir:?}, is on another disk than the \
                         case's, {}",
                        dir.display()
                    ));
                }
            }
            let root = setup_rootfs(test_setup)?;
            let layout = dir.join("sandbox-image");
            image(dir, &root, &layout)?;
            // Short, as a Unix socket's path is.
            let sockets = tempdir::TempDir::new("fxdax-sandboxes")?;
            let mut hosts = Vec::new();
            for (at, name) in ["host-a", "host-b"].into_iter().enumerate() {
                let socket = sockets.path().join(format!("{name}.sock"));
                // Each host's own placement.
                let token = (at + 1).to_string();
                let env = [("FXDAX_EXTENTD_TOKEN", std::ffi::OsStr::new(&token))];
                let daemon = crate::common::spawn_daemon(
                    test_setup,
                    &socket,
                    name,
                    &env,
                    Some(mode.daemon_memory),
                )?;
                let base = dir.join(format!("{name}.base"));
                let imported = Instant::now();
                control(&socket)?.import_image(&base, ROOT_CAPACITY, &layout, None)?;
                eprintln!("sandboxes: {name} imported the image in {:?}", imported.elapsed());
                hosts.push(Host { name, daemon, socket, base });
            }
            let sandboxes: Vec<Sandbox> = (0..mode.vms).map(Sandbox::nth).collect();
            let cpus = host_cpus()?;
            let vcpus: usize = (sandboxes.iter().take(mode.at_once))
                .map(|sandbox| usize::from((mode.shape)(sandbox.role).0))
                .sum();
            eprintln!(
                "sandboxes: {} mode, {} VMs, {} at once, {vcpus} vCPUs at once on {cpus} CPUs",
                mode.name, mode.vms, mode.at_once
            );
            if vcpus <= cpus {
                tally
                    .failed
                    .push(format!("topology: {vcpus} vCPUs at once on {cpus} CPUs: no overcommit"));
            }
            let started = Instant::now();
            let (mut running, mut next) = (Vec::new(), 0);
            while next < sandboxes.len() || !running.is_empty() {
                while next < sandboxes.len() && running.len() < mode.at_once {
                    let sandbox = &sandboxes[next];
                    eprintln!(
                        "sandboxes: {} on {} booted at {:.1} s",
                        sandbox.name(),
                        hosts[sandbox.host].name,
                        started.elapsed().as_secs_f64()
                    );
                    let firecracker = boot(test_setup, &root, &hosts, sandbox, &mode)?;
                    running.push((next, firecracker, Instant::now()));
                    next += 1;
                }
                std::thread::sleep(Duration::from_millis(100));
                if let Some(problem) = out_of_memory(&hosts) {
                    // Stopped by its own word, not the OOM killer's, which took the run's every
                    // process with it, and an A/B's next rounds too (TASK-330.4: LangSmith's pin's
                    // daemon at 5 GB, TASK-317's growth).
                    tally.failed.push(problem);
                    for (_, firecracker, _) in running {
                        firecracker.kill()?;
                    }
                    say_failures(&tally.failed, measured);
                    bail!("the run's memory ran out: stopped");
                }
                let mut still = Vec::new();
                for (at, mut firecracker, booted) in running {
                    match firecracker.running()? {
                        true => still.push((at, firecracker, booted)),
                        false => finish(firecracker, &sandboxes[at], booted, &mut tally)?,
                    }
                }
                running = still;
            }
            let ran = started.elapsed().as_secs_f64();
            eprintln!("sandboxes: the VMs ran in {ran:.1} s");
            measured.lower("sandboxes.vms", "s", ran);
            for (role, longest) in &tally.longest {
                measured.lower(format!("sandboxes.vm.{role}"), "s", *longest);
            }
            for (command, slowest) in &tally.slowest {
                measured.lower(format!("sandboxes.command.{command}"), "ms", *slowest as f64);
            }
            // Their stats files, written within a poll of the last change.
            std::thread::sleep(Duration::from_secs(2));
            let (mut guests, mut stores, mut counted) = (0, 0, true);
            let (mut framings, mut costs) = (Vec::new(), Vec::new());
            for host in &mut hosts {
                let log = std::fs::read_to_string(dir.join(format!("{}.log", host.name)))?;
                let let_go: Vec<&str> =
                    log.lines().filter(|line| line.contains("letting a VM process go")).collect();
                if !let_go.is_empty() {
                    tally
                        .failed
                        .push(format!("let go: {} let VM processes go: {let_go:#?}", host.name));
                }
                let slow = slow_framings(&log);
                let slowest = slow.iter().copied().fold(0.0, f64::max);
                let over = slow.iter().filter(|&&ms| ms > FRAMING_BUDGET_MS).count();
                eprintln!(
                    "sandboxes: {}: slow_framings={} slowest_ms={slowest:.1} sum_ms={:.0} over_budget={over}",
                    host.name,
                    slow.len(),
                    slow.iter().sum::<f64>(),
                );
                if over > 0 {
                    tally.failed.push(format!(
                        "framings: {} framed {over} segments slower than {FRAMING_BUDGET_MS} ms, the \
                         slowest {slowest:.1} ms",
                        host.name
                    ));
                }
                framings.extend(slow);
                match host_writes(dir, host.name) {
                    Ok((raw, written)) => (guests, stores) = (guests + raw, stores + written),
                    Err(err) => {
                        counted = false;
                        tally.failed.push(format!("writes: {}'s daemon: {err:#}", host.name));
                    }
                }
                crate::common::say_daemon_stats(test_setup, host.name);
                // What its kernel counted of it, as any pin's daemon is counted (TASK-330.4).
                let cost = ProcessCosts::of(host.daemon.id())?;
                eprintln!("sandboxes: {}: {cost:?}", host.name);
                costs.push(cost);
                host.daemon.kill()?;
                host.daemon.wait()?;
            }
            measured.lower("sandboxes.framings.slow", "count", framings.len() as f64);
            let slowest = framings.iter().copied().fold(0.0, f64::max);
            measured.lower("sandboxes.framings.slowest", "ms", slowest);
            measured.lower("sandboxes.framings.sum", "s", framings.iter().sum::<f64>() / 1000.0);
            let disk_written: u64 = costs.iter().map(|cost| cost.disk_written).sum();
            measured.costs(
                "sandboxes.daemons",
                ProcessCosts {
                    disk_written,
                    peak_rss: costs.iter().map(|cost| cost.peak_rss).max().unwrap_or(0),
                    cpu_secs: costs.iter().map(|cost| cost.cpu_secs).sum(),
                },
            );
            if counted {
                let ratio = stores as f64 / guests.max(1) as f64;
                eprintln!("sandboxes: the stores wrote {ratio:.2} times what the guests did");
                measured.higher("sandboxes.guests.written", "GiB", guests as f64 / GIB as f64);
                measured.lower("sandboxes.stores.per_guest_byte", "x", ratio);
                let disk = disk_written as f64 / guests.max(1) as f64;
                measured.lower("sandboxes.daemons.disk_per_guest_byte", "x", disk);
                if ratio > WRITES_BUDGET {
                    tally.failed.push(format!(
                        "writes: the stores wrote {ratio:.2} times what the guests did, past \
                         {WRITES_BUDGET}"
                    ));
                }
            }
            say_failures(&tally.failed, measured);
            let failed = tally.failed.len();
            ensure!(failed == 0, "{failed} checks failed");
            // Passed: its stores, caches and image go, gigabytes of them; its logs stay.
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let bulky =
                    [".root", ".data", ".base", "-cache"].iter().any(|end| name.ends_with(end))
                        || ["sandbox-image", "rootfs"].contains(&&*name);
                if bulky && path.is_dir() {
                    std::fs::remove_dir_all(&path)
                        .with_context(|| format!("removing {}", path.display()))?;
                }
            }
            Ok(())
        }
    }

    impl Test for TestFxfsGuestSandboxes {
        fn should_run(&self) -> ShouldRun {
            if std::env::var_os("FXDAX_EXTENTD_BIN").is_none() {
                return ShouldRun::No("FXDAX_EXTENTD_BIN does not name the extent daemon");
            }
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<crate::rootfs::Image> {
            Some(crate::rootfs::Image::new(CONTAINERFILE))
        }

        fn timeout_secs(&self) -> u64 {
            mode().map_or(1200, |mode| mode.timeout_secs)
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            // What it measured, and why it failed, for an A/B of two pins (TASK-330.4): its
            // checks', or else the error it stopped at.
            let mut measured = Measured::new(&test_setup.test_case);
            let ran = self.run(&test_setup, &mut measured);
            if let Err(err) = &ran {
                eprintln!("{err:#}");
                if !measured.has_failed() {
                    measured.failed("error", format!("{err:#}"));
                }
            }
            if let Err(err) = measured.save() {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(if ran.is_ok() { 0 } else { 1 });
        }

        fn check(self: Box<Self>, _stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            TestOutcome::Pass
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// TASK-330.1: each round of the roles has each on a host, the hosts the other way round
        /// from the round before, so that every host serves every role in a short run's eight;
        /// and the rounds' own stores alternate, mapped through a DAX window and not.
        #[test]
        fn every_host_serves_every_role_with_a_window_and_without() {
            let sandboxes: Vec<Sandbox> = (0..8).map(Sandbox::nth).collect();
            for host in 0..2 {
                let mut roles: Vec<&str> = (sandboxes.iter())
                    .filter(|sandbox| sandbox.host == host)
                    .map(|sandbox| sandbox.role.name())
                    .collect();
                roles.sort_unstable();
                assert_eq!(roles, ["builder", "git", "pgbench", "writer"], "host {host}");
            }
            let windows: Vec<bool> = sandboxes.iter().map(|sandbox| sandbox.dax).collect();
            assert_eq!(windows, [true, true, true, true, false, false, false, false]);
            assert_eq!(sandboxes[5].name(), "sandbox-5-pgbench");
        }

        /// What a guest says of its commands is each command's runs and slowest, and a line of
        /// another shape is no report.
        #[test]
        fn a_guests_commands_line_reads_back() {
            let said = "OK wrote 1\ncommands: echo=12/31 uname=12/4 cat=12/1200\nOK sandbox git\n";
            let read = commands(said).unwrap();
            assert_eq!(
                read,
                [
                    ("echo".to_owned(), 12, 31),
                    ("uname".to_owned(), 12, 4),
                    ("cat".to_owned(), 12, 1200)
                ]
            );
            assert_eq!(commands("commands: echo=12\n"), None);
            assert_eq!(commands("OK sandbox git\n"), None);
        }

        /// The framings a daemon's log warns of, each's milliseconds, as extent_store's timings
        /// say them.
        #[test]
        fn a_daemons_slow_framings_read_back() {
            let log = "\
WARN extent_store::timings: extent store: framing SegmentId { branch: 3670381491063303249, number: 28 } took 497.0 ms (own 0.0, write 26.6, sync 28.6, dirs 441.8 ms)
WARN extent_store::timings: extent store: branch 0x32efd08504ff2c51's flush of 10 commits took 407.4 ms (wait 0.0)
WARN extent_store::timings: extent store: framing SegmentId { branch: 1, number: 2 } took 1291.8 ms (own 0.0, write 110.1, sync 1175.0, dirs 6.7 ms)
";
            assert_eq!(slow_framings(log), [497.0, 1291.8]);
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::fs;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";

    /// `var`, a count, or `default`.
    fn count(var: &str, default: u64) -> u64 {
        std::env::var(var).map_or(default, |value| value.parse().unwrap())
    }

    /// Runs [`COMMANDS`] every second until `stop`, each timed from its start to its exit; returns
    /// each's runs and its slowest, in ms.
    fn run_commands(stop: Arc<AtomicBool>) -> Vec<(&'static str, u64, u64)> {
        let mut timed: Vec<(&str, u64, u64)> =
            COMMANDS.iter().map(|(name, _)| (*name, 0, 0)).collect();
        while !stop.load(Ordering::Relaxed) {
            for ((_, argv), (_, runs, slowest)) in COMMANDS.iter().zip(&mut timed) {
                let began = Instant::now();
                let status = Command::new(argv[0])
                    .args(&argv[1..])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let took = began.elapsed().as_millis() as u64;
                assert!(
                    status.as_ref().is_ok_and(|status| status.success()),
                    "{argv:?}: {status:?}"
                );
                *runs += 1;
                *slowest = (*slowest).max(took);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        timed
    }

    /// Runs `git` with `args` in `dir`, as a sandbox's user, and returns its standard output; it
    /// must succeed. Its automatic gc, which a commit past so many loose objects starts, runs as
    /// the commit does, not detached: detached, it still held gc's lock as the role's own `git gc`
    /// ran, which failed ("gc is already running", the long mode's 100 commits of 3,000 files,
    /// TASK-330.4's A/B).
    fn git(dir: &str, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=sandbox", "-c", "user.email=sandbox@fxdax"])
            .args(["-c", "gc.autoDetach=false", "-C", dir])
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// A source file's text, of `lines` lines, from `seed`: words a program has, which compress, as
    /// a repository's do.
    fn source(seed: u64, lines: u64) -> String {
        const WORDS: [&str; 16] = [
            "fn", "let", "match", "self", "return", "struct", "impl", "Some", "None", "Ok", "Err",
            "value", "count", "index", "buffer", "length",
        ];
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut text = String::new();
        for _ in 0..lines {
            for _ in 0..8 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                text.push_str(WORDS[(state % WORDS.len() as u64) as usize]);
                text.push(' ');
            }
            text.push('\n');
        }
        text
    }

    /// The git role: a repository of FXDAX_SANDBOX_GIT_FILES files on the VM's own store, made
    /// over FXDAX_SANDBOX_GIT_COMMITS commits, each changing a tenth of its files and adding one;
    /// packed; cloned, not by links, and the clone checked whole.
    fn git_role() {
        let (files, commits) =
            (count("FXDAX_SANDBOX_GIT_FILES", 500), count("FXDAX_SANDBOX_GIT_COMMITS", 20));
        mount_data();
        let repo = format!("{MNT}/repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        let path = |file: u64| format!("{repo}/src/d{}/f{file}.rs", file % 32);
        for commit in 0..commits {
            let changed: Vec<u64> = match commit {
                0 => (0..files).collect(),
                _ => (0..files / 10).map(|at| (at * 7 + commit * 13) % files).collect(),
            };
            for file in changed.into_iter().chain([files + commit]) {
                let path = path(file);
                fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
                fs::write(&path, source(file * 1_000 + commit, 40)).unwrap();
            }
            git(&repo, &["add", "-A"]);
            git(&repo, &["commit", "-q", "-m", &format!("commit {commit}")]);
        }
        git(&repo, &["gc", "-q"]);
        let clone = format!("{MNT}/clone");
        git(MNT, &["clone", "-q", "--no-local", &repo, &clone]);
        git(&clone, &["fsck", "--strict", "--no-progress"]);
        assert_eq!(git(&clone, &["rev-list", "--count", "HEAD"]).trim(), commits.to_string());
        assert_eq!(git(&clone, &["rev-parse", "HEAD"]), git(&repo, &["rev-parse", "HEAD"]));
        assert_eq!(git(&clone, &["status", "--porcelain"]), "", "the clone's checkout");
        fs::remove_dir_all(&clone).unwrap();
        syncfs();
        unmount_data();
        println!("OK git {commits} commits of {files} files");
    }

    /// Mounts the VM's own store's Fxfs at [`MNT`], made as it mounts.
    fn mount_data() {
        fs::create_dir_all(MNT).unwrap();
        let (source, target, fstype) = (c"extents", c"/mnt/fxfs", c"fxfs");
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount_data() {
        let ret = unsafe { libc::umount(c"/mnt/fxfs".as_ptr()) };
        assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
    }

    fn syncfs() {
        let dir = fs::File::open(MNT).unwrap();
        assert_eq!(unsafe { libc::syncfs(std::os::fd::AsRawFd::as_raw_fd(&dir)) }, 0, "syncfs");
    }

    impl Test for TestFxfsGuestSandboxes {
        fn in_guest(self: Box<Self>) {
            let role = std::env::var("FXDAX_SANDBOX_ROLE").unwrap();
            let role = Role::named(&role).unwrap_or_else(|| panic!("no role {role}"));
            let stop = Arc::new(AtomicBool::new(false));
            let commands = std::thread::spawn({
                let stop = stop.clone();
                move || run_commands(stop)
            });
            // Each role as its own case's guest does it, on this root, a fork of the host's base.
            match role {
                Role::Builder => {
                    Box::new(crate::test_fxfs_guest_docker::TestFxfsGuestDocker { build: true })
                        .in_guest()
                }
                Role::Pgbench => {
                    Box::new(crate::test_fxfs_guest_pgbench::TestFxfsGuestPgbench::on(
                        crate::test_fxfs_guest_pgbench::Storage::Fxfs,
                    ))
                    .in_guest()
                }
                Role::Git => git_role(),
                Role::Writer => {
                    Box::new(crate::test_fxfs_guest_shared_disk::TestFxfsGuestSharedDisk {
                        dax: false,
                    })
                    .in_guest()
                }
            }
            stop.store(true, Ordering::Relaxed);
            let timed = commands.join().unwrap();
            let timed: Vec<String> = timed
                .iter()
                .map(|(name, runs, slowest)| format!("{name}={runs}/{slowest}"))
                .collect();
            println!("commands: {}", timed.join(" "));
            println!("OK sandbox {}", role.name());
        }
    }
}
