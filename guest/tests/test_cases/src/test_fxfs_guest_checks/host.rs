// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The host's half of fxfs-guest-checks: the image, the VM, and what the host checks of what the
//! guest left.

use super::*;

use crate::common::{fxdax_guest_should_run, guest_kernel_slowness, setup_rootfs};
use crate::script::{self, Failure};
use crate::script_vm::{self, Replays};
use crate::vm::{Vm, without_kernel_lines};
use crate::{ShouldRun, Test, TestOutcome, TestSetup};
use anyhow::Context;
use futures::executor::block_on;
use fxdax::{Attr, Filesystem, Node, Owner, Storage};
use std::path::Path;
use std::time::Duration;

fn create_image(path: &Path) -> anyhow::Result<()> {
    block_on(async {
        let fs = Filesystem::format(Storage::create(path, IMAGE_SIZE)?).await?;
        let owner = Owner { uid: 0, gid: 0 };
        let (file, _) = fs.root().create(b"from-host", 0o644, owner).await?;
        file.write(0, HOST_CONTENT).await?;
        fs.close().await?;
        Ok(())
    })
}

/// Checks the image at `path`, and serves it.
fn open_image(path: &Path) -> Filesystem {
    let storage = Storage::open(path).expect("open image");
    block_on(async { Filesystem::mount(storage.check().await?).await }).expect("mount image")
}

fn lookup_path(fs: &Filesystem, path: &str) -> (Node, Attr) {
    let mut node = fs.root().clone();
    let mut attr = block_on(node.getattr()).unwrap();
    for name in path.split('/') {
        (node, attr) = block_on(node.lookup(name.as_bytes()))
            .unwrap_or_else(|err| panic!("host: lookup {path}: {err}"));
    }
    (node, attr)
}

fn read_path(fs: &Filesystem, path: &str) -> Vec<u8> {
    let (file, attr) = lookup_path(fs, path);
    let mut buf = vec![0; attr.size as usize];
    let count = block_on(file.read(0, &mut buf)).unwrap();
    buf.truncate(count);
    buf
}

/// fxdax: the seed of a run's random choices (random_ops's operations, fxfs.dax_delay_us's
/// delays): FXDAX_SEED's, to replay a run, or else a new one each run, which the run says
/// (TASK-119.2).
pub(crate) fn seed() -> u64 {
    std::env::var("FXDAX_SEED").ok().and_then(|seed| seed.parse().ok()).unwrap_or_else(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        now.as_nanos() as u64 ^ u64::from(std::process::id()) << 32
    })
}

/// fxdax: what a run of the checks takes besides its checks: creating the image, booting, and
/// syncing as the guest ends.
const SETUP: Duration = Duration::from_secs(30);

/// fxdax: what the checks' budgets are scaled by where the daemon keeps their stores in a real
/// bucket (`FXDAX_EXTENTD_REMOTE`): the most a check took of its budget on GCS, 3.3 times (GCE
/// r980), and half that again (TASK-165.11).
const REMOTE_SCALE: f64 = 5.0;

impl TestFxfsGuestChecks {
    /// fxdax: the checks a run is of, FXDAX_CHECKS's, as [`TestFxfsGuestChecks::checks`] has
    /// them.
    fn only(&self) -> String {
        std::env::var("FXDAX_CHECKS").unwrap_or_default()
    }

