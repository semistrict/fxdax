// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: fxfs-guest-soak (TASK-67): hours of mixed load on forks of one base, through crashes of
//! every kind doc-12 names, each round checked against it. Each of the runner's rounds
//! (FXDAX_E2E_ROUNDS) is a round of the soak, which goes on from where the last left it: a bucket,
//! a host's stores and cache, and a ledger of what the guests were told, kept outside the results
//! (`Paths`), which the first round makes.
//!
//! A round, as the run's seed and its number say (`crate::soak::Plan`): a daemon of the soak's
//! own, its bucket answering late or an upload failing; a collector (fxdax-gc) collecting the
//! bucket throughout; five VMs at once, each on a fork of the base, which holds files and a pgbench
//! database, a VM's store forked anew now and then; each VM a writer, fsyncing file after file and
//! syncing the filesystem every so often, saying what returned; or fsx-style scripts against their
//! model; or pgbench. VMs are killed mid-write and booted again; the daemon is killed, started
//! again, and its VMs booted again; the host is lost, its stores and cache taken away, and every
//! branch taken again from the bucket by a daemon of a later placement; the cache is lost.
//!
//! Then a daemon with no faults, and a VM on every store, which checks it: every file a writer was
//! told of is there whole (a prefix of each writer's fsyncs where the daemon died or the host was
//! lost, with every file a syncfs was told of: `crate::soak::kept`), every fork reads its base,
//! fsx's files are their model's, the database is consistent; the collector lists every live
//! branch. The round's stores, cache and bucket are measured, and each daemon's peak memory, said
//! on a line of the log (`soak-round {...}`), and a measure grown past twice the warm rounds' most
//! fails the round (`crate::soak::Bound`).
//!
//! FXDAX_SEED replays a run, round by round: its first round's seed, which the run's outcomes say;
//! FXDAX_SOAK_FAULTS leaves faults out (`crate::soak::Fault`'s names, or `none`);
//! FXDAX_SOAK_LOAD_SECS fixes the loads' time; FXDAX_SOAK_STATE is where runs keep their state,
//! /var/tmp/fxdax-soak by default: on disk, as /tmp may be memory (tmpfs, as on GCE's Ubuntu 26.04,
//! where the soak's stores and cache took the host's RAM, and their measures said nothing of disk). The bucket is FXDAX_EXTENTD_REMOTE's, under
//! a prefix of the run's, or a file bucket in the state. The daemons fail uploads only if built
//! with failpoints (scripts/test-e2e.sh's FXDAX_EXTENTD_FAILPOINTS).

use macros::{guest, host};

pub struct TestFxfsGuestSoak;

/// The stores' tag, and where guests mount them.
#[cfg_attr(feature = "host", allow(dead_code))]
const TAG: &str = "extents";
#[cfg_attr(feature = "host", allow(dead_code))]
const MNT: &str = "/mnt/fxfs";

/// What a VM does: makes the base (`base`), runs its load (`load`), or checks its store (`check`).
const ROLE: &str = "FXDAX_SOAK_ROLE";
const SLOT: &str = "FXDAX_SOAK_SLOT";
const LOAD: &str = "FXDAX_SOAK_LOAD";
/// The run directory a writer's VM writes in.
const RUN: &str = "FXDAX_SOAK_RUN";
const SECS: &str = "FXDAX_SOAK_SECS";
/// The seed its load is drawn from.
const VM_SEED: &str = "FXDAX_SOAK_VM_SEED";
/// The run's seed, which the base's files are of.
const BASE_SEED: &str = "FXDAX_SOAK_BASE_SEED";

