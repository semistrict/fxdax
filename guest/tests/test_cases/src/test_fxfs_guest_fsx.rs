// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: fsx-style random operations on the guest's Fxfs, differential against ext4 in the same
//! guest (TASK-44.15, TASK-149.12): scripts of the wide mix (`script::WIDE`: writes, stores through
//! mappings kept, made and private, reads, truncations, punches, ranges zeroed, allocated, cloned
//! and copied, fsyncs, fdatasyncs, msyncs, O_DIRECT, page caches dropped and files evicted and read
//! in again, which on a DAX mount makes them DAX, copying on write) and the hostile one's
//! (`script::HOSTILE`, TASK-149.16: offsets and lengths at and past i64's end and the largest
//! file's, ranges onto themselves, mappings at odd offsets, MAP_FIXED over those kept, moved by
//! mremap, madvised away, truncations racing stores, writes from buffers that fault partway and
//! from the file's own mapping), seed after seed for
//! FXDAX_FSX_SECS (one script if 0), each on three files of Fxfs's and three of ext4's, a disk of
//! the guest's own, step by step, both held to the model and to each other (`differential`).
//!
//! Each script's seed is said as it starts, FXDAX_SEED replaying it with FXDAX_FSX_SECS=0. A failure
//! is shrunk in the guest, a try a run of the script, no boot, for FXDAX_FSX_SHRINK_SECS at most;
//! the host replays what it shrank to once, from a boot, to say whether that replays it. A guest
//! that died, hung or complained is shrunk by the host, a VM each try (`script_vm`), as
//! fxfs-guest-script's are. The last script's files stay in the store, with their models, which
//! the host's Fxfs reads and compares.
//!
//! fxfs-guest-fsx-dax with a DAX window, -delay-dax with delays injected at the DAX protocol's
//! racing steps (fxfs.dax_delay_us), and -faults-dax, on the debug kernel, with the Fxfs half's
//! calls failing their allocations (`script_run::Faults`): ENOMEM a refusal, its operation's
//! effects within what it could have reached, and ext4's file then made Fxfs's. Which call fails
//! which allocation is the script's seed's, by operation, so that a replay, and a shrink's tries,
//! fail the same.

use macros::{guest, host};

pub struct TestFxfsGuestFsx {
    /// Whether the device has a DAX window, and the guest maps files into it.
    pub dax: bool,
    /// Whether the guest's DAX code sleeps at its protocol's racing steps (fxfs.dax_delay_us).
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub delay: bool,
    /// Whether Fxfs's calls fail their allocations at random (a debug kernel's fault injection).
    pub faults: bool,
}

/// How long a run starts scripts for, in seconds, in the host's environment and the guest's: one
/// script if 0.
const SECS: &str = "FXDAX_FSX_SECS";
/// How long the guest may shrink a failed script for, in seconds: not at all if 0.
const SHRINK_SECS: &str = "FXDAX_FSX_SHRINK_SECS";
/// In the guest's environment, the reference's disk.
const REFERENCE: &str = "FXDAX_FSX_REFERENCE";
/// The reference's mount options: no delayed allocation, whose blocks a write that stopped short
/// left reserved past the end it reached, which an O_DIRECT read past it found delayed and failed,
/// EIO (Linux 6.18's ext4 and iomap_dio_iter; GCE r860, TASK-149.16). Allocating as it writes,
/// ext4 lets a write's blocks past what it wrote go as it ends (ext4_write_end,
/// ext4_truncate_failed_write).
#[cfg_attr(feature = "host", allow(dead_code))]
const REFERENCE_OPTIONS: &str = "nodelalloc";
const DEFAULT_SECS: u64 = 60;
const DEFAULT_SHRINK_SECS: u64 = 300;
/// What the guest says as each script starts, before its seed.
const SEED_LINE: &str = "script seed ";
/// What the guest says of a script it shrank, before the drops that leave what still fails.
const SHRUNK_LINE: &str = "shrunk: ";