    /// fxdax: the run's checks, and what their budgets are scaled by: the guest kernel's
    /// slowness, its storage's, and FXDAX_CHECKS_SCALE's, for a slower host (TASK-168). A real
    /// bucket's, [`REMOTE_SCALE`], is measured: on GCS a check took at most 3.3 times its budget
    /// on a file bucket (GCE r980, TASK-165.11: append_writeback's 64 fdatasyncs, each a durable
    /// commit's create and lease read, 10.0 s of 3.0 s; torture 79 s of 38 s), against r353's
    /// twice, which four of them overran on GCS (r740).
    fn run(&self) -> anyhow::Result<(Vec<(usize, &'static Check)>, f64)> {
        let checks = self.checks(&self.only()).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(!checks.is_empty(), "FXDAX_CHECKS names none of this suite's checks");
        let remote = std::env::var_os("FXDAX_EXTENTD_REMOTE").is_some_and(|r| !r.is_empty());
        let storage = if remote { REMOTE_SCALE } else { 1.0 };
        let given = match std::env::var(SCALE) {
            Ok(given) => given.parse().with_context(|| format!("{SCALE} is {given:?}"))?,
            Err(_) => 1.0,
        };
        Ok((checks, guest_kernel_slowness()? * storage * given))
    }

    /// fxdax: what the guest says as the run ends: OK, or the checks that failed, those made to
    /// fail alone ([`Suites::Failing`]).
    fn expected(&self, checks: &[(usize, &'static Check)]) -> String {
        let failing: Vec<&str> = (checks.iter())
            .filter(|(_, check)| check.suites == Suites::Failing)
            .map(|(_, check)| check.name)
            .collect();
        if failing.is_empty() {
            "OK\n".to_owned()
        } else {
            format!("FAILED: {}\n", failing.join(", "))
        }
    }

    /// fxdax: whether the guest's log, `log`, says that those of `checks` made to fail failed as
    /// they were made to, by name, and that it put back what they left: the fault injection
    /// `crashes` set, the filesystem `hangs` froze (TASK-160.30). Each check's own line, after its
    /// time.
    fn failed_as_made_to(checks: &[(usize, &Check)], log: &str, scale: f64) -> Result<(), String> {
        let ended = |name: &str, ended: Ended| {
            let line = log.lines().find(|line| line.starts_with(&format!("FAIL: {name} (")));
            let said = format!("): {ended}");
            match line {
                Some(line) if line.ends_with(&said) => Ok(()),
                line => Err(format!("{name}: expected a line ending {said:?}, got {line:?}")),
            }
        };
        let said = |line: String| {
            let found = log.lines().any(|said| said == line);
            if found { Ok(()) } else { Err(format!("expected the line {line:?} in the log")) }
        };
        for (_, check) in checks.iter().filter(|(_, check)| check.suites == Suites::Failing) {
            let budget = check.budget.mul_f64(scale);
            match check.name {
                "crashes" => {
                    ended("crashes", Ended::Signalled(nix::libc::SIGBUS))?;
                    said("crashes: left fxfs.dax_fail_maps at 1000, set back to 0".to_owned())?;
                }
                "hangs" => {
                    ended("hangs", Ended::Overran)?;
                    said(format!(
                        "hangs: still running after its budget of {budget:.2?}; the guest's stacks:"
                    ))?;
                    said("hangs: left the filesystem frozen, thawed".to_owned())?;
                }
                name => panic!("{name}, made to fail, says nothing of how"),
            }
        }
        Ok(())
    }

    /// fxdax: what a replay of random_ops alone may take (`script_vm`).
    fn random_ops_limit(&self) -> Duration {
        let scale = self.run().map_or(1.0, |(_, scale)| scale);
        let random_ops = CHECKS.iter().find(|check| check.name == "random_ops").unwrap();
        SETUP + random_ops.budget.mul_f64(scale * (2.0 + HOST_SCALE))
    }
}

impl Test for TestFxfsGuestChecks {
    fn should_run(&self) -> ShouldRun {
        fxdax_guest_should_run()
    }

    /// fxdax: a run's limit: [`SETUP`]; its checks' budgets, scaled, twice over, the second for
    /// what they wait for the host, which their budgets leave out but for a [`HOST_SCALE`]th of it
    /// (`guest::run`'s `Clock`, TASK-263); and the most one check may wait for the host,
    /// [`HOST_SCALE`] times the largest budget, so that the guest ends a check whose device never
    /// answers, by name, before this does. A run of xattrs_bounded and append_writeback waited 4.5
    /// times their budgets for the host, on ZFS; most of a whole suite's checks wait little. The
    /// guest ends a run sooner, as a check outruns its budget, unless too stuck to (TASK-168).
    /// [`SETUP`] alone for a run of no checks it can run, which start_vm fails.
    fn timeout_secs(&self) -> u64 {
        let Ok((checks, scale)) = self.run() else { return SETUP.as_secs() };
        let budgets = checks.iter().map(|(_, check)| check.budget.mul_f64(scale));
        let (sum, most) = budgets.fold((Duration::ZERO, Duration::ZERO), |(sum, most), budget| {
            (sum + budget, most.max(budget))
        });
        (SETUP + sum * 2 + most.mul_f64(HOST_SCALE)).as_secs()
    }

    /// fxdax: names the check the guest was in, after the last its log has the time of.
    fn timed_out(self: Box<Self>, test_setup: TestSetup) -> Option<String> {
        let (checks, scale) = self.run().ok()?;
        let log = std::fs::read_to_string(test_setup.tmp_dir.join("log.txt")).ok()?;
        let last = log.lines().rev().find_map(|line| {
            let (_, rest) = line.split_once("PASS: ").or_else(|| line.split_once("FAIL: "))?;
            rest.split_whitespace().next()
        });
        let at = match last {
            Some(last) => checks.iter().position(|(_, check)| check.name == last)? + 1,
            None => 0,
        };
        let (_, running) = checks.get(at)?;
        let budget = running.budget.mul_f64(scale);
        Some(format!("out of time in {}, of a budget of {budget:.2?}", running.name))
    }

    fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
        let image = test_setup.tmp_dir.join("fxdax.img");
        create_image(&image)?;
        // fxdax: a window as large as the data, or a small one's, that fills (-small); either
        // FXDAX_DAX_WINDOW_MIB's if set (TASK-227).
        let window = match (self.dax, self.small_window) {
            (false, _) => 0,
            (true, false) => crate::common::dax_window(crate::common::Window::Large)?,
            (true, true) => crate::common::dax_window(crate::common::Window::Small(32 << 20))?,
        };
        let mut vm = Vm::new(2, 512)?;
        vm.guest_kernel()?;
        vm.extent_store("fxdax", &image, IMAGE_SIZE, window)?;
        // FXDAX_CHECKS names the checks to run, separated by commas (such as
        // `seal_race,sparse`), for a quick run: all of them otherwise. fxdax: and the guest
        // their budgets' scale (TASK-168).
        let (_, scale) = self.run()?;
        vm.env(&std::ffi::CString::new(format!("FXDAX_CHECKS={}", self.only()))?);
        vm.env(&std::ffi::CString::new(format!("{SCALE}={scale}"))?);
        let seed = seed();
        eprintln!("{}: seed {seed} (FXDAX_SEED={seed} replays it)", test_setup.test_case);
        std::fs::write(test_setup.tmp_dir.join("seed"), seed.to_string())?;
        script_vm::pass_script(&mut vm, seed)?;
        if self.delay {
            vm.module_param(c"fxfs.dax_delay_us=200");
            vm.module_param(&std::ffi::CString::new(format!("fxfs.dax_delay_seed={seed}"))?);
        }
        vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
    }

    fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
        let output = String::from_utf8(stdout).unwrap();
        let seed = std::fs::read_to_string(test_setup.tmp_dir.join("seed")).unwrap();
        let (checks, scale) = self.run().unwrap();
        let expected = self.expected(&checks);
        if output != expected {
            // fxdax: random_ops, run alone, as a run of its script told what to drop can be
            // (TASK-149.2).
            let shrunk = Failure::parse(&output).map(|failure| {
                let replays = Replays {
                    runner: std::env::current_exe().unwrap(),
                    test_setup: &test_setup,
                    seed: seed.parse().unwrap(),
                    mix: script::ONE_FILE,
                    env: &[("FXDAX_CHECKS", "random_ops")],
                    timeout: self.random_ops_limit(),
                    verdict: &|_, stdout| Failure::parse(&String::from_utf8_lossy(stdout)),
                };
                replays.report(&failure)
            });
            let shrunk = shrunk.unwrap_or_default();
            return TestOutcome::Fail(format!(
                "seed {seed}: expected exactly {expected:?}, got {output:?}\n{shrunk}"
            ));
        }
        // fxdax: the checks made to fail failed alone, as made to, and the rest passed after
        // them (TASK-160.30).
        let log = std::fs::read_to_string(test_setup.tmp_dir.join("log.txt")).unwrap();
        if let Err(failed) = Self::failed_as_made_to(&checks, &without_kernel_lines(&log), scale) {
            return TestOutcome::Fail(format!("seed {seed}: {failed}"));
        }
        // fxdax: fsck, whichever checks ran.
        let fs = open_image(&test_setup.tmp_dir.join("fxdax.img"));
        // fxdax: what a coverage kernel counted, under the directory the run says (TASK-119.14).
        if let Some(coverage) = std::env::var_os("FXDAX_COVERAGE") {
            let vm = format!("{}-{}", test_setup.test_case, std::process::id());
            let counts = Path::new(&coverage).join(vm);
            std::fs::create_dir(&counts).unwrap();
            for source in COVERED {
                let name = format!("{source}.gcda");
                let count = read_path(&fs, &format!("{COVERAGE}/{name}"));
                std::fs::write(counts.join(name), count).unwrap();
            }
        }
        // The host's verifies of what each check left (TASK-160.30), of those that ran.
        let image = Image { fs: &fs, seed: &seed, suite: &self };
        for (at, _) in &checks {
            if let Some(verify) = VERIFIES[*at] {
                verify(&image);
            }
        }
        TestOutcome::Pass
    }
}

/// fxdax: the image as the host's Fxfs reads it once the VM is gone, which a check's verify
/// looks at what the check left in: of the run of seed `seed`, of suite `suite`.
pub(crate) struct Image<'a> {
    fs: &'a Filesystem,
    seed: &'a str,
    suite: &'a TestFxfsGuestChecks,
}

