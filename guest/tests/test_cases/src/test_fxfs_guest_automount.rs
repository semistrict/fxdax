// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shared volumes attached on a path's lookup (fxdax's TASK-136.6, backlog doc-10, Automount). A
//! publisher VM publishes a volume, as fxfs-guest-shared-volumes's does. A consumer VM, on a
//! store of its own, runs the automounter (`fxfs-automount`) on `/repos`, whose asks the host
//! answers as the sandbox manager would: `repo` and `other` are the generation, imported as
//! volumes of those names; any other name is refused. In the consumer:
//!
//! - a lookup of `/repos/repo` returns once the volume is mounted there, holding the generation;
//! - lookups of `/repos/other`, side by side, have it attached once (the host counts);
//! - a lookup of a name refused fails `ENOENT`, leaving nothing mounted and no volume made;
//! - idle for the timeout, the mounts expire: unmounted, their volumes closed (`volumes_open`),
//!   and the next lookup mounts `repo` again, holding what was written to it, with no ask;
//!
//! and a reboot of the consumer, running the automounter again, mounts `repo` with no ask either.
//!
//! fxfs-guest-automount-dax: the same with a DAX window, on both VMs.

use macros::{guest, host};

pub struct TestFxfsGuestAutomount {
    /// Whether the devices have a DAX window.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run, setup_rootfs};
    use crate::shared_volumes::host::{Manager, Vms, format, next_generation};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    const CAPACITY: u64 = 1 << 30;
    const WINDOW: crate::common::Window = crate::common::Window::Large;

    impl TestFxfsGuestAutomount {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let extentd: PathBuf =
                std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into();
            let control = control(&extentd)?;
            let window = if self.dax { crate::common::dax_window(WINDOW)? } else { 0 };
            let root = setup_rootfs(test_setup)?;
            let vms = Vms::new(test_setup, root, extentd.clone(), CAPACITY, window);
            let (publisher, consumer) = (dir.join("publisher"), dir.join("consumer"));
            format(&publisher, CAPACITY)?;

            let (vm, stream) = vms.run("publisher", &publisher, true)?;
            let mut stream = stream.context("the publisher's connection")?;
            let path = dir.join("generation");
            let generation = next_generation(&mut stream, &control, &publisher, path)?;
            vm.wait()?;

            let names = ["repo", "other"].map(|name| (name.to_owned(), generation.clone()));
            let manager = Manager::new(extentd, BTreeMap::from(names), BTreeMap::new());
            for role in ["consumer", "rebooted"] {
                manager.serve(role, &consumer)?;
                vms.run(role, &consumer, false)?.0.wait()?;
            }
            let asked = manager.asked();
            let expected = [("other", 1), ("refused", 1), ("repo", 1)];
            let expected = BTreeMap::from(expected.map(|(name, asks)| (name.to_owned(), asks)));
            anyhow::ensure!(asked == expected, "the sandbox manager was asked {asked:?}");
            Ok(())
        }
    }

    impl Test for TestFxfsGuestAutomount {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            240
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
            if output == "OK publisher\nOK consumer\nOK rebooted\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected the three roles' OKs, got {output:?}"))
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use crate::shared_volumes::guest::{
        MNT, REPO, connect, give, mount, mount_repo, publish, sync, try_mount, unmount,
    };
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// Where the automounter mounts.
    const ROOT: &str = "/repos";
    /// How long its mounts stay idle before they expire, in seconds.
    const TIMEOUT: &str = "2";

    fn contents(name: &str) -> String {
        format!("{name} as published\n").repeat(100)
    }

    fn publisher() {
        mount_repo();
        give(&format!("{REPO}/dir"));
        for name in ["file", "dir/nested"] {
            fs::write(format!("{REPO}/{name}"), contents(name)).unwrap();
        }
        publish(&mut connect());
        unmount(REPO);
    }

    /// Waits up to a minute for `done`.
    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < deadline, "waited a minute for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The type of the filesystem mounted at `path`, if one is.
    fn mounted(path: &str) -> Option<String> {
        let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap();
        mountinfo.lines().find_map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let separator = fields.iter().position(|field| *field == "-")?;
            (fields[4] == path).then(|| fields[separator + 1].to_owned())
        })
    }

    /// The volumes the filesystem has open, as it says (TASK-136.6).
    fn volumes_open() -> usize {
        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
        let line = stats.lines().find_map(|line| line.strip_prefix("volumes_open "));
        line.expect("no volumes_open").parse().unwrap()
    }

    /// The automounter, on `ROOT`, as a process of its own, once it serves.
    fn automounter() -> Child {
        fs::create_dir_all(ROOT).unwrap();
        let child = Command::new("/proc/self/exe")
            .args(["automount", ROOT, MNT, "--timeout", TIMEOUT])
            .spawn()
            .expect("starting the automounter");
        wait_for("the automounter to serve", || mounted(ROOT).as_deref() == Some("autofs"));
        child
    }

    fn check_published(path: &str) {
        for name in ["file", "dir/nested"] {
            assert_eq!(fs::read_to_string(format!("{path}/{name}")).unwrap(), contents(name));
        }
    }

    fn consumer() {
        let _automounter = automounter();
        let before = volumes_open();

        // A lookup returns once mounted.
        let repo = format!("{ROOT}/repo");
        check_published(&repo);
        assert_eq!(mounted(&repo).as_deref(), Some("fxfs"));
        let dev = |path: &str| fs::metadata(path).unwrap().dev();
        assert_ne!(dev(&repo), dev(MNT));
        fs::write(format!("{repo}/written"), b"the consumer's\n").unwrap();
        sync();

        // Lookups side by side, attached once (the host counts).
        let other = format!("{ROOT}/other");
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    check_published(&other);
                });
            }
        });
        assert_eq!(mounted(&other).as_deref(), Some("fxfs"));
        assert_eq!(volumes_open(), before + 2);

        // A name refused: no mount, no volume.
        let refused = format!("{ROOT}/refused");
        let error = fs::metadata(&refused).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
        assert_eq!(mounted(&refused), None);
        let probe = format!("{MNT}/probe");
        fs::create_dir_all(&probe).unwrap();
        let error = try_mount(Some("refused"), &probe).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(nix::libc::ENOENT), "{error}");

        // Idle, they expire, their volumes closed; repo mounts again, unasked.
        wait_for("the mounts to expire", || mounted(&repo).is_none() && mounted(&other).is_none());
        assert_eq!(volumes_open(), before);
        assert_eq!(fs::read(format!("{repo}/written")).unwrap(), b"the consumer's\n");
        check_published(&repo);
        assert_eq!(mounted(&repo).as_deref(), Some("fxfs"));
    }

    fn rebooted() {
        let _automounter = automounter();
        let repo = format!("{ROOT}/repo");
        assert_eq!(fs::read(format!("{repo}/written")).unwrap(), b"the consumer's\n");
        check_published(&repo);
    }

    impl Test for TestFxfsGuestAutomount {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount(None, MNT);
            let role = std::env::var("FXDAX_SHARED_VOLUMES").unwrap();
            match role.as_str() {
                "publisher" => publisher(),
                "consumer" => consumer(),
                "rebooted" => rebooted(),
                role => panic!("no role {role}"),
            }
            // What the automounter mounted goes with the VM, which resets.
            println!("OK {role}");
        }
    }
}