/// FXDAX_FSX_SECS's or FXDAX_FSX_SHRINK_SECS's, or `default`.
fn secs(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .map_or(default, |secs| secs.parse().unwrap_or_else(|_| panic!("{var} is {secs:?}")))
}

/// A script's file `n`, and its model's.
fn file(n: usize) -> String {
    format!("fsx-{n}")
}

#[cfg_attr(feature = "guest", allow(dead_code))]
fn model(n: usize) -> String {
    format!("{}.model", file(n))
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, guest_kernel_slowness, setup_rootfs};
    use crate::script::{self, Failure, HOSTILE, kind};
    use crate::script_vm::{self, Replays};
    use crate::test_fxfs_guest_checks::host::seed;
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use futures::executor::block_on;
    use fxdax::{Filesystem, Storage};
    use std::ffi::CString;
    use std::path::Path;
    use std::time::Duration;

    /// The volumes' sizes, their images sparse, and the DAX window's.
    const IMAGE_SIZE: u64 = 256 << 20;
    const REFERENCE_SIZE: u64 = 256 << 20;
    const WINDOW: crate::common::Window = crate::common::Window::Large;
    /// What one script may take on both, on a release kernel, and a run besides its scripts.
    const SCRIPT: Duration = Duration::from_secs(120);
    const SETUP: Duration = Duration::from_secs(60);

    /// What the guest said.
    #[derive(Debug, Default, PartialEq)]
    pub(super) struct Said {
        /// The seed of the last script it started.
        pub(super) seed: Option<u64>,
        pub(super) failure: Option<Failure>,
        /// What its shrink dropped, if it shrank the failure, and the operations it kept.
        pub(super) shrunk: Option<(String, Vec<String>)>,
        /// The differences in times it saw, a line each, those not allowed.
        pub(super) unallowed: Vec<String>,
        /// Whether it said it passed.
        pub(super) ok: bool,
    }

    impl Said {
        pub(super) fn parse(output: &str) -> Self {
            let mut said = Said { failure: Failure::parse(output), ..Said::default() };
            let mut lines = output.lines().peekable();
            while let Some(line) = lines.next() {
                if let Some(seed) = line.strip_prefix(SEED_LINE) {
                    said.seed = seed.parse().ok();
                } else if let Some(drops) = line.strip_prefix(SHRUNK_LINE) {
                    let mut kept = Vec::new();
                    while let Some(op) = lines.next_if(|line| line.starts_with("  ")) {
                        kept.push(op.trim_start().to_owned());
                    }
                    said.shrunk = Some((drops.to_owned(), kept));
                } else if line.starts_with(script::TIMES) && !line.contains(script::ALLOWED) {
                    said.unallowed.push(line.to_owned());
                }
            }
            said.ok = output.lines().last() == Some("OK");
            said
        }
    }

    /// How a run in `dir`, a replay of one script, which said `stdout`, failed, if it did.
    fn verdict(dir: &Path, stdout: &[u8]) -> Option<Failure> {
        let said = Said::parse(&String::from_utf8_lossy(stdout));
        if let Some(failure) = said.failure {
            return Some(failure);
        }
        if !said.ok {
            let what = format!("it said {:?}", String::from_utf8_lossy(stdout));
            return Some(Failure { op: None, kind: kind::DIED.to_owned(), what });
        }
        crate::vm::complaints(&dir.join("console.txt")).err().map(|err| Failure {
            op: None,
            kind: kind::COMPLAINT.to_owned(),
            what: format!("{err:#}"),
        })
    }

    /// The seed of the last script the guest of the run in `dir` started, as its console has it.
    fn last_seed(dir: &Path) -> Option<u64> {
        let console = std::fs::read(dir.join("console.txt")).ok()?;
        let console = String::from_utf8_lossy(&console);
        Said::parse(&console.replace(|c: char| c.is_control() && c != '\n', "\n")).seed
    }

    impl TestFxfsGuestFsx {
        /// What the guest kernel's kind slows a run by.
        fn slowness(&self) -> f64 {
            guest_kernel_slowness().unwrap_or(1.0)
        }

        /// What a replay of one script may take.
        fn replay_limit(&self) -> Duration {
            (SETUP + SCRIPT).mul_f64(self.slowness())
        }

        /// The replays of the script of `seed`, one a VM.
        fn replays<'a>(&self, test_setup: &'a TestSetup, seed: u64) -> Replays<'a> {
            Replays {
                runner: std::env::current_exe().unwrap(),
                test_setup,
                seed,
                mix: HOSTILE,
                env: &[(SECS, "0"), (SHRINK_SECS, "0")],
                timeout: self.replay_limit(),
                verdict: &verdict,
            }
        }

        /// What to say of `failure`, of the run in `test_setup` that said `said`: as the guest
        /// shrank it, replayed once from a boot, or shrunk by the host, a VM each try.
        fn report(&self, test_setup: &TestSetup, said: &Said, failure: &Failure) -> String {
            let run = std::fs::read_to_string(test_setup.tmp_dir.join("seed")).unwrap();
            let seed = said.seed.unwrap_or_else(|| run.parse().unwrap());
            let replays = self.replays(test_setup, seed);
            let Some((drops, kept)) = &said.shrunk else {
                return format!("run seed {run}, script seed {seed}:\n{}", replays.report(failure));
            };
            let replayed = match replays.replay(drops) {
                Ok(Some(again)) if again.same_way(failure) => {
                    "which a replay from a boot fails the same way".to_owned()
                }
                Ok(again) => {
                    format!("which a replay from a boot doesn't fail the same way: {again:?}")
                }
                Err(err) => format!("which a replay from a boot couldn't run: {err:#}"),
            };
            let len = script::generate(seed, HOSTILE).len();
            format!(
                "{failure}\nrun seed {run}: the guest shrank script seed {seed} to {} of {len} \
                 operations, {replayed}; FXDAX_SEED={seed} {}={drops} {SECS}=0 replays it:\n  {}",
                kept.len(),
                script::DROP_VAR,
                kept.join("\n  ")
            )
        }

        fn create_images(test_setup: &TestSetup) -> anyhow::Result<()> {
            let image = test_setup.tmp_dir.join("fxdax.img");
            block_on(async {
                let fs = Filesystem::format(Storage::create(&image, IMAGE_SIZE)?).await?;
                fs.close().await?;
                anyhow::Ok(())
            })?;
            let reference = test_setup.tmp_dir.join("ext4.img");
            std::fs::File::create(&reference)?.set_len(REFERENCE_SIZE)?;
            let status = std::process::Command::new("mkfs.ext4")
                .args(["-q", "-F"])
                .arg(&reference)
                .status()
                .context("running mkfs.ext4")?;
            anyhow::ensure!(status.success(), "mkfs.ext4 of {}: {status}", reference.display());
            Ok(())
        }
    }

    impl Test for TestFxfsGuestFsx {
        /// Not -faults-dax but on the debug kernel, whose fault injection it uses.
        fn should_run(&self) -> ShouldRun {
            let build = std::env::var("FXDAX_GUEST_KERNEL_BUILD").unwrap_or_default();
            if self.faults && build != "debug" {
                return ShouldRun::No(
                    "fault injection is the debug kernel's: FXDAX_GUEST_KERNEL_BUILD=debug",
                );
            }
            fxdax_guest_should_run()
        }

        /// Its scripts' time, the shrink's, and a script's and a boot's past them, as the guest
        /// starts a script until its time is up.
        fn timeout_secs(&self) -> u64 {
            let scripts = Duration::from_secs(
                secs(SECS, DEFAULT_SECS) + secs(SHRINK_SECS, DEFAULT_SHRINK_SECS),
            );
            (scripts + (SETUP + SCRIPT).mul_f64(self.slowness())).as_secs()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            Self::create_images(&test_setup)?;
            let mut vm = Vm::new(2, 1024)?;
            vm.guest_kernel()?;
            let window = if self.dax { crate::common::dax_window(WINDOW)? } else { 0 };
            vm.extent_store("fxdax", &test_setup.tmp_dir.join("fxdax.img"), IMAGE_SIZE, window)?;
            let device = vm.disk("reference", &test_setup.tmp_dir.join("ext4.img"))?;
            vm.env(&CString::new(format!("{REFERENCE}={device}"))?);
            vm.env(&CString::new(format!("{SECS}={}", secs(SECS, DEFAULT_SECS)))?);
            vm.env(&CString::new(format!(
                "{SHRINK_SECS}={}",
                secs(SHRINK_SECS, DEFAULT_SHRINK_SECS)
            ))?);
            let seed = seed();
            eprintln!("{}: seed {seed} (FXDAX_SEED={seed} replays it)", test_setup.test_case);
            std::fs::write(test_setup.tmp_dir.join("seed"), seed.to_string())?;
            script_vm::pass_script(&mut vm, seed)?;
            if self.delay {
                vm.module_param(c"fxfs.dax_delay_us=200");
                vm.module_param(&CString::new(format!("fxfs.dax_delay_seed={seed}"))?);
            }
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8_lossy(&stdout);
            let said = Said::parse(&output);
            if let Some(failure) = &said.failure {
                return TestOutcome::Fail(self.report(&test_setup, &said, failure));
            }
            if !said.ok {
                let what = format!("it said {output:?}");
                let died = Failure { op: None, kind: kind::DIED.to_owned(), what };
                return TestOutcome::Fail(self.report(&test_setup, &said, &died));
            }
            if !said.unallowed.is_empty() {
                let run = std::fs::read_to_string(test_setup.tmp_dir.join("seed")).unwrap();
                return TestOutcome::Fail(format!(
                    "run seed {run}: the filesystems' times differ, not as allowed \
                     (differential::ALLOWED):\n{}",
                    said.unallowed.join("\n")
                ));
            }
            let files: Vec<_> = (0..HOSTILE.files).map(|n| (file(n), model(n))).collect();
            match script_vm::stored_as_modelled(&test_setup.tmp_dir.join("fxdax.img"), &files) {
                Some(failure) => TestOutcome::Fail(format!("the last script's files: {failure}")),
                None => TestOutcome::Pass,
            }
        }

        fn timed_out(self: Box<Self>, test_setup: TestSetup) -> Option<String> {
            let said = Said { seed: last_seed(&test_setup.tmp_dir), ..Said::default() };
            let timeout = Duration::from_secs(self.timeout_secs());
            Some(self.report(&test_setup, &said, &script_vm::hang(timeout)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// What a guest that failed and shrank it says, read back.
        #[test]
        fn what_the_guest_said_is_read_back() {
            let failure =
                Failure { op: Some(40), kind: kind::DIFFERS.to_owned(), what: "what".to_owned() };
            let output = format!(
                "{SEED_LINE}7\n{SEED_LINE}8\ntimes: write changes it (allowed: why), first at seed 1\ntimes: copy \
                 leaves it\n{failure}\n{SHRUNK_LINE}0-3,5-39\n  4: Write\n  40: Read\n"
            );
            assert_eq!(
                Said::parse(&output),
                Said {
                    seed: Some(8),
                    failure: Some(failure),
                    shrunk: Some((
                        "0-3,5-39".to_owned(),
                        vec!["4: Write".to_owned(), "40: Read".to_owned()]
                    )),
                    unallowed: vec!["times: copy leaves it".to_owned()],
                    ok: false,
                }
            );
            let passed = Said::parse(&format!("{SEED_LINE}1\nOK\n"));
            assert_eq!(passed, Said { seed: Some(1), ok: true, ..Said::default() });
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use crate::differential::{Differences, Differential};
    use crate::guest_fxfs::evict;
    use crate::script::{self, Failure, HOSTILE, Model, kind};
    use crate::script_run::{self, Faults, Options, Run};
    use crate::test_fxfs_guest_checks::guest::{mount, path, seed};
    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::time::{Duration, Instant};

    /// Where ext4 is mounted.
    const MNT: &str = "/mnt/ext4";
    /// How many runs of a script a shrink in the guest may take: more than the host's, as each is
    /// a run alone, no boot.
    const TRIES: usize = 4 * script::TRIES;
    /// How many of a shrunk script's operations the guest lists.
    const LISTED: usize = 64;

    /// Mounts ext4 from the disk FXDAX_FSX_REFERENCE names, with REFERENCE_OPTIONS.
    fn mount_reference() {
        fs::create_dir_all(MNT).unwrap();
        let device = CString::new(std::env::var(REFERENCE).unwrap()).unwrap();
        let (target, fstype) = (CString::new(MNT).unwrap(), c"ext4");
        let options = CString::new(REFERENCE_OPTIONS).unwrap();
        let ret = unsafe {
            let options = options.as_ptr().cast();
            libc::mount(device.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, options)
        };
        assert_eq!(ret, 0, "mount {MNT}: {}", std::io::Error::last_os_error());
    }

    /// What the scripts of a run came to.
    #[derive(Default)]
    struct Totals {
        scripts: usize,
        refused: usize,
        dax: usize,
        differences: Differences,
    }

    impl TestFxfsGuestFsx {
        /// A script's two runs, ext4's and Fxfs's, on files made anew (`script_run::made_anew`):
        /// a shrink's, each try, as a boot's.
        fn differential(&self, seed: u64, faults: Option<Faults>) -> Differential {
            let names = |dir: &dyn Fn(&str) -> String| -> Vec<String> {
                let names: Vec<String> = (0..HOSTILE.files).map(|n| dir(&file(n))).collect();
                script_run::made_anew(&names);
                names
            };
            let options = Options { seeks: true, ..Options::default() };
            let reference = Run::new(
                &names(&|name| format!("{MNT}/{name}")),
                Options { copies_clones: true, largest: script_run::ext4_largest(MNT), ..options },
            );
            let largest = if self.dax { script::LARGEST_DAX } else { script::LARGEST };
            let mut subject = Run::new(&names(&|name| path(name)), Options { largest, ..options });
            if let Some(faults) = faults {
                subject.inject(faults.of(seed));
            }
            Differential::new(seed, reference, subject)
        }

        /// Says `failure`, of the script of `seed`, `len` operations long, those of `drops` dropped,
        /// and shrinks it, if it was the whole script, for `shrink` at most: each try the script
        /// run again, on files made anew.
        fn failed(
            &self,
            seed: u64,
            drops: &str,
            len: usize,
            failure: &Failure,
            faults: Option<Faults>,
            shrink: Duration,
        ) {
            println!("{failure}");
            crate::kernel_log::print();
            if !drops.is_empty() || shrink.is_zero() || !failure.shrinks() {
                return;
            }
            let deadline = Instant::now() + shrink;
            let shrunk = script::shrink_failure(len, failure, TRIES, |drops| {
                if Instant::now() >= deadline {
                    return None;
                }
                let (script, len) = script_run::numbered(seed, HOSTILE, drops);
                self.differential(seed, faults).script(&script, len).err()
            });
            let Some(kept) = shrunk.kept else {
                eprintln!(
                    "seed {seed}: not shrunk: {} tries of it didn't fail again",
                    shrunk.tries
                );
                return;
            };
            let ops = script::generate(seed, HOSTILE);
            println!("{SHRUNK_LINE}{}", script::drops(&kept, len));
            for &index in kept.iter().take(LISTED) {
                println!("  {index}: {:?}", ops[index]);
            }
            if kept.len() > LISTED {
                println!("  and {} more", kept.len() - LISTED);
            }
            eprintln!("seed {seed}: shrunk to {} operations in {} tries", kept.len(), shrunk.tries);
        }
    }

    /// Whether the files `names` hold, evicted and read in again, what `model` does.
    fn evicted_as_modelled(names: &[String], model: &Model) -> Result<(), Failure> {
        for (name, held) in names.iter().zip(&model.files) {
            evict(name);
            let got = fs::read(name).unwrap();
            if got != *held {
                let first = got.iter().zip(held).position(|(a, b)| a != b);
                let what = format!(
                    "{name} evicted is {} bytes, the model {}, differing at {first:#x?}",
                    got.len(),
                    held.len()
                );
                return Err(Failure { op: None, kind: kind::STORED_DIFFERS.to_owned(), what });
            }
        }
        Ok(())
    }

    impl Test for TestFxfsGuestFsx {
        fn in_guest(self: Box<Self>) {
            mount();
            mount_reference();
            let (first, drops) = (seed(), std::env::var(script::DROP_VAR).unwrap_or_default());
            let faults = self.faults.then(|| Faults::set_up(first));
            let secs = Duration::from_secs(super::secs(SECS, DEFAULT_SECS));
            let shrink = Duration::from_secs(super::secs(SHRINK_SECS, DEFAULT_SHRINK_SECS));
            let deadline = Instant::now() + secs;
            let names: Vec<String> = (0..HOSTILE.files).map(|n| path(&file(n))).collect();
            let mut totals = Totals::default();
            for seed in first.. {
                println!("{SEED_LINE}{seed}");
                let (script, len) = script_run::numbered(seed, HOSTILE, &drops);
                let began = Instant::now();
                let mut differential = self.differential(seed, faults);
                // A replay of part of a script, as a shrink leaves it: few steps, each said.
                differential.says_steps = !drops.is_empty();
                if let Err(failure) = differential.script(&script, len) {
                    eprintln!("seed {seed}: {failure}; {}", differential.subject.history());
                    drop(differential);
                    self.failed(seed, &drops, len, &failure, faults, shrink);
                    return;
                }
                let (_, ran, differences) = differential.finish();
                if let Err(failure) = evicted_as_modelled(&names, &ran.model) {
                    crate::kernel_log::print();
                    println!("{failure}");
                    return;
                }
                eprintln!(
                    "seed {seed}: {} operations, {} refused, {} files read in DAX, in {:.2?}",
                    script.len(),
                    ran.refused,
                    ran.dax,
                    began.elapsed()
                );
                totals.scripts += 1;
                totals.refused += ran.refused;
                totals.dax += ran.dax;
                totals.differences.add(differences);
                if secs.is_zero() || !drops.is_empty() || Instant::now() >= deadline {
                    // The last script's files stay, with their models, for the host to read.
                    for (n, held) in ran.model.files.iter().enumerate() {
                        fs::write(path(&model(n)), held).unwrap();
                        fs::File::open(path(&model(n))).unwrap().sync_all().unwrap();
                    }
                    break;
                }
            }
            eprintln!(
                "{} scripts, {} operations refused, {} files read in DAX, {} times changed alike",
                totals.scripts, totals.refused, totals.dax, totals.differences.1
            );
            for line in totals.differences.lines() {
                println!("{line}");
            }
            // A run that looked at nothing it was to look at fails, but a replay of part of one.
            let whole = drops.is_empty();
            if self.dax && totals.dax == 0 && whole {
                let what = "no file read in again was DAX".to_owned();
                println!("{}", Failure { op: None, kind: kind::NEVER_DAX.to_owned(), what });
                return;
            }
            if totals.differences.1 == 0 && whole {
                let what = "no time compared changed, on either filesystem".to_owned();
                println!("{}", Failure { op: None, kind: kind::NO_TIMES.to_owned(), what });
                return;
            }
            if self.faults && totals.refused == 0 && whole {
                let what = "fault injection failed nothing".to_owned();
                println!("{}", Failure { op: None, kind: kind::NOTHING_REFUSED.to_owned(), what });
                return;
            }
            println!("OK");
        }
    }
}