/// pgbench's database's scale, made in the base.
#[cfg_attr(feature = "host", allow(dead_code))]
const SCALE: u64 = 4;

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::script::Failure;
    use crate::soak::{self, Acks, Bound, Fault, Found, KEPT_RUNS, Load, Plan, SLOTS, WRITERS};
    use crate::vm::{self, Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::{Context, bail, ensure};
    use serde::{Deserialize, Serialize};
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::CString;
    use std::path::{Path, PathBuf};
    use std::process::Child;
    use std::time::{Duration, Instant};

    /// Each store's capacity, and its VM's DAX window, memory and vCPUs.
    const CAPACITY: u64 = 4 << 30;
    const WINDOW: crate::common::Window = crate::common::Window::Large;
    const MEMORY_MIB: u32 = 1024;
    const VCPUS: u8 = 2;
    /// The memory each daemon's kernel takes at most.
    const EXTENTD_MEMORY: u64 = 4 << 30;
    /// How long past its load a VM may run before it is hung.
    const GRACE: Duration = Duration::from_secs(120);
    /// How long a daemon may take to stop on SIGTERM, and its VMs to exit once it is killed.
    const STOP: Duration = Duration::from_secs(120);
    const LOST: Duration = Duration::from_secs(15);
    /// The longest a checking VM runs: pgbench's database recovered and checked, every file read.
    const CHECK: Duration = Duration::from_secs(600);
    /// The base's VM's: pgbench's database made.
    const BASE: Duration = Duration::from_secs(600);
    /// The shortest load a VM is booted again for.
    const REBOOT_LEFT: u64 = 30;
    /// The rounds whose measures later rounds' are held to, and how much more than twice they may
    /// grow: past those, the stores, the bucket or a daemon's memory grow with the soak's age.
    const STORES: Bound = Bound { warm: (3, 6), slack: 1 << 30 };
    const BUCKET: Bound = Bound { warm: (3, 6), slack: 2 << 30 };
    const MEMORY: Bound = Bound { warm: (3, 6), slack: 512 << 20 };
    /// The least free space the state's filesystem may be left with: the cache leaves 4 GiB.
    const FREE: u64 = 2 << 30;

    /// The soak's ledger across rounds: what guests were told, and what was measured.
    #[derive(Default, Serialize, Deserialize)]
    struct Ledger {
        seed: u64,
        /// The rounds done.
        rounds: u64,
        /// The placement's fencing token, which each lost host's successor's exceeds.
        token: u64,
        /// The base's branch, once made.
        base: Option<u64>,
        slots: Vec<SlotState>,
        measures: Vec<Measure>,
    }

    /// A VM's store, as the ledger has it.
    #[derive(Clone, Default, Serialize, Deserialize)]
    struct SlotState {
        /// How many times it was forked again.
        generation: u32,
        branch: Option<u64>,
        /// Its writers' runs, by name.
        runs: BTreeMap<String, RunState>,
        /// The fsx script whose files and models it holds, synced, if any.
        fsx: Option<u64>,
    }

    /// A writer VM's run: the last of each writer's files kept for good, if any, and whether a
    /// writer since may have dropped it.
    #[derive(Clone, Default, Serialize, Deserialize)]
    struct RunState {
        kept: [Option<u32>; WRITERS as usize],
        dropping: bool,
    }

    /// What a round measured once checked.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct Measure {
        round: u64,
        /// Bytes the host's stores' directories take, and its cache's.
        stores: u64,
        cache: u64,
        /// Bytes of the bucket's objects.
        bucket: u64,
        /// The peak resident memory of the round's loaded daemons, the most of them, and of the
        /// checking one.
        extentd_peak: u64,
        check_extentd_peak: u64,
        /// The state's filesystem's free bytes.
        free: u64,
        /// How long the round took.
        secs: u64,
    }

    /// Where the soak keeps its state, and what it runs.
    struct Paths {
        ledger: PathBuf,
        stores: PathBuf,
        cache: PathBuf,
        bucket: String,
        bucket_dir: Option<PathBuf>,
        state: PathBuf,
        /// The daemons' socket: short, as a socket's path is.
        socket: PathBuf,
        gc: PathBuf,
    }

    impl Paths {
        fn new(sockets: &Path) -> anyhow::Result<Self> {
            let run = std::env::var("FXDAX_E2E_RUN").context(
                "FXDAX_E2E_RUN names no run (scripts/test-e2e.sh's): the soak keeps its state by it",
            )?;
            let root = std::env::var_os("FXDAX_SOAK_STATE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/var/tmp/fxdax-soak"));
            let key = format!("{:016x}", soak::fnv(run.as_bytes()));
            let state = root.join(&key);
            let host = state.join("host");
            let (bucket, bucket_dir) = match std::env::var("FXDAX_EXTENTD_REMOTE") {
                Ok(remote) if !remote.is_empty() => {
                    (format!("{}/soak-{key}", remote.trim_end_matches('/')), None)
                }
                _ => {
                    let dir = state.join("bucket");
                    (format!("file://{}/", dir.display()), Some(dir))
                }
            };
            let gc = std::env::var_os("FXDAX_GC_BIN")
                .context("FXDAX_GC_BIN names no fxdax-gc (scripts/test-e2e.sh's)")?;
            Ok(Self {
                ledger: state.join("ledger.json"),
                stores: host.join("stores"),
                cache: host.join("cache"),
                state,
                bucket,
                bucket_dir,
                socket: sockets.join("extentd.sock"),
                gc: gc.into(),
            })
        }

        fn base(&self) -> PathBuf {
            self.stores.join("base")
        }

        fn slot(&self, slot: usize) -> PathBuf {
            self.stores.join(format!("slot-{slot}"))
        }
    }

    /// A daemon of the soak's.
    struct Daemon {
        child: Child,
    }

    impl Daemon {
        /// Its peak resident memory so far, in bytes.
        fn peak(&self) -> anyhow::Result<u64> {
            let status = std::fs::read_to_string(format!("/proc/{}/status", self.child.id()))?;
            let line = (status.lines())
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .context("no VmHWM in the daemon's status")?;
            let kib: u64 = line.trim().trim_end_matches("kB").trim().parse()?;
            Ok(kib << 10)
        }

        /// Kills it, as its host's crash would.
        fn kill(mut self) -> anyhow::Result<()> {
            self.child.kill()?;
            self.child.wait()?;
            Ok(())
        }

        /// Stops it as an orchestrator does (SIGTERM), within [`STOP`].
        fn stop(mut self) -> anyhow::Result<()> {
            // SAFETY: A plain system call, of a child of this process's.
            ensure!(unsafe { nix::libc::kill(self.child.id() as i32, nix::libc::SIGTERM) } == 0);
            let deadline = Instant::now() + STOP;
            loop {
                if let Some(status) = self.child.try_wait()? {
                    ensure!(status.success(), "the daemon stopped with {status}");
                    return Ok(());
                }
                if Instant::now() > deadline {
                    self.child.kill()?;
                    bail!("the daemon didn't stop within {STOP:?} of SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// A VM of the round's load, booted on its slot's store.
    struct Boot {
        slot: usize,
        name: String,
        load: Load,
        /// A writer's run's name.
        run: Option<String>,
        /// When it booted, from the round's start.
        at: Duration,
        firecracker: Option<Firecracker>,
        ended: Option<Ended>,
    }

    /// How a VM ended.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Ended {
        /// Its process exited of itself, with this code (none: a signal's).
        Exited(Option<i32>),
        /// The soak killed it.
        Killed,
    }

    /// The exit code of a VM process whose daemon went (Firecracker's `ExtentdLost`).
    const EXTENTD_LOST: i32 = 158;

    /// What a round's load came to.
    #[derive(Default)]
    struct Happened {
        /// When the daemon was killed, or the host lost: what a run begun before may lose.
        losses: Vec<Duration>,
        host_lost: bool,
        /// The peak memory of the round's loaded daemons, the most.
        peak: u64,
    }

    impl Happened {
        /// Whether what a VM booted at `at` was told may have gone with a daemon or a host.
        fn lossy(&self, at: Duration) -> bool {
            self.losses.iter().any(|&loss| loss >= at)
        }
    }

    /// What a VM checking a store found.
    #[derive(Default)]
    struct Checked {
        /// Each run's writers' files, by run and writer.
        runs: BTreeMap<(String, u8), Found>,
        lines: Vec<String>,
    }

    impl Checked {
        fn parse(output: &str) -> anyhow::Result<Self> {
            let mut checked = Checked::default();
            for line in output.lines() {
                let mut words = line.split(' ');
                match (words.next(), words.next(), words.next(), words.next()) {
                    (Some(what @ ("have" | "torn")), Some(run), Some(writer), Some(ranges)) => {
                        let found =
                            checked.runs.entry((run.to_owned(), writer.parse()?)).or_default();
                        let indices = soak::parse_ranges(ranges).map_err(anyhow::Error::msg)?;
                        match what {
                            "have" => found.whole = indices,
                            _ => found.torn = indices,
                        }
                    }
                    _ => checked.lines.push(line.to_owned()),
                }
            }
            Ok(checked)
        }

        fn said(&self, line: &str) -> bool {
            self.lines.iter().any(|said| said == line)
        }

        /// What run `run` holds of writer `writer`'s files.
        fn found(&self, run: &str, writer: u8) -> Found {
            self.runs.get(&(run.to_owned(), writer)).cloned().unwrap_or_default()
        }
    }

    struct Soak<'a> {
        setup: &'a TestSetup,
        root: PathBuf,
        paths: Paths,
        ledger: Ledger,
        plan: Plan,
        /// How many daemons the round started, for their logs' names.
        daemons: usize,
        started: Instant,
    }

    impl<'a> Soak<'a> {
        fn new(setup: &'a TestSetup, sockets: &Path) -> anyhow::Result<Self> {
            let paths = Paths::new(sockets)?;
            std::fs::create_dir_all(&paths.stores)?;
            if let Some(dir) = &paths.bucket_dir {
                std::fs::create_dir_all(dir)?;
            }
            let ledger = match std::fs::read(&paths.ledger) {
                Ok(bytes) => serde_json::from_slice(&bytes).context("the soak's ledger")?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let seed = match std::env::var("FXDAX_SEED") {
                        Ok(seed) => seed.parse().context("FXDAX_SEED is a number")?,
                        Err(_) => rand_seed(),
                    };
                    let slots = vec![SlotState::default(); SLOTS];
                    Ledger { seed, token: 1, slots, ..Default::default() }
                }
                Err(err) => return Err(err.into()),
            };
            std::fs::write(setup.tmp_dir.join("seed"), ledger.seed.to_string())?;
            let faults =
                Fault::parse(&std::env::var("FXDAX_SOAK_FAULTS").unwrap_or_else(|_| "all".into()))
                    .map_err(anyhow::Error::msg)?;
            let load_secs = match std::env::var("FXDAX_SOAK_LOAD_SECS") {
                Ok(secs) => Some(secs.parse().context("FXDAX_SOAK_LOAD_SECS is seconds")?),
                Err(_) => None,
            };
            let mut plan = Plan::new(ledger.seed, ledger.rounds + 1, &faults, load_secs);
            // A daemon without failpoints would fail no upload, and the round say it did.
            if std::env::var_os("FXDAX_EXTENTD_FAILPOINTS").is_none() {
                plan.upload_fails_after = None;
            }
            // Latency is added to every bucket the plan names: a file bucket's reads take it since
            // TASK-208, and a real bucket's crashed VM's store, draining its uploads, refuses the VM
            // booted again no more since TASK-216.
            Ok(Self {
                root: setup_rootfs(setup)?,
                setup,
                paths,
                ledger,
                plan,
                daemons: 0,
                started: Instant::now(),
            })
        }

        fn say(&self, what: impl std::fmt::Display) {
            eprintln!("soak: {:>7.1?}: {what}", self.started.elapsed());
        }

        /// A daemon of the soak's, at the ledger's token, with the round's faults if `faulty`.
        fn daemon(&mut self, faulty: bool) -> anyhow::Result<Daemon> {
            for path in [&self.paths.socket, &self.paths.socket.with_extension("control")] {
                if let Err(err) = std::fs::remove_file(path)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(err.into());
                }
            }
            self.daemons += 1;
            let token = self.ledger.token.to_string();
            let latency = if faulty { self.plan.latency_ms } else { 0 }.to_string();
            let failpoints = if faulty { self.plan.failpoints() } else { None }.unwrap_or_default();
            let env = [
                ("FXDAX_EXTENTD_REMOTE", self.paths.bucket.as_ref()),
                ("FXDAX_EXTENTD_TOKEN", token.as_ref()),
                ("FXDAX_EXTENTD_CACHE", self.paths.cache.as_os_str()),
                ("FXDAX_REMOTE_LATENCY_MS", latency.as_ref()),
                ("FAILPOINTS", failpoints.as_ref()),
            ];
            let name = format!("extentd-{}", self.daemons);
            let child =
                spawn_daemon(self.setup, &self.paths.socket, &name, &env, Some(EXTENTD_MEMORY))?;
            self.say(format_args!("{name} (pid {}) serves, token {token}", child.id()));
            Ok(Daemon { child })
        }

        /// The collector, collecting the bucket every 15 s, as another host's would.
        fn collector(&self) -> anyhow::Result<Child> {
            let log = std::fs::File::create(self.setup.tmp_dir.join("gc.log"))?;
            let child = std::process::Command::new(&self.paths.gc)
                .args([&self.paths.bucket, "--every", "15", "--grace", "10"])
                .stderr(log)
                .spawn()
                .context("starting fxdax-gc")?;
            self.setup.register_cleanup_pid(child.id());
            Ok(child)
        }

        /// The live branches the collector finds in the bucket.
        fn live_branches(&self) -> anyhow::Result<BTreeSet<u64>> {
            let output = std::process::Command::new(&self.paths.gc)
                .args([&self.paths.bucket, "--branches"])
                .output()?;
            ensure!(
                output.status.success(),
                "fxdax-gc --branches: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            (String::from_utf8(output.stdout)?.lines())
                .filter_map(|line| line.split_whitespace().next())
                .map(|branch| u64::from_str_radix(branch, 16).context("a branch"))
                .collect()
        }

        /// Boots a VM named `name` on the store at `store`, its guest told `env`.
        fn boot(&self, name: &str, store: &Path, env: &[String]) -> anyhow::Result<Firecracker> {
            let mut vm = Vm::new(VCPUS, MEMORY_MIB)?;
            vm.guest_kernel()?;
            vm.extent_store_served(
                TAG,
                store,
                CAPACITY,
                crate::common::dax_window(WINDOW)?,
                &self.paths.socket,
            );
            for var in env {
                vm.env(&CString::new(var.as_str())?);
            }
            vm.root_image(&format!("{name}.rootfs.ext4"));
            vm.spawn_api(&self.root, &self.setup.test_case, name, &Vm::api_socket(name))
        }

        /// What VM `name` said, once it ended: the lines its guest finished. Its kernel may not
        /// have complained.
        fn said(&self, name: &str) -> anyhow::Result<String> {
            let console = self.setup.tmp_dir.join(format!("{name}.console.txt"));
            let output = vm::without_kernel_lines(&vm::guest_stdout(&std::fs::read(&console)?));
            // Its root image goes: the round's VMs make many.
            let image = self.setup.tmp_dir.join(format!("{name}.rootfs.ext4"));
            if let Err(err) = std::fs::remove_file(&image)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err.into());
            }
            vm::complaints(&console).with_context(|| format!("VM {name}"))?;
            Ok(finished_lines(&output))
        }

        /// The round: its load, then its check and measures.
        fn round(&mut self) -> anyhow::Result<()> {
            self.say(self.plan.describe());
            let daemon = self.daemon(true)?;
            let mut collector = self.collector()?;
            if self.ledger.base.is_none() {
                self.make_base()?;
            }
            self.fork_slots()?;
            let mut happened = Happened::default();
            let mut boots = Vec::new();
            let loaded = self.load(daemon, &mut boots, &mut happened);
            let _ = collector.kill();
            let _ = collector.wait();
            if let Some(daemon) = loaded? {
                happened.peak = happened.peak.max(daemon.peak()?);
                self.say("the daemon stops");
                daemon.stop().context("the loaded daemon")?;
            }
            if happened.host_lost {
                self.say("the host's stores and cache are lost");
                remove_dir(&self.paths.stores)?;
                remove_dir(&self.paths.cache)?;
                std::fs::create_dir_all(&self.paths.stores)?;
                self.ledger.token += 1;
            } else if self.plan.cache_lost {
                self.say("the cache is lost");
                remove_dir(&self.paths.cache)?;
            }
            self.check(&boots, &happened)
        }

        /// Makes the base: its filesystem, its files and pgbench's database, on a store of its
        /// own, which every slot's store is forked from.
        fn make_base(&mut self) -> anyhow::Result<()> {
            let base = self.paths.base();
            let env = [format!("{ROLE}=base"), format!("{BASE_SEED}={}", self.ledger.seed)];
            let mut vm = self.boot("base", &base, &env)?;
            let deadline = Instant::now() + BASE;
            while vm.running()? {
                ensure!(Instant::now() < deadline, "the base's VM ran past {BASE:?}");
                std::thread::sleep(Duration::from_millis(100));
            }
            let (status, _) = vm.wait()?;
            let said = self.said("base")?;
            ensure!(
                status.success() && said.lines().any(|line| line == "base made"),
                "the base's VM exited {status}, saying {said:?}"
            );
            let control = control(&self.paths.socket)?;
            control.sync_store(&base)?;
            let branch = control.stat_store(&base)?.branch;
            self.ledger.base = Some(branch);
            self.say(format_args!("the base made: branch {branch:x}"));
            Ok(())
        }

        /// Forks each slot's store from the base where it has none, or the plan forks it again,
        /// deleting its old one, branch and all.
        fn fork_slots(&mut self) -> anyhow::Result<()> {
            let control = control(&self.paths.socket)?;
            for slot in 0..SLOTS {
                let path = self.paths.slot(slot);
                let state = &mut self.ledger.slots[slot];
                if self.plan.slots[slot].refork && state.branch.is_some() {
                    control.delete_store(&path).with_context(|| format!("deleting slot {slot}"))?;
                    *state = SlotState { generation: state.generation + 1, ..Default::default() };
                }
                if state.branch.is_none() {
                    let branch = control.fork_store(&self.paths.base(), &path)?;
                    state.branch = Some(branch);
                    let generation = state.generation;
                    self.say(format_args!(
                        "slot {slot} forked from the base: generation {generation}, branch \
                         {branch:x}"
                    ));
                }
            }
            Ok(())
        }

        /// Boots slot `slot`'s VM for its load of `secs` seconds, the round's boot `boot` of it.
        fn boot_load(&self, slot: usize, boot: u32, secs: u64) -> anyhow::Result<Boot> {
            let load = self.plan.slots[slot].load;
            let state = &self.ledger.slots[slot];
            let name = format!("s{slot}-b{boot}");
            let mut env = vec![
                format!("{ROLE}=load"),
                format!("{SLOT}={slot}"),
                format!("{LOAD}={}", load.name()),
                format!("{SECS}={secs}"),
                format!("{VM_SEED}={}", self.plan.vm_seed(slot, boot)),
                format!("{BASE_SEED}={}", self.ledger.seed),
            ];
            let run = (load == Load::Writer)
                .then(|| soak::run_name(state.generation, self.plan.round, boot));
            if let Some(run) = &run {
                env.push(format!("{RUN}={run}"));
            }
            let firecracker = self.boot(&name, &self.paths.slot(slot), &env)?;
            Ok(Boot {
                slot,
                name,
                load,
                run,
                at: self.started.elapsed(),
                firecracker: Some(firecracker),
                ended: None,
            })
        }

        /// Boots slot `slot`'s VM again, if its load has time left before `end`.
        fn reboot(&self, slot: usize, end: Duration, boots: &mut Vec<Boot>) -> anyhow::Result<()> {
            let left = end.saturating_sub(self.started.elapsed()).as_secs();
            if left >= REBOOT_LEFT {
                let boot = boots.iter().filter(|boot| boot.slot == slot).count() as u32;
                boots.push(self.boot_load(slot, boot, left)?);
            }
            Ok(())
        }

        /// The VMs' loads, through the plan's faults: returns the daemon still serving, unless the
        /// host was lost.
        fn load(
            &mut self,
            daemon: Daemon,
            boots: &mut Vec<Boot>,
            happened: &mut Happened,
        ) -> anyhow::Result<Option<Daemon>> {
            let mut daemon = daemon;
            let start = self.started.elapsed();
            let at = |secs: u64| start + Duration::from_secs(secs);
            let end = at(self.plan.load_secs);
            for slot in 0..SLOTS {
                boots.push(self.boot_load(slot, 0, self.plan.load_secs)?);
            }
            let mut crashes: Vec<(usize, Duration)> = (self.plan.slots.iter().enumerate())
                .filter_map(|(slot, plan)| Some((slot, at(plan.crash_at?))))
                .collect();
            let mut daemon_killed = self.plan.daemon_killed_at.map(at);
            let host_lost = self.plan.host_lost_at.map(at);
            loop {
                let now = self.started.elapsed();
                for boot in boots.iter_mut() {
                    if let Some(firecracker) = &mut boot.firecracker
                        && !firecracker.running()?
                    {
                        let (status, _) = boot.firecracker.take().unwrap().wait()?;
                        boot.ended = Some(Ended::Exited(status.code()));
                    }
                }
                if host_lost.is_some_and(|lost| now >= lost) {
                    self.say("the host is lost: its daemon and VMs killed");
                    happened.peak = happened.peak.max(daemon.peak()?);
                    daemon.kill()?;
                    for boot in boots.iter_mut() {
                        if let Some(firecracker) = boot.firecracker.take() {
                            firecracker.kill()?;
                            boot.ended = Some(Ended::Killed);
                        }
                    }
                    happened.losses.push(now);
                    happened.host_lost = true;
                    return Ok(None);
                }
                if daemon_killed.is_some_and(|killed| now >= killed) {
                    daemon_killed = None;
                    self.say("the daemon is killed");
                    happened.peak = happened.peak.max(daemon.peak()?);
                    daemon.kill()?;
                    happened.losses.push(now);
                    self.daemon_lost(boots)?;
                    daemon = self.daemon(true)?;
                    for slot in 0..SLOTS {
                        self.reboot(slot, end, boots)?;
                    }
                }
                let mut crashed = Vec::new();
                for &(slot, when) in &crashes {
                    let running = boots
                        .iter_mut()
                        .find(|boot| boot.slot == slot && boot.firecracker.is_some());
                    if now >= when
                        && let Some(boot) = running
                    {
                        self.say(format_args!("VM {} is killed", boot.name));
                        boot.firecracker.take().unwrap().kill()?;
                        boot.ended = Some(Ended::Killed);
                        crashed.push(slot);
                    }
                }
                crashes.retain(|&(_, when)| now < when);
                for slot in crashed {
                    self.reboot(slot, end, boots)?;
                }
                let pending = !crashes.is_empty() || daemon_killed.is_some() || host_lost.is_some();
                if boots.iter().all(|boot| boot.firecracker.is_none()) && (now >= end || !pending) {
                    return Ok(Some(daemon));
                }
                if now > end + GRACE {
                    let hung: Vec<&str> = (boots.iter())
                        .filter(|boot| boot.firecracker.is_some())
                        .map(|boot| boot.name.as_str())
                        .collect();
                    bail!("VMs {hung:?} ran on {GRACE:?} past their load's end: hung");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        /// Waits for the VMs running as their daemon died to exit, as they do, ExtentdLost.
        fn daemon_lost(&self, boots: &mut [Boot]) -> anyhow::Result<()> {
            let deadline = Instant::now() + LOST;
            for boot in boots.iter_mut() {
                let Some(firecracker) = &mut boot.firecracker else { continue };
                while firecracker.running()? {
                    if Instant::now() > deadline {
                        bail!("VM {} ran on {LOST:?} after its daemon died", boot.name);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let (status, _) = boot.firecracker.take().unwrap().wait()?;
                boot.ended = Some(Ended::Exited(status.code()));
            }
            Ok(())
        }

        /// Checks every store, as a daemon with no faults serves it, against what the round's VMs
        /// were told, and measures the round.
        fn check(&mut self, boots: &[Boot], happened: &Happened) -> anyhow::Result<()> {
            let checker = self.daemon(false)?;
            let control = control(&self.paths.socket)?;
            let token = Some(self.ledger.token);
            if happened.host_lost {
                // As another host takes what was the lost host's, from the bucket alone.
                let base = self.ledger.base.context("no base")?;
                control.create_store(&self.paths.base(), CAPACITY, Some(base), token)?;
                for slot in 0..SLOTS {
                    let branch = self.ledger.slots[slot].branch.context("a slot with no store")?;
                    control.create_store(&self.paths.slot(slot), CAPACITY, Some(branch), token)?;
                }
                self.say("every branch taken again from the bucket");
            }
            let mut failures = Failures::default();
            let uploads_stopped = self.uploads_stopped()?;
            // What the loads were told, of each slot's runs, as each boot said it.
            let mut told: Vec<Vec<(String, [Acks; WRITERS as usize], bool)>> =
                vec![Vec::new(); SLOTS];
            let mut slots = self.ledger.slots.clone();
            for boot in boots {
                let said = match self.said(&boot.name) {
                    Ok(said) => said,
                    Err(err) => {
                        failures.add(format!("{err:#}"));
                        continue;
                    }
                };
                let state = &mut slots[boot.slot];
                let lossy = happened.lossy(boot.at);
                let stopped = state.branch.is_some_and(|branch| uploads_stopped.contains(&branch));
                for problem in load_problems(boot.ended, &said, lossy, stopped) {
                    failures.add(format!("VM {} ({}): {problem}", boot.name, boot.load.name()));
                }
                if let Some(run) = &boot.run {
                    // The runs before this one but the newest the guest keeps it may drop.
                    let older = state.runs.len().saturating_sub(KEPT_RUNS);
                    for kept in state.runs.values_mut().take(older) {
                        kept.dropping = true;
                    }
                    for line in said.lines() {
                        if let Some(dropped) = line.strip_prefix("dropped ")
                            && state.runs.remove(dropped).is_none()
                        {
                            failures.add(format!(
                                "VM {} dropped {dropped}, which it had not",
                                boot.name
                            ));
                        }
                    }
                    let acks = writer_acks(&said, run);
                    state.runs.insert(run.clone(), RunState::default());
                    told[boot.slot].push((run.clone(), acks, lossy));
                }
                if boot.load == Load::Fsx {
                    state.fsx = (said.lines().find_map(|line| line.strip_prefix("fsx ok ")))
                        .and_then(|seed| seed.parse().ok());
                }
            }
            // Each slot checked by a VM of its own, all at once.
            let mut checking = Vec::new();
            for slot in 0..SLOTS {
                let name = format!("check-s{slot}");
                let env = [
                    format!("{ROLE}=check"),
                    format!("{SLOT}={slot}"),
                    format!("{BASE_SEED}={}", self.ledger.seed),
                ];
                checking.push((name.clone(), self.boot(&name, &self.paths.slot(slot), &env)?));
            }
            let deadline = Instant::now() + CHECK;
            for (slot, (name, mut vm)) in checking.into_iter().enumerate() {
                while vm.running()? {
                    if Instant::now() > deadline {
                        vm.kill()?;
                        bail!("VM {name} checked on past {CHECK:?}");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let (status, _) = vm.wait()?;
                let said = match self.said(&name) {
                    Ok(said) => said,
                    Err(err) => {
                        failures.add(format!("{err:#}"));
                        continue;
                    }
                };
                let checked = Checked::parse(&said)?;
                let state = &mut slots[slot];
                for problem in judge(state, &told[slot], &checked) {
                    failures.add(format!("slot {slot}: {problem}"));
                }
                if !status.success() || !checked.said("checked") {
                    failures.add(format!("VM {name} exited {status} unchecked, saying {said:?}"));
                }
            }
            let live = self.live_branches()?;
            let branches = (slots.iter().filter_map(|slot| slot.branch)).chain(self.ledger.base);
            for branch in branches {
                if !live.contains(&branch) {
                    failures.add(format!(
                        "the collector finds branch {branch:x} dead, which a store holds"
                    ));
                }
            }
            let measure = Measure {
                round: self.plan.round,
                stores: du(&self.paths.stores)?,
                cache: du(&self.paths.cache)?,
                bucket: self.bucket_bytes()?,
                extentd_peak: happened.peak,
                check_extentd_peak: checker.peak()?,
                free: free(&self.paths.state)?,
                secs: self.started.elapsed().as_secs(),
            };
            checker.stop().context("the checking daemon")?;
            self.ledger.measures.push(measure.clone());
            let series = |of: fn(&Measure) -> u64| -> Vec<u64> {
                self.ledger.measures.iter().map(of).collect()
            };
            let grown = [
                STORES.grown("the stores' bytes", &series(|m| m.stores)),
                BUCKET.grown("the bucket's bytes", &series(|m| m.bucket)),
                MEMORY.grown("the loaded daemons' peak memory", &series(|m| m.extentd_peak)),
                MEMORY
                    .grown("the checking daemon's peak memory", &series(|m| m.check_extentd_peak)),
            ];
            for grown in grown.into_iter().flatten() {
                failures.add(grown);
            }
            if measure.free < FREE {
                failures
                    .add(format!("{} bytes are left free on the state's filesystem", measure.free));
            }
            eprintln!(
                "soak-round {}",
                serde_json::json!({
                    "plan": self.plan.describe(),
                    "measure": measure,
                    "failures": failures.0,
                })
            );
            ensure!(failures.0.is_empty(), "{}: {}", self.plan.describe(), failures.0.join("; "));
            self.ledger.slots = slots;
            self.ledger.rounds += 1;
            std::fs::write(&self.paths.ledger, serde_json::to_vec_pretty(&self.ledger)?)?;
            Ok(())
        }

        /// The branches whose uploads the round's daemons failed, as their logs say: those stores'
        /// uploaders stopped, and their commits fail until they are attached again (remote.rs).
        fn uploads_stopped(&self) -> anyhow::Result<BTreeSet<u64>> {
            let mut stopped = BTreeSet::new();
            for daemon in 1..=self.daemons {
                let log = self.setup.tmp_dir.join(format!("extentd-{daemon}.log"));
                stopped.extend(upload_failures(&std::fs::read_to_string(&log)?));
            }
            Ok(stopped)
        }

        /// The bytes of the bucket's objects.
        fn bucket_bytes(&self) -> anyhow::Result<u64> {
            use extent_remote::bucket::Bucket as _;
            if let Some(dir) = &self.paths.bucket_dir {
                return du(dir);
            }
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let bucket = extent_remote::store::Store::from_url(&self.paths.bucket)
                .map_err(anyhow::Error::msg)?;
            let objects = runtime
                .block_on(bucket.list(""))
                .map_err(|err| anyhow::anyhow!("listing {}: {err}", self.paths.bucket))?;
            Ok(objects.iter().map(|object| object.size).sum())
        }
    }

    /// What's wrong with slot `slot`'s store, as `checked` found it, against what its ledger
    /// `state` holds it to, and the round's runs' acknowledgements, `told`, which it takes in.
    fn judge(
        state: &mut SlotState,
        told: &[(String, [Acks; WRITERS as usize], bool)],
        checked: &Checked,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        if !checked.said("base ok") {
            problems.push("the fork doesn't read its base whole".to_owned());
        }
        if !checked.said("pgbench consistent") {
            problems.push("pgbench's database isn't consistent".to_owned());
        }
        let fsx_same = checked.said("fsx same");
        if checked.lines.iter().any(|line| line.starts_with("fsx differs")) {
            problems.push("fsx's files differ from their models".to_owned());
        } else if let Some(seed) = state.fsx
            && !fsx_same
        {
            problems.push(format!("fsx's files and models of script {seed}, synced, are gone"));
        }
        let new: BTreeMap<&str, (&[Acks; WRITERS as usize], bool)> =
            told.iter().map(|(run, acks, lossy)| (run.as_str(), (acks, *lossy))).collect();
        let found_runs: BTreeSet<&String> = checked.runs.keys().map(|(run, _)| run).collect();
        for run in found_runs {
            if !state.runs.contains_key(run) {
                problems.push(format!("run {run} is there, which no VM made, or one dropped"));
            }
        }
        for (run, kept) in state.runs.iter_mut() {
            for writer in 0..WRITERS {
                let found = checked.found(run, writer);
                let w = writer as usize;
                match new.get(run.as_str()) {
                    Some((acks, lossy)) => match soak::kept(acks[w], &found, *lossy) {
                        Ok(through) => kept.kept[w] = through,
                        Err(why) => problems.push(format!("run {run} writer {writer}: {why}")),
                    },
                    None => {
                        let Some(through) = kept.kept[w] else { continue };
                        let torn: Vec<&u32> =
                            found.torn.iter().filter(|&&i| i <= through).collect();
                        let missing = (0..=through).filter(|i| !found.whole.contains(i)).count();
                        if !torn.is_empty() {
                            problems.push(format!(
                                "run {run} writer {writer}: files {torn:?} of 0-{through}, kept, \
                                 are torn"
                            ));
                        } else if missing > 0 && !kept.dropping {
                            problems.push(format!(
                                "run {run} writer {writer}: {missing} of files 0-{through}, \
                                 kept, are gone"
                            ));
                        }
                    }
                }
            }
        }
        problems
    }

    /// A round's failures, each said as it is found.
    #[derive(Default)]
    struct Failures(Vec<String>);

    impl Failures {
        fn add(&mut self, failure: String) {
            eprintln!("soak: FAILED: {failure}");
            self.0.push(failure);
        }
    }

    /// What's wrong with how a load's VM ended, `ended`, saying `said`: `lossy` if it lost its
    /// daemon or host, `stopped` if its store's uploads were failed, whose commits fail from then
    /// on, which its guest's calls see as EIO.
    fn load_problems(ended: Option<Ended>, said: &str, lossy: bool, stopped: bool) -> Vec<String> {
        let mut problems = Vec::new();
        let lines: Vec<&str> = said.lines().collect();
        if let Some(failure) = Failure::parse(said)
            && !(stopped && failure.kind == Failure::errno_kind(Some(nix::libc::EIO)))
        {
            let seed = lines.iter().rev().find_map(|line| line.strip_prefix("fsx script "));
            problems.push(format!(
                "{failure} (FXDAX_RANDOM_OPS_SEED={} replays the script alone in \
                 fxfs-guest-script-dax)",
                seed.unwrap_or("?")
            ));
        }
        for line in &lines {
            if line.starts_with("base bad") {
                problems.push(format!("the fork misreads its base: {line}"));
            }
            if line.starts_with("error ") && !stopped {
                problems.push(line.to_string());
            }
        }
        match ended {
            // A guest whose calls failed may have given up, its test panicking.
            Some(Ended::Exited(Some(0))) if !lines.contains(&"done") && !stopped => {
                problems.push(format!("its guest died: {said:?}"));
            }
            Some(Ended::Exited(Some(0)) | Ended::Killed) => {}
            Some(Ended::Exited(Some(EXTENTD_LOST))) if lossy => {}
            other => problems.push(format!("it ended {other:?}")),
        }
        problems
    }

    /// The branches a daemon's log, `log`, says it failed an upload of (FAILPOINTS'
    /// `writer::upload`).
    fn upload_failures(log: &str) -> BTreeSet<u64> {
        (log.lines())
            .filter(|line| line.contains("failpoint writer::upload"))
            .filter_map(|line| line.split("uploading branch ").nth(1)?.split(':').next())
            .filter_map(|branch| u64::from_str_radix(branch, 16).ok())
            .collect()
    }

    /// Each writer's acknowledgements of run `run`, as its VM said them: those fsynced, from 0
    /// on, and those synced whole.
    fn writer_acks(said: &str, run: &str) -> [Acks; WRITERS as usize] {
        let mut acks = [Acks::default(); WRITERS as usize];
        for line in said.lines() {
            let mut words = line.split(' ');
            let (Some(what), Some(of), Some(writer), Some(index), None) =
                (words.next(), words.next(), words.next(), words.next(), words.next())
            else {
                continue;
            };
            let (Ok(writer), Ok(index)) = (writer.parse::<usize>(), index.parse::<u32>()) else {
                continue;
            };
            if of != run || writer >= acks.len() {
                continue;
            }
            match what {
                "synced" => acks[writer].synced = Some(index),
                "durable" => acks[writer].durable = Some(index),
                _ => {}
            }
        }
        acks
    }

    fn rand_seed() -> u64 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        soak::mix(now.as_nanos() as u64, std::process::id() as u64)
    }

    /// The lines of `output` it finished: a VM killed may have cut its last short.
    fn finished_lines(output: &str) -> String {
        match output.rfind('\n') {
            Some(end) => output[..=end].to_owned(),
            None => String::new(),
        }
    }

    fn remove_dir(dir: &Path) -> anyhow::Result<()> {
        match std::fs::remove_dir_all(dir) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(anyhow::Error::new(err).context(format!("removing {}", dir.display())))
            }
            _ => Ok(()),
        }
    }

    /// The bytes the files under `dir` take on its filesystem, holes left out.
    fn du(dir: &Path) -> anyhow::Result<u64> {
        use std::os::unix::fs::MetadataExt;
        let entries = match std::fs::read_dir(dir) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            entries => entries?,
        };
        let mut bytes = 0;
        for entry in entries {
            let entry = entry?;
            // What a daemon removes as this walks goes.
            let metadata = match entry.metadata() {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                metadata => metadata?,
            };
            bytes += match metadata.is_dir() {
                true => du(&entry.path())?,
                false => metadata.blocks() * 512,
            };
        }
        Ok(bytes)
    }

    /// The bytes free on the filesystem of `path`.
    fn free(path: &Path) -> anyhow::Result<u64> {
        let stat = nix::sys::statvfs::statvfs(path)?;
        Ok(stat.blocks_available() as u64 * stat.fragment_size() as u64)
    }

    impl Test for TestFxfsGuestSoak {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<crate::rootfs::Image> {
            Some(crate::rootfs::Image::new(
                "FROM alpine:3.22
RUN apk add --no-cache postgresql postgresql-contrib
",
            ))
        }

        fn timeout_secs(&self) -> u64 {
            // The longest load, and what may run past it: the base made, the daemons stopped, the
            // checks.
            let load = std::env::var("FXDAX_SOAK_LOAD_SECS")
                .ok()
                .and_then(|secs| secs.parse().ok())
                .unwrap_or(240u64);
            load + (GRACE + STOP * 2 + CHECK + BASE).as_secs() + 300
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let sockets = tempdir::TempDir::new("fxdax-soak")?;
            let ran = Soak::new(&test_setup, sockets.path()).and_then(|mut soak| soak.round());
            match ran {
                Ok(()) => {
                    println!("OK");
                    std::process::exit(0)
                }
                Err(err) => {
                    let said = format!("{err:#}");
                    eprintln!("soak: FAILED: {said}");
                    std::fs::write(test_setup.tmp_dir.join("failure.txt"), &said)?;
                    std::process::exit(1)
                }
            }
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            if stdout == b"OK\n" {
                return TestOutcome::Pass;
            }
            match std::fs::read_to_string(test_setup.tmp_dir.join("failure.txt")) {
                Ok(said) => TestOutcome::Fail(said),
                Err(_) => TestOutcome::Fail(format!(
                    "the soak said {:?}, and no failure",
                    String::from_utf8_lossy(&stdout)
                )),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A writer's acknowledgements are its lines', of its run alone, cut lines none.
        #[test]
        fn writers_acks_are_their_lines() {
            let said = "synced r 0 0\nsynced r 1 0\nsynced r 0 1\ndurable r 0 0\nsynced q 0 7\n\
                        synced r 1 x\nsynced r 0 2 3\nerror fsync: EIO\n";
            assert_eq!(
                writer_acks(said, "r"),
                [
                    Acks { synced: Some(1), durable: Some(0) },
                    Acks { synced: Some(0), durable: None }
                ]
            );
            assert_eq!(finished_lines("a\nb\nsynced r 0 4"), "a\nb\n");
            assert_eq!(finished_lines("cut"), "");
        }

        /// Only a store whose uploads were failed may see EIO, and its guest give up.
        #[test]
        fn loads_fail_only_where_uploads_stopped() {
            let log = "ERROR fxdax::remote: fxdax: uploading branch fb232d4739d569c0: reading \
                       failpoint writer::upload\nERROR fxdax::extents: fxdax: uploading the \
                       store's commits: reading failpoint writer::upload\n";
            assert_eq!(upload_failures(log), BTreeSet::from([0xfb23_2d47_39d5_69c0]));
            let failed = "base ok\nfsx script 7\nscript failed: operation 989: errno 5: fsync: \
                          I/O error (os error 5)\n";
            let died = Some(Ended::Exited(Some(0)));
            assert_eq!(load_problems(died, failed, false, true), Vec::<String>::new());
            let problems = load_problems(died, failed, false, false);
            assert_eq!(problems.len(), 2, "{problems:?}");
            assert!(problems[0].contains("FXDAX_RANDOM_OPS_SEED=7"), "{problems:?}");
            let differs = "fsx script 8\nscript failed: operation 3: differs: a read\n";
            assert_eq!(load_problems(died, differs, false, true).len(), 1);
            let erred = "base ok\nerror syncfs: Input/output error (os error 5)\ndone\n";
            assert_eq!(load_problems(died, erred, false, true), Vec::<String>::new());
            assert_eq!(load_problems(died, erred, false, false).len(), 1);
            let lost = Some(Ended::Exited(Some(EXTENTD_LOST)));
            assert_eq!(load_problems(lost, "base ok\n", true, false), Vec::<String>::new());
            assert_eq!(load_problems(lost, "base ok\n", false, false).len(), 1);
            assert_eq!(load_problems(Some(Ended::Killed), "", false, false), Vec::<String>::new());
        }

        fn checked(lines: &str) -> Checked {
            Checked::parse(lines).unwrap()
        }

        const WHOLE: &str = "base ok\nfsx none\npgbench consistent\nchecked\n";

        /// A store holds its kept runs whole, and its new runs as their acknowledgements say;
        /// its base, its database and fsx's models as they were.
        #[test]
        fn stores_are_judged_against_the_ledger() {
            let mut state = SlotState::default();
            state.runs.insert("old".into(), RunState { kept: [Some(3), None], dropping: false });
            state.runs.insert("new".into(), RunState::default());
            let acks = |synced| Acks { synced: Some(synced), durable: None };
            let told = [("new".to_owned(), [acks(2), acks(1)], false)];
            let found = format!("have old 0 0-3\nhave new 0 0-2\nhave new 1 0-1\n{WHOLE}");
            assert_eq!(judge(&mut state, &told, &checked(&found)), Vec::<String>::new());
            assert_eq!(state.runs["new"].kept, [Some(2), Some(1)]);

            let lost = format!("have old 0 0-2\nhave new 0 0-2\nhave new 1 0-1\n{WHOLE}");
            assert_eq!(
                judge(&mut state.clone(), &told, &checked(&lost)),
                ["run old writer 0: 1 of files 0-3, kept, are gone"]
            );
            let mut dropping = state.clone();
            dropping.runs.get_mut("old").unwrap().dropping = true;
            assert_eq!(judge(&mut dropping, &[], &checked(&lost)), Vec::<String>::new());
            let torn =
                format!("have old 0 0-2\ntorn old 0 3\nhave new 0 0-2\nhave new 1 0-1\n{WHOLE}");
            assert_eq!(
                judge(&mut dropping, &[], &checked(&torn)),
                ["run old writer 0: files [3] of 0-3, kept, are torn"]
            );
            let short = format!("have new 0 0-1\nhave new 1 0-1\n{WHOLE}");
            assert_eq!(
                judge(&mut state.clone(), &told, &checked(&short)).len(),
                2,
                "old's files gone, and new's fsync of 2 lost with nothing to lose it"
            );
            let stranger = format!("have stray 0 0\n{WHOLE}");
            assert!(
                judge(&mut SlotState::default(), &[], &checked(&stranger))[0]
                    .contains("no VM made")
            );
        }

        #[test]
        fn bases_databases_and_models_are_judged() {
            let mut state = SlotState { fsx: Some(9), ..Default::default() };
            assert_eq!(
                judge(&mut state, &[], &checked("checked\n")),
                [
                    "the fork doesn't read its base whole",
                    "pgbench's database isn't consistent",
                    "fsx's files and models of script 9, synced, are gone",
                ]
            );
            let same = WHOLE.replace("fsx none", "fsx same");
            assert_eq!(judge(&mut state, &[], &checked(&same)), Vec::<String>::new());
            let differs = WHOLE.replace("fsx none", "fsx differs 2");
            assert_eq!(
                judge(&mut state, &[], &checked(&differs)),
                ["fsx's files differ from their models"]
            );
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use crate::script::{self, Op, SEVERAL_FILES};
    use crate::script_run::{Options, Run};
    use crate::soak::{self, BASE_FILES, KEPT_RUNS, Load, WRITERS};
    use nix::libc;
    use std::collections::BTreeSet;
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::process::{Command, Output};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::time::{Duration, Instant};

    /// pgbench's database, and its server's socket's directory.
    const DATA: &str = "/mnt/fxfs/pgdata";
    const SOCKET: &str = "/tmp";

    fn var(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
    }

    fn number(name: &str) -> u64 {
        var(name).parse().unwrap_or_else(|_| panic!("{name} is not a number"))
    }

    fn path(name: &str) -> String {
        format!("{MNT}/{name}")
    }

    fn mount() {
        fs::create_dir_all(MNT).unwrap();
        let (source, target, fstype) =
            (CString::new(TAG).unwrap(), CString::new(MNT).unwrap(), CString::new("fxfs").unwrap());
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() -> std::io::Result<()> {
        let target = CString::new(MNT).unwrap();
        match unsafe { libc::umount(target.as_ptr()) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    /// Syncs the filesystem whole: once this returns, what it holds is durable, and uploaded
    /// (doc-12).
    fn syncfs() -> std::io::Result<()> {
        let dir = fs::File::open(MNT)?;
        match unsafe { libc::syncfs(dir.as_raw_fd()) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    /// Writes `bytes` to a new file at `path`, and fsyncs it.
    fn write_synced(path: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// The base's file `index`'s path.
    fn base_file(index: u32) -> String {
        path(&format!("base/{index}"))
    }

    /// Says whether the base's files are whole, as the fork reads them.
    fn check_base(seed: u64) {
        for index in 0..BASE_FILES {
            let expected = soak::file_bytes(soak::base_key(seed, index));
            match fs::read(base_file(index)) {
                Ok(read) if read == expected => {}
                Ok(read) => {
                    println!("base bad {index}: {} bytes, not {}", read.len(), expected.len());
                    return;
                }
                Err(err) => {
                    println!("base bad {index}: {err}");
                    return;
                }
            }
        }
        println!("base ok");
    }

    /// Runs `command` as PostgreSQL's user: its output, if it succeeded; or what it said, its
    /// standard error's last lines too. Not on the console as it runs: PostgreSQL's clients say
    /// `WARNING:` as their server crashes, as it does where the store's commits fail, which the
    /// console's check takes for the kernel's complaint (`vm::complaints`, GCE r756).
    fn postgres(command: &str) -> Result<Output, String> {
        let output = Command::new("su")
            .args(["postgres", "-s", "/bin/sh", "-c", command])
            .output()
            .map_err(|err| format!("su: {err}"))?;
        if output.status.success() {
            return Ok(output);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        let last = lines[lines.len().saturating_sub(5)..].join(" | ");
        Err(format!(
            "{command}: {}: {} | {}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            last.replace("WARNING:", "warning:")
        ))
    }

    fn psql(query: &str) -> Result<String, String> {
        let output =
            postgres(&format!("psql -h {SOCKET} -v ON_ERROR_STOP=1 -At -c \"{query}\" bench"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Starts PostgreSQL on the database, recovering it from its WAL if it crashed.
    fn start_postgres() -> Result<(), String> {
        let status = Command::new("chown").args(["-R", "postgres:postgres", DATA]).status();
        if !status.is_ok_and(|status| status.success()) {
            return Err(format!("chown {DATA}"));
        }
        let options = format!("-c shared_buffers=128MB -k {SOCKET} -c listen_addresses=''");
        postgres(&format!("pg_ctl -D {DATA} -l /tmp/postgres.log -o \"{options}\" -w -t 600 start"))
            .map(drop)
    }

    /// Stops PostgreSQL, and waits for its processes to be gone: pg_ctl returns once the
    /// postmaster removed its pid file, which it does on its way out, still in the database's
    /// directory, where an unmount then found it busy (GCE r751).
    fn stop_postgres() -> Result<(), String> {
        postgres(&format!("pg_ctl -D {DATA} -w -t 600 -m fast stop"))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        while postgres_running() {
            if Instant::now() > deadline {
                return Err("PostgreSQL's processes ran on a minute after it stopped".to_owned());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// Whether a process of PostgreSQL's is running still: not a zombie, which holds no files and
    /// no directory, and which the guest agent, as init, never reaps (GCE r752).
    fn postgres_running() -> bool {
        (fs::read_dir("/proc").into_iter().flatten().flatten()).any(|entry| {
            // `PID (COMM) STATE ...`, the command between the first parenthesis and the last.
            let stat = fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
                return false;
            };
            let state = stat[close + 1..].split_whitespace().next();
            &stat[open + 1..close] == "postgres" && state != Some("Z")
        })
    }

    /// Whether the database is consistent: every account there once, each of pgbench's
    /// transactions in each of its tables' balances, which sum the same; every tuple sound, and
    /// every index holding them, as amcheck finds them.
    fn consistent() -> Result<(), String> {
        let sums = psql(
            "select (select count(*) from pgbench_accounts), \
             (select coalesce(sum(abalance), 0) from pgbench_accounts), \
             (select coalesce(sum(tbalance), 0) from pgbench_tellers), \
             (select coalesce(sum(bbalance), 0) from pgbench_branches)",
        )?;
        let sums: Vec<&str> = sums.split('|').collect();
        let accounts = (SCALE * 100_000).to_string();
        if sums.len() != 4 || sums[0] != accounts || sums[1] != sums[2] || sums[2] != sums[3] {
            return Err(format!("accounts and balances {sums:?}: not {accounts} and three alike"));
        }
        psql("create extension if not exists amcheck")?;
        let tables = "array['pgbench_accounts', 'pgbench_branches', 'pgbench_tellers', \
                      'pgbench_history']::regclass[]";
        let corrupt =
            psql(&format!("select count(*) from unnest({tables}) as t(rel), verify_heapam(rel)"))?;
        if corrupt != "0" {
            return Err(format!("verify_heapam finds {corrupt} tuples corrupt"));
        }
        psql(&format!(
            "select count(*) from (select bt_index_check(indexrelid, true) from pg_index \
             where indrelid = any({tables})) as checked"
        ))
        .map(drop)
    }

    /// The base: its files, synced, and pgbench's database, made and stopped cleanly.
    fn make_base() {
        let seed = number(BASE_SEED);
        fs::create_dir_all(path("base")).unwrap();
        for index in 0..BASE_FILES {
            write_synced(&base_file(index), &soak::file_bytes(soak::base_key(seed, index)))
                .unwrap();
        }
        fs::create_dir_all(DATA).unwrap();
        let status = Command::new("chown").args(["postgres:postgres", DATA]).status().unwrap();
        assert!(status.success(), "chown {DATA}");
        postgres(&format!("initdb -D {DATA} -A trust --data-checksums")).unwrap();
        start_postgres().unwrap();
        postgres(&format!("createdb -h {SOCKET} bench")).unwrap();
        postgres(&format!("pgbench -h {SOCKET} -i -q -s {SCALE} bench")).unwrap();
        consistent().unwrap();
        stop_postgres().unwrap();
        syncfs().unwrap();
        unmount().unwrap();
        println!("base made");
    }

    /// The writers' load: [`WRITERS`] threads creating and fsyncing files, one after another, in
    /// the run's directory, each saying `synced RUN WRITER INDEX` once its fsync returned; and a
    /// thread syncing the filesystem whole every so often, saying `durable RUN WRITER INDEX` for
    /// each writer's last file synced before, once it returned. First, the runs but the newest
    /// [`KEPT_RUNS`] go, each said `dropped RUN` once a syncfs after it returned.
    fn writers(slot: usize, run: &str, deadline: Instant, seed: u64) {
        let runs = path("w");
        fs::create_dir_all(&runs).unwrap();
        let mut names: Vec<String> = (fs::read_dir(&runs).unwrap().flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let older = names.len().saturating_sub(KEPT_RUNS);
        for name in &names[..older] {
            fs::remove_dir_all(format!("{runs}/{name}")).unwrap();
        }
        if older > 0 {
            match syncfs() {
                Ok(()) => names[..older].iter().for_each(|name| println!("dropped {name}")),
                Err(err) => println!("error syncfs: {err}"),
            }
        }
        let dir = format!("{runs}/{run}");
        fs::create_dir(&dir).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let last: Arc<Vec<AtomicI64>> =
            Arc::new((0..WRITERS).map(|_| AtomicI64::new(-1)).collect());
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let (dir, run, stop, last) =
                    (dir.clone(), run.to_owned(), stop.clone(), last.clone());
                std::thread::spawn(move || {
                    let mut rng = script::Rng::new(soak::mix(seed, writer as u64));
                    for index in 0.. {
                        if Instant::now() >= deadline || stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let bytes = soak::file_bytes(soak::file_key(slot, &run, writer, index));
                        let file = format!("{dir}/{writer}-{index}");
                        if let Err(err) = write_synced(&file, &bytes) {
                            println!("error {file}: {err}");
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                        last[writer as usize].store(index as i64, Ordering::Release);
                        println!("synced {run} {writer} {index}");
                        // Paced: a soak's writers write for hours, not as fast as they can.
                        std::thread::sleep(Duration::from_millis(rng.next(100)));
                    }
                })
            })
            .collect();
        // Syncs the filesystem, saying each writer's last file it covers.
        let sync = || {
            let covered: Vec<i64> = last.iter().map(|last| last.load(Ordering::Acquire)).collect();
            if let Err(err) = syncfs() {
                println!("error syncfs: {err}");
                stop.store(true, Ordering::Relaxed);
                return;
            }
            for (writer, &index) in covered.iter().enumerate() {
                if index >= 0 {
                    println!("durable {run} {writer} {index}");
                }
            }
        };
        let mut rng = script::Rng::new(seed);
        while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(300 + rng.next(1500)));
            sync();
        }
        for writer in writers {
            writer.join().unwrap();
        }
        if !stop.load(Ordering::Relaxed) {
            sync();
        }
    }

    /// fsx's files, and their models.
    fn fsx_file(n: usize) -> String {
        path(&format!("fsx/script-{n}"))
    }

    fn fsx_model(n: usize) -> String {
        format!("{}.model", fsx_file(n))
    }

    /// fsx's load: scripts of operations on three files, one after another, each checked against
    /// its model as it runs, until the deadline; the last, cut short there, checked whole, and
    /// its models written beside its files, synced, which a checking VM compares them with: said
    /// `fsx ok SEED` once a syncfs after them returned. What an earlier load left goes first, its
    /// models before its files, so that no files of one script are ever beside another's models.
    fn fsx(deadline: Instant, seed: u64) {
        let dir = path("fsx");
        if fs::metadata(&dir).is_ok() {
            for n in 0..SEVERAL_FILES.files {
                let _ = fs::remove_file(fsx_model(n));
            }
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir(&dir).unwrap();
        let names: Vec<String> = (0..SEVERAL_FILES.files).map(fsx_file).collect();
        let mut last = None;
        for script in 0.. {
            let seed = soak::mix(seed, script);
            println!("fsx script {seed}");
            let ops = script::generate(seed, SEVERAL_FILES);
            let mut run = Run::new(&names, Options::default());
            for (index, op) in ops.iter().enumerate() {
                if Instant::now() >= deadline {
                    break;
                }
                if let Err(failure) = run.step(index, op) {
                    crate::kernel_log::print();
                    run.fail(seed, &failure);
                }
            }
            if let Err(failure) = run.step(ops.len(), &Op::Check) {
                crate::kernel_log::print();
                run.fail(seed, &failure);
            }
            let ran = run.finish();
            last = Some((seed, ran.model));
            if Instant::now() >= deadline {
                break;
            }
        }
        let (seed, model) = last.expect("a script ran");
        for (n, held) in model.files.iter().enumerate() {
            let new = format!("{}.new", fsx_model(n));
            if let Err(err) = write_synced(&new, held).and_then(|()| fs::rename(&new, fsx_model(n)))
            {
                println!("error {new}: {err}");
                return;
            }
        }
        match syncfs() {
            Ok(()) => println!("fsx ok {seed}"),
            Err(err) => println!("error syncfs: {err}"),
        }
    }

    /// pgbench's load: the database recovered and checked, pgbench run on it till the deadline,
    /// and checked again.
    fn pgbench(deadline: Instant) {
        let ran = start_postgres().and_then(|()| consistent()).and_then(|()| {
            let secs = deadline.saturating_duration_since(Instant::now()).as_secs().max(5);
            postgres(&format!("pgbench -h {SOCKET} -c 4 -j 2 -T {secs} bench"))?;
            consistent()?;
            stop_postgres()
        });
        match ran {
            Ok(()) => println!("pgbench ok"),
            Err(err) => println!("error pgbench: {err}"),
        }
    }

    /// The load VM: its base read, then its load till its deadline, then its filesystem unmounted,
    /// said `done`.
    fn load() {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(number(SECS));
        let (slot, seed) = (number(SLOT) as usize, number(VM_SEED));
        check_base(number(BASE_SEED));
        match Load::named(&var(LOAD)).expect(LOAD) {
            Load::Writer => writers(slot, &var(RUN), deadline, seed),
            Load::Fsx => fsx(deadline, seed),
            Load::Pgbench => pgbench(deadline),
        }
        match unmount() {
            Ok(()) => println!("done"),
            Err(err) => println!("error umount: {err}"),
        }
    }

    /// Says which of each run's writers' files are whole, `have RUN WRITER RANGES`, and which
    /// there but not, `torn RUN WRITER RANGES`.
    fn check_runs(slot: usize) {
        let runs = path("w");
        let Ok(entries) = fs::read_dir(&runs) else { return };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for run in names {
            let mut found: Vec<(BTreeSet<u32>, BTreeSet<u32>)> =
                (0..WRITERS).map(|_| Default::default()).collect();
            for entry in fs::read_dir(format!("{runs}/{run}")).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let (writer, index) = name.split_once('-').expect("WRITER-INDEX");
                let (writer, index): (u8, u32) = (writer.parse().unwrap(), index.parse().unwrap());
                let expected = soak::file_bytes(soak::file_key(slot, &run, writer, index));
                let (whole, torn) = &mut found[writer as usize];
                match fs::read(entry.path()) {
                    Ok(read) if read == expected => whole.insert(index),
                    _ => torn.insert(index),
                };
            }
            for (writer, (whole, torn)) in found.iter().enumerate() {
                println!("have {run} {writer} {}", soak::ranges(whole));
                if !torn.is_empty() {
                    println!("torn {run} {writer} {}", soak::ranges(torn));
                }
            }
        }
    }

    /// Says whether fsx's files are their models': `fsx same`, `fsx differs N`, or `fsx none`
    /// where no script's models are whole.
    fn check_fsx() {
        let models: Vec<Option<Vec<u8>>> =
            (0..SEVERAL_FILES.files).map(|n| fs::read(fsx_model(n)).ok()).collect();
        if models.iter().any(Option::is_none) {
            println!("fsx none");
            return;
        }
        for (n, model) in models.into_iter().enumerate() {
            if fs::read(fsx_file(n)).ok() != model {
                println!("fsx differs {n}");
                return;
            }
        }
        println!("fsx same");
    }

    /// The checking VM: says what its store holds, its database checked, then unmounts it, said
    /// `checked`.
    fn check() {
        let slot = number(SLOT) as usize;
        check_base(number(BASE_SEED));
        check_runs(slot);
        check_fsx();
        let checked = start_postgres().and_then(|()| consistent()).and_then(|()| stop_postgres());
        match checked {
            Ok(()) => println!("pgbench consistent"),
            Err(err) => println!("pgbench inconsistent: {err}"),
        }
        let unmounted = syncfs().and_then(|()| unmount());
        match unmounted {
            Ok(()) => println!("checked"),
            Err(err) => println!("error unmounting: {err}"),
        }
    }

    impl Test for TestFxfsGuestSoak {
        fn in_guest(self: Box<Self>) {
            // Every message on the console: a task hung says where.
            fs::write("/proc/sys/kernel/printk", "7").unwrap();
            mount();
            match var(ROLE).as_str() {
                "base" => make_base(),
                "load" => load(),
                "check" => check(),
                role => panic!("no role {role}"),
            }
        }
    }
}
