// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a script of operations on several files in a guest of its own, shrunk if it fails
//! (TASK-149.2): what fxfs-guest-checks' random_ops does to a file, across three, with clones
//! between them and stores by forked children, each file checked against a model by the guest, and
//! by the host's Fxfs as it reads them from the store. A failure says the seed, and the host boots
//! a VM for each try of a shrink (`script_vm`), of whatever failed: the guest's model, the image's
//! fsck, the host's comparison, a kernel's complaint, its death or its running out of time.
//!
//! fxfs-guest-script-full and -full-dax: on a small volume, filled to `LEFT` before the script
//! starts, where anything may be refused for lack of room (`script_run`), and a run that's never
//! refused anything fails, having looked at nothing.
//!
//! fxfs-guest-script-killed and -killed-dax: the VM killed at an operation the seed picks
//! (`script::kill_index`): the guest does those before it, says [`KILL_ME`] and waits, and the host
//! kills its VM process, writeback, rebinds and the journal still going in the guest's kernel; then
//! the host's Fxfs replays the store, which fsck checks, and each file must hold what it may after a
//! kill (`script::Survivals`): a size it had since its last fsync, and each page as one of its states
//! since then had it. fxfs-guest-script-killed-full and -killed-full-dax: on the small volume, nearly
//! full, where what an operation refused left of a file the guest reads back: it says each such as
//! it goes (`script::Partial`), and the host replays the model by them.

use macros::{guest, host};

pub struct TestFxfsGuestScript {
    /// Whether the device has a DAX window, and the guest maps files into it.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    /// Whether the volume is small, and nearly full.
    pub full: bool,
    /// fxdax: whether the VM is killed part way, and the files checked against what a kill may
    /// leave.
    pub killed: bool,
}

/// fxdax: what a killed run's guest says once it has done the operations before the kill, at which
/// the host kills its VM process.
#[cfg_attr(feature = "host", allow(dead_code))]
const KILL_ME: &str = "kill me";

/// `file`'s name, and its model's.
fn file(n: usize) -> String {
    format!("script-{n}")
}