/// fxdax: [`checks`]'s verifies, in [`CHECKS`]'s order: each check's, if it has one.
macro_rules! check_verifies {
    ($($name:ident: $suites:ident, $budget:literal => $body:ident $(($field:ident))?
        $(, $verify:ident)?;)*) => {
        const VERIFIES: &[Option<fn(&Image)>] = &[$(check_verifies!(@verify $($verify)?),)*];
    };
    (@verify) => { None };
    (@verify $verify:ident) => { Some($verify) };
}
checks!(check_verifies);

fn verify_mmap_write(image: &Image) {
    let mapped = read_path(image.fs, "dax-mapped");
    assert_eq!(mapped.len(), MAPPED_SIZE, "host: dax-mapped size");
    // The guest's Fxfs, with a window, wrote more once the file was read in as DAX.
    let dax_writes = image.suite.dax;
    let after = CHUNK + 4096;
    for (offset, &byte) in mapped.iter().enumerate() {
        let expected = if dax_writes && offset < EVICTED.len() {
            EVICTED[offset]
        } else if dax_writes && (after..after + WRITTEN.len()).contains(&offset) {
            WRITTEN[offset - after]
        } else if (CHUNK..CHUNK + WRITTEN.len()).contains(&offset) {
            WRITTEN[offset - CHUNK]
        } else {
            pattern(offset)
        };
        assert_eq!(byte, expected, "host: dax-mapped byte {offset}");
    }
}

