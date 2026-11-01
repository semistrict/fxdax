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

use macros::{guest, host};

pub struct TestFxfsGuestScript {
    /// Whether the device has a DAX window, and the guest maps files into it.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    /// Whether the volume is small, and nearly full.
    pub full: bool,
}

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
    use crate::script::{Failure, SEVERAL_FILES, kind};
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
    const WINDOW: u64 = 1 << 29;

    fn create_image(path: &Path, size: u64) -> anyhow::Result<()> {
        block_on(async {
            let fs = Filesystem::format(Storage::create(path, size)?).await?;
            fs.close().await?;
            Ok(())
        })
    }

    fn read(fs: &Filesystem, name: &str) -> anyhow::Result<Vec<u8>> {
        block_on(async {
            let (file, attr) = fs.root().lookup(name.as_bytes()).await?;
            let mut buf = vec![0; attr.size as usize];
            let count = file.read(0, &mut buf).await?;
            buf.truncate(count);
            Ok(buf)
        })
    }

    /// Whether each file of the image at `path` is as its model, its fsck passing, or else how
    /// not.
    fn stored_as_modelled(path: &Path) -> Option<Failure> {
        let failed =
            |kind: &str, what: String| Some(Failure { op: None, kind: kind.to_owned(), what });
        let fs = Storage::open(path).map_err(anyhow::Error::from).and_then(|storage| {
            block_on(async { Ok(Filesystem::mount(storage.check().await?).await?) })
        });
        let fs = match fs {
            Ok(fs) => fs,
            Err(err) => return failed(kind::FSCK, format!("{err:#}")),
        };
        for n in 0..SEVERAL_FILES.files {
            let same = read(&fs, &file(n)).and_then(|read_| Ok(read_ == read(&fs, &model(n))?));
            match same {
                Ok(true) => {}
                Ok(false) => {
                    return failed(kind::STORED_DIFFERS, format!("{} is not as modelled", file(n)));
                }
                Err(err) => return failed(kind::STORED_DIFFERS, format!("{}: {err:#}", file(n))),
            }
        }
        None
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
        stored_as_modelled(&dir.join("fxdax.img")).or_else(|| {
            crate::vm::complaints(&dir.join("console.txt")).err().map(|err| Failure {
                op: None,
                kind: kind::COMPLAINT.to_owned(),
                what: format!("{err:#}"),
            })
        })
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
                verdict: &verdict,
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
            vm.extent_store("fxdax", &image, size, if self.dax { WINDOW } else { 0 })?;
            let seed = seed();
            eprintln!("{}: seed {seed} (FXDAX_SEED={seed} replays it)", test_setup.test_case);
            std::fs::write(test_setup.tmp_dir.join("seed"), seed.to_string())?;
            script_vm::pass_script(&mut vm, seed)?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
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
    use crate::script_run::{self, Run};
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
            let mut run = Run::new(&names, self.full);
            if self.full {
                script_run::fill(&path(BALLAST), LEFT);
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
                assert!(fs::read(name).unwrap() == *held, "seed {seed}: {name} evicted differs");
            }
            println!("OK");
        }
    }
}