fn model(n: usize) -> String {
    format!("{}.model", file(n))
}

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::script::{DROP_VAR, Failure, SEVERAL_FILES, kind};
    use crate::script_vm::{self, Replays};
    use crate::test_fxfs_guest_checks::host::seed;
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::{Filesystem, Storage};
    use std::path::Path;
    use std::time::Duration;

    /// Sizes of the volume, whose image is sparse, and the room the guest leaves on the small one.
    const IMAGE_SIZE: u64 = 256 << 20;
    const SMALL_IMAGE_SIZE: u64 = 64 << 20;
    const WINDOW: crate::common::Window = crate::common::Window::Large;

    fn create_image(path: &Path, size: u64) -> anyhow::Result<()> {
        block_on(async {
            let fs = Filesystem::format(Storage::create(path, size)?).await?;
            fs.close().await?;
            Ok(())
        })
    }

    /// fxdax: how the killed run in `dir`, which said `stdout`, failed, if it did: as a run does, if
    /// it said so before the kill; or a file the store holds isn't what it may be after a kill
    /// before the script's operation the seed picks, of what the run's drops kept (TASK-149.2).
    fn killed_verdict(dir: &Path, stdout: &[u8]) -> Option<Failure> {
        let output = String::from_utf8_lossy(stdout);
        if let Some(failure) = Failure::parse(&output) {
            return Some(failure);
        }
        if !output.lines().any(|line| line == KILL_ME) {
            let what = format!("it said {output:?}");
            return Some(Failure { op: None, kind: kind::DIED.to_owned(), what });
        }
        let seed: u64 = std::fs::read_to_string(dir.join("seed")).ok()?.parse().ok()?;
        let drops = std::fs::read_to_string(dir.join("drops")).unwrap_or_default();
        let ops = crate::script::generate(seed, SEVERAL_FILES);
        let kept = crate::script::kept(&drops, ops.len()).expect(DROP_VAR);
        let script: Vec<_> = kept.into_iter().map(|index| (index, ops[index])).collect();
        let kill = crate::script::kill_index(seed, ops.len());
        let partials = crate::script::Partial::said(&output);
        let survivals = crate::script::survivals(&script, kill, SEVERAL_FILES.files, &partials);
        let failed = |kind: &str, what: String| {
            Some(Failure { op: Some(kill), kind: kind.to_owned(), what })
        };
        let fs =
            Storage::open(dir.join("fxdax.img")).map_err(anyhow::Error::from).and_then(|storage| {
                block_on(async { Ok(Filesystem::mount(storage.check().await?).await?) })
            });
        let fs = match fs {
            Ok(fs) => fs,
            Err(err) => return failed(kind::FSCK, format!("{err:#}")),
        };
        for (n, survivals) in survivals.iter().enumerate() {
            let got = block_on(async {
                let Ok((handle, attr)) = fs.root().lookup(file(n).as_bytes()).await else {
                    return Ok(None);
                };
                let mut buf = vec![0; attr.size as usize];
                let count = handle.read(0, &mut buf).await?;
                buf.truncate(count);
                anyhow::Ok(Some(buf))
            });
            let got = match got {
                Ok(got) => got,
                Err(err) => return failed(kind::KILLED_DIFFERS, format!("{}: {err:#}", file(n))),
            };
            if let Err(what) = survivals.hold(got.as_deref()) {
                return failed(kind::KILLED_DIFFERS, format!("{}: {what}", file(n)));
            }
        }
        crate::vm::complaints(&dir.join("console.txt")).err().map(|err| Failure {
            op: None,
            kind: kind::COMPLAINT.to_owned(),
            what: format!("{err:#}"),
        })
    }

    /// How the run in `dir`, which said `stdout`, failed, if it did.
    fn verdict(dir: &Path, stdout: &[u8]) -> Option<Failure> {
        let output = String::from_utf8_lossy(stdout);
        if let Some(failure) = Failure::parse(&output) {
            return Some(failure);
        }
        if output != "OK\n" {
            let what = format!("it said {output:?}");
            return Some(Failure { op: None, kind: kind::DIED.to_owned(), what });
        }
        let files: Vec<_> = (0..SEVERAL_FILES.files).map(|n| (file(n), model(n))).collect();
        script_vm::stored_as_modelled(&dir.join("fxdax.img"), &files).or_else(|| {
            crate::vm::complaints(&dir.join("console.txt")).err().map(|err| Failure {
                op: None,
                kind: kind::COMPLAINT.to_owned(),
                what: format!("{err:#}"),
            })
        })
    }

    /// fxdax: a killed run's files checked as the host's Fxfs reads them from the store (TASK-149.2):
    /// what the operations before the kill left passes; a page no state of the file held fails,
    /// and so does a run that never said it was to be killed.
    #[test]
    fn a_killed_runs_files_are_checked_as_the_store_holds_them() {
        use fxdax::Owner;
        let killed = |files: &[Vec<u8>], seed: u64, output: &[u8]| {
            let dir = tempdir::TempDir::new("killed").unwrap();
            std::fs::write(dir.path().join("seed"), seed.to_string()).unwrap();
            std::fs::write(dir.path().join("console.txt"), b"").unwrap();
            let image = dir.path().join("fxdax.img");
            block_on(async {
                let fs = Filesystem::format(Storage::create(&image, IMAGE_SIZE)?).await?;
                let owner = Owner { uid: 0, gid: 0 };
                for (n, data) in files.iter().enumerate() {
                    let (handle, _) = fs.root().create(file(n).as_bytes(), 0o644, owner).await?;
                    handle.write(0, data).await?;
                }
                fs.close().await?;
                anyhow::Ok(())
            })
            .unwrap();
            killed_verdict(dir.path(), output)
        };
        // A seed whose first file is more than a page long at its kill.
        let (seed, model) = (0..)
            .map(|seed| {
                let ops = crate::script::generate(seed, SEVERAL_FILES);
                let kill = crate::script::kill_index(seed, ops.len());
                let mut model = crate::script::Model::new(SEVERAL_FILES.files);
                for op in &ops[..kill] {
                    model.apply(&model.resolve(op));
                }
                (seed, model)
            })
            .find(|(_, model)| model.files[0].len() > 2 * crate::script::PAGE as usize)
            .unwrap();
        let said = format!("{KILL_ME}\n");
        assert_eq!(killed(&model.files, seed, said.as_bytes()), None);
        let mut garbled = model.files.clone();
        garbled[0][crate::script::PAGE as usize + 3] ^= 0xa5;
        let failure = killed(&garbled, seed, said.as_bytes()).expect("a garbled page passed");
        assert_eq!(failure.kind, kind::KILLED_DIFFERS, "{failure}");
        let failure = killed(&model.files, seed, b"").expect("a run never killed passed");
        assert_eq!(failure.kind, kind::DIED, "{failure}");
    }

    impl TestFxfsGuestScript {
        fn report(&self, test_setup: &TestSetup, failure: &Failure) -> String {
            let seed = std::fs::read_to_string(test_setup.tmp_dir.join("seed")).unwrap();
            let replays = Replays {
                runner: std::env::current_exe().unwrap(),
                test_setup,
                seed: seed.parse().unwrap(),
                mix: SEVERAL_FILES,
                env: &[],
                timeout: Duration::from_secs(self.timeout_secs()),
                verdict: if self.killed { &killed_verdict } else { &verdict },
            };
            replays.report(failure)
        }
    }

    impl Test for TestFxfsGuestScript {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            180
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let image = test_setup.tmp_dir.join("fxdax.img");
            let size = if self.full { SMALL_IMAGE_SIZE } else { IMAGE_SIZE };
            create_image(&image, size)?;
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store(
                "fxdax",
                &image,
                size,
                if self.dax { crate::common::dax_window(WINDOW)? } else { 0 },
            )?;
            let seed = seed();
            eprintln!("{}: seed {seed} (FXDAX_SEED={seed} replays it)", test_setup.test_case);
            std::fs::write(test_setup.tmp_dir.join("seed"), seed.to_string())?;
            // fxdax: what a replay drops, for the verdict of a killed run, which replays the model.
            let drops = std::env::var(DROP_VAR).unwrap_or_default();
            std::fs::write(test_setup.tmp_dir.join("drops"), drops)?;
            if self.killed {
                vm.kill_on(KILL_ME);
            }
            script_vm::pass_script(&mut vm, seed)?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let verdict = if self.killed { killed_verdict } else { verdict };
            match verdict(&test_setup.tmp_dir, &stdout) {
                Some(failure) => TestOutcome::Fail(self.report(&test_setup, &failure)),
                None => TestOutcome::Pass,
            }
        }

        fn timed_out(self: Box<Self>, test_setup: TestSetup) -> Option<String> {
            let timeout = Duration::from_secs(self.timeout_secs());
            Some(self.report(&test_setup, &script_vm::hang(timeout)))
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use crate::script::{Failure, SEVERAL_FILES, kind};
    use crate::script_run::{self, Options, Run};
    use crate::test_fxfs_guest_checks::guest::{evict, mount, path, seed};
    use nix::libc;
    use std::fs;
    use std::os::unix::io::AsRawFd;

    /// How much room the guest leaves on the small volume: less than the files take at their
    /// largest. They may reach 12 MiB, but are full of holes: on XFS and ext4, 2 MiB left had few
    /// operations refused of a script.
    const LEFT: u64 = 1 << 20;
    const BALLAST: &str = "ballast";

    impl Test for TestFxfsGuestScript {
        fn in_guest(self: Box<Self>) {
            mount();
            let seed = seed();
            let (script, len) = script_run::from_env(seed, SEVERAL_FILES);
            let names: Vec<String> = (0..SEVERAL_FILES.files).map(|n| path(&file(n))).collect();
            let options = Options { full: self.full, partials: self.killed, ..Options::default() };
            let mut run = Run::new(&names, options);
            // fxdax: on a DAX mount, the files read in again first, as a file the mount makes is
            // the page cache's until then, copying on write (doc-13): the DAX cases looked at the
            // page cache alone, their files made here and never evicted (TASK-149.2).
            if self.dax {
                for file in 0..names.len() {
                    if let Err(failure) = run.step(0, &crate::script::Op::Reopen { file }) {
                        run.fail(seed, &failure);
                    }
                }
                if run.dax != names.len() {
                    let what =
                        format!("{} of {} files read in again are DAX", run.dax, names.len());
                    let failure = Failure { op: Some(0), kind: kind::NEVER_DAX.to_owned(), what };
                    println!("{failure}");
                    return;
                }
            }
            if self.full {
                script_run::fill(&path(BALLAST), LEFT);
            }
            if self.killed {
                // fxdax: the operations before the kill, then the host's kill, waited for.
                let kill = crate::script::kill_index(seed, len);
                for (index, op) in script.iter().take_while(|(index, _)| *index < kill) {
                    if let Err(failure) = run.step(*index, op) {
                        crate::kernel_log::print();
                        run.fail(seed, &failure);
                    }
                }
                eprintln!("killed before operation {kill} of {len}, {} refused", run.refused);
                println!("{KILL_ME}");
                loop {
                    unsafe { libc::pause() };
                }
            }
            if let Err(failure) = run.script(&script, len) {
                crate::kernel_log::print();
                run.fail(seed, &failure);
            }
            let ran = run.finish();
            if self.full {
                // A shorter script may be one that fits.
                if ran.refused == 0 && script.len() == len {
                    let failure = Failure {
                        op: Some(len),
                        kind: kind::NOTHING_REFUSED.to_owned(),
                        what: format!("{} bytes are free", script_run::free(&path(""))),
                    };
                    println!("{failure}");
                    return;
                }
                // Room for the models.
                fs::remove_file(path(BALLAST)).unwrap();
                let dir = fs::File::open(path("")).unwrap();
                assert_eq!(unsafe { libc::syncfs(dir.as_raw_fd()) }, 0);
            }
            eprintln!("{} operations refused of {}", ran.refused, script.len());
            for (n, name) in names.iter().enumerate() {
                let held = &ran.model.files[n];
                fs::write(path(&model(n)), held).unwrap();
                fs::File::open(path(&model(n))).unwrap().sync_all().unwrap();
                evict(name);
                let got = fs::read(name).unwrap();
                if got != *held {
                    let first = got.iter().zip(held).position(|(a, b)| a != b);
                    let what = format!(
                        "{name} evicted is {} bytes, the model {}, differing at {first:#x?}",
                        got.len(),
                        held.len()
                    );
                    crate::kernel_log::print();
                    // As a failure the host can shrink: a panic reads as a run that died.
                    let failure = Failure { op: None, kind: kind::STORED_DIFFERS.to_owned(), what };
                    println!("{failure}");
                    return;
                }
            }
            println!("OK");
        }
    }
}