fn verify_regular_write(image: &Image) {
    assert_eq!(read_path(image.fs, "written"), WRITTEN);
}

fn verify_namespace(image: &Image) {
    let fs = image.fs;
    assert_eq!(read_path(fs, "dir/renamed"), b"renamed");
    assert_eq!(lookup_path(fs, "dir/renamed").1.nlink, 2);
    assert_eq!(read_path(fs, "dir/link"), b"renamed");
    assert_eq!(block_on(lookup_path(fs, "sym").0.readlink()).unwrap(), b"written");
    assert!(block_on(fs.root().lookup(b"unlinked")).is_err());
}

fn verify_truncate_mapped(image: &Image) {
    assert_eq!(read_path(image.fs, "truncated"), vec![b't'; 100]);
}

/// Never fsynced: only removing the mapping wrote it back
fn verify_eviction_writes_back(image: &Image) {
    assert_eq!(read_path(image.fs, "evicted"), EVICTED);
}

/// Made with O_TMPFILE, then named.
fn verify_tmpfile(image: &Image) {
    assert_eq!(read_path(image.fs, "tmpfile-linked"), b"unnamed, then linked");
    assert_eq!(lookup_path(image.fs, "tmpfile-linked").1.nlink, 1);
}

fn verify_fallocate(image: &Image) {
    assert_eq!(read_path(image.fs, "fallocated"), vec![0; 8192]);
    assert_eq!(read_path(image.fs, "punched"), punched_contents(16000, 12000));
}

/// Unlinked, later, in batches, before the fsync of their directory.
fn verify_delayed_unlinks(image: &Image) {
    let fs = image.fs;
    assert!(block_on(fs.root().lookup(b"delayed")).is_err(), "delayed/ remains");
    assert_eq!(read_path(fs, "delayed-kept/again"), b"recreated");
    assert_eq!(lookup_path(fs, "delayed-kept/linked").1.nlink, 1);
    assert!(block_on(lookup_path(fs, "delayed-kept").0.lookup(b"gone")).is_err());
}

/// fxdax: random_ops's file, as the store holds it, is as its model (TASK-119.2), and its
/// operations, which replay it, are there (TASK-119.15).
fn verify_random_ops(image: &Image) {
    let (fs, seed) = (image.fs, image.seed);
    assert!(
        read_path(fs, RANDOM_OPS) == read_path(fs, RANDOM_OPS_MODEL),
        "host: seed {seed}: random-ops differs from its model"
    );
    let history = read_path(fs, RANDOM_OPS_HISTORY);
    assert!(
        history.starts_with(format!("seed {seed}\n").as_bytes()),
        "host: random_ops's history is of another seed"
    );
}

/// fxdax: the blocks appends reserved past a file's end left it as written (TASK-133).
fn verify_append_reserved_tail(image: &Image) {
    assert_eq!(read_path(image.fs, APPEND_RESERVED), append_reserved());
}

/// fxdax: torture's files are as their models (TASK-119.5).
fn verify_torture(image: &Image) {
    for name in (0..TORTURE_FILES).map(torture) {
        assert!(
            read_path(image.fs, &name) == read_path(image.fs, &format!("{name}.model")),
            "host: seed {}: {name} differs from its model",
            image.seed
        );
    }
}
