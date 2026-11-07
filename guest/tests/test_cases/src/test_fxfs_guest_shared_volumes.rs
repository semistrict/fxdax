// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shared volumes (fxdax's TASK-136.4, backlog doc-10), with Git, in the guests that use them. A
//! publisher VM clones a repo into a volume and publishes it twice, updating it in between: for
//! each generation it freezes, flushes and asks for the volume's manifest, and the host forks its
//! store, with no `snapshot_volume`, and goes on keeping the manifest, which it doesn't read. A
//! consumer VM, on a store of its own, asks the host for the generations as it runs, newest first,
//! so that the older is attached after the publisher moved on. It imports each as a volume and
//! mounts it, and as the publisher's user:
//!
//! - its first `git status` lstats every tracked file and opens none, so hashes none;
//! - what the publisher's last fetch left unreachable is there;
//! - what it writes over the base shows, stays private to it, and survives a remount;
//!
//! and a reboot of the consumer finds what it wrote, and reads through to both generations still.
//! Files given to another user, as a consumer would to make a checkout its own, do get hashed: the
//! index names their owner, which is why they must be the consumers' as the publisher writes it.
//!
//! fxfs-guest-shared-volumes-dax: the same with a DAX window, on both VMs.

use macros::{guest, host};

pub struct TestFxfsGuestSharedVolumes {
    /// Whether the devices have a DAX window.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

/// The generations the publisher publishes.
#[cfg_attr(feature = "guest", allow(dead_code))]
const GENERATIONS: usize = 2;

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run, setup_rootfs};
    use crate::shared_volumes::host::{Vms, format, next_generation};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::path::PathBuf;

    const CONTAINERFILE: &str = "FROM fedora:43\nRUN dnf install -y git strace && dnf clean all\n";
    const CAPACITY: u64 = 1 << 30;
    const WINDOW: crate::common::Window = crate::common::Window::Large;

    impl TestFxfsGuestSharedVolumes {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let extentd: PathBuf =
                std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into();
            let control = control(&extentd)?;
            let window = if self.dax { crate::common::dax_window(WINDOW)? } else { 0 };
            let root = setup_rootfs(test_setup)?;
            let vms = Vms::new(test_setup, root, extentd, CAPACITY, window);
            let publisher = dir.join("publisher");
            let consumer = dir.join("consumer");
            format(&publisher, CAPACITY)?;

            let (vm, stream) = vms.run("publisher", &publisher, true)?;
            let mut stream = stream.context("the publisher's connection")?;
            let mut generations = Vec::new();
            for number in 0..GENERATIONS {
                let path = dir.join(format!("generation-{number}"));
                generations.push(next_generation(&mut stream, &control, &publisher, path)?);
            }
            vm.wait()?;

            vms.run_consumer("consumer", &consumer, &control, &generations)?;
            vms.run("rebooted", &consumer, false)?.0.wait()
        }
    }

    impl Test for TestFxfsGuestSharedVolumes {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        fn timeout_secs(&self) -> u64 {
            300
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
        FIFREEZE, FITHAW, HOME, MNT, REPO, USER, attach_generation, check_first_status, connect,
        drop_caches, git, give, ioctl, mount, mount_repo, publish, settle_index, sync,
        traced_status, tracked_files, try_flush, try_import, try_manifest, unmount,
    };
    use std::collections::HashSet;
    use std::fs::{self, File};
    use std::os::unix::fs::{MetadataExt, chown};

    /// The repo the publisher fetches from, in the default volume, so outside what it publishes.
    const UPSTREAM: &str = "/mnt/fxfs/upstream";
    /// A user who owns nothing the publisher wrote.
    const OTHER_USER: u32 = 1001;
    /// How many files the repo tracks, in each of `DIRECTORIES` directories.
    const FILES: usize = 50;
    const DIRECTORIES: usize = 4;
    /// Holds the commit the publisher's second fetch leaves unreachable.
    const UNREACHABLE: &str = ".git/published-unreachable";

    /// The tracked file `number` of directory `directory`.
    fn tracked(directory: usize, number: usize) -> String {
        format!("d{directory}/f{number:03}")
    }

    fn file_contents(directory: usize, number: usize, generation: usize) -> String {
        format!("file {directory}/{number} of generation {generation}\n").repeat(100)
    }

    fn write_tracked(generation: usize) {
        for directory in 0..DIRECTORIES {
            for number in 0..FILES {
                // Generation 1 changes one file in each directory, as a pull does.
                if generation == 0 || number == 0 {
                    let path = format!("{UPSTREAM}/{}", tracked(directory, number));
                    fs::write(path, file_contents(directory, number, generation)).unwrap();
                }
            }
        }
    }

    fn make_upstream() {
        give(UPSTREAM);
        git(UPSTREAM, &["init", "--initial-branch=main"]);
        for directory in 0..DIRECTORIES {
            give(&format!("{UPSTREAM}/d{directory}"));
        }
        for path in written_files() {
            let path = format!("{UPSTREAM}/{path}");
            File::create(&path).unwrap();
            chown(&path, Some(USER), Some(USER)).unwrap();
        }
        write_tracked(0);
        git(UPSTREAM, &["add", "."]);
        git(UPSTREAM, &["commit", "-m", "first"]);
        git(UPSTREAM, &["checkout", "-b", "side"]);
        fs::write(format!("{UPSTREAM}/d0/f000"), "only on the side branch\n").unwrap();
        git(UPSTREAM, &["commit", "-am", "side"]);
        git(UPSTREAM, &["checkout", "main"]);
    }

    /// What refuses to publish: a filesystem not frozen, or frozen too soon, and a volume whose
    /// records are still in the journal.
    fn check_refusals() {
        assert_eq!(try_manifest(REPO).unwrap_err(), nix::Error::EINVAL);

        ioctl(REPO, FIFREEZE);
        ioctl(MNT, FIFREEZE);
        assert_eq!(try_flush(REPO).unwrap_err(), nix::Error::EBUSY);
        assert_eq!(try_import("frozen", b"").unwrap_err(), nix::Error::EBUSY);
        ioctl(MNT, FITHAW);
        ioctl(REPO, FITHAW);

        fs::write(format!("{REPO}/.git/in-the-journal"), b"not flushed").unwrap();
        sync();
        ioctl(REPO, FIFREEZE);
        ioctl(MNT, FIFREEZE);
        assert_eq!(try_manifest(REPO).unwrap_err(), nix::Error::EBUSY);
        ioctl(MNT, FITHAW);
        ioctl(REPO, FITHAW);
        fs::remove_file(format!("{REPO}/.git/in-the-journal")).unwrap();
    }

    fn publisher() {
        mount_repo();
        make_upstream();
        git(MNT, &["clone", "--no-hardlinks", UPSTREAM, REPO]);
        git(REPO, &["config", "gc.auto", "0"]);
        git(REPO, &["fetch", "origin", "side:side"]);
        check_refusals();

        settle_index();
        let mut host = connect();
        publish(&mut host);

        // The second generation: upstream moves on, and its side branch goes, which leaves that
        // branch's commit unreachable here. The publisher keeps it, for alternates pointing here.
        let side = git(REPO, &["rev-parse", "side"]);
        write_tracked(1);
        git(UPSTREAM, &["commit", "-am", "second"]);
        git(UPSTREAM, &["branch", "-D", "side"]);
        git(REPO, &["fetch", "--prune", "origin"]);
        git(REPO, &["merge", "--ff-only", "origin/main"]);
        git(REPO, &["branch", "-D", "side"]);
        fs::write(format!("{REPO}/{UNREACHABLE}"), side).unwrap();
        settle_index();
        publish(&mut host);

        // Nothing it does from here on reaches a generation.
        fs::write(format!("{REPO}/after-the-last-generation"), b"not published").unwrap();
        fs::write(format!("{REPO}/d1/f001"), b"not published").unwrap();
        unmount(REPO);
    }

    /// Attaches generation `number` and makes it the volume mounted at the path this returns.
    fn attach(host: &mut File, number: usize) -> String {
        let manifest = attach_generation(host, number);
        let name = format!("generation-{number}");
        try_import(&name, &manifest).expect("importing");
        assert_eq!(try_import(&name, &manifest).unwrap_err(), nix::Error::EEXIST);
        let path = mounted(number);
        fs::create_dir(&path).unwrap();
        mount(Some(&name), &path);
        path
    }

    fn mounted(number: usize) -> String {
        format!("{MNT}/generation-{number}")
    }

    /// What the repo tracks, as `make_upstream` writes it.
    fn written_files() -> HashSet<String> {
        (0..DIRECTORIES)
            .flat_map(|directory| (0..FILES).map(move |number| tracked(directory, number)))
            .collect()
    }

    /// That the repo at `path` tracks what was written, and its first `git status` trusts it.
    fn check_status(path: &str) {
        assert_eq!(tracked_files(path), written_files());
        check_first_status(path);
    }

    /// That `git status` hashes every tracked file of the repo at `path` once they change owners,
    /// which the index names: what makes `check_first_status` mean something, too.
    fn check_new_owner_hashes(path: &str) {
        let tracked = written_files();
        for file in &tracked {
            chown(format!("{path}/{file}"), Some(OTHER_USER), Some(OTHER_USER)).unwrap();
        }
        let (_, opened) = traced_status(path);
        let trusted: Vec<_> = tracked.difference(&opened).collect();
        assert!(trusted.is_empty(), "not opened: {trusted:?}");
    }

    /// What the two generations hold.
    fn check_generation(path: &str, number: usize) {
        let last = ["first", "second"][number];
        assert_eq!(git(path, &["log", "-1", "--format=%s"]), format!("{last}\n"));
        assert_eq!(
            fs::read_to_string(format!("{path}/d2/f000")).unwrap(),
            file_contents(2, 0, number)
        );
        assert_eq!(fs::read_to_string(format!("{path}/d3/f042")).unwrap(), file_contents(3, 42, 0));
        assert!(fs::metadata(format!("{path}/after-the-last-generation")).is_err());
    }

    fn check_unreachable(path: &str) {
        let commit = fs::read_to_string(format!("{path}/{UNREACHABLE}")).unwrap();
        let commit = commit.trim();
        assert_eq!(git(path, &["cat-file", "-t", commit]), "commit\n");
        assert!(!git(path, &["rev-list", "--all"]).contains(commit), "{commit} is reachable");
    }

    /// Changes the repo at `path`, privately, and that it reads so.
    fn write_over(path: &str) {
        fs::write(format!("{path}/d0/f001"), b"the consumer's\n").unwrap();
        fs::remove_file(format!("{path}/d1/f002")).unwrap();
        fs::rename(format!("{path}/d2/f003"), format!("{path}/d2/moved")).unwrap();
        fs::write(format!("{path}/mine"), b"the consumer's\n").unwrap();
        check_written_over(path);
        git(path, &["add", "-A"]);
        git(path, &["commit", "-m", "the consumer's"]);
    }

    fn check_written_over(path: &str) {
        assert_eq!(fs::read(format!("{path}/d0/f001")).unwrap(), b"the consumer's\n");
        assert!(fs::metadata(format!("{path}/d1/f002")).is_err());
        assert!(fs::metadata(format!("{path}/d2/f003")).is_err());
        assert_eq!(fs::read(format!("{path}/mine")).unwrap(), b"the consumer's\n");
    }

    fn consumer() {
        let mut host = connect();

        let newest = attach(&mut host, 1);
        check_status(&newest);
        check_generation(&newest, 1);
        check_unreachable(&newest);

        // The older, after the publisher moved on, with its own files and history.
        let older = attach(&mut host, 0);
        check_status(&older);
        check_generation(&older, 0);
        assert_ne!(fs::metadata(&newest).unwrap().dev(), fs::metadata(&older).unwrap().dev());

        write_over(&newest);
        check_generation(&older, 0);
        assert_eq!(git(&older, &["status", "--porcelain"]), "");
        drop_caches();
        check_written_over(&newest);

        unmount(&older);
        unmount(&newest);
        unmount(MNT);
        mount(None, MNT);
        mount(Some("generation-1"), &newest);
        mount(Some("generation-0"), &older);
        check_written_over(&newest);
        assert_eq!(git(&newest, &["status", "--porcelain"]), "");
        assert_eq!(git(&newest, &["log", "-1", "--format=%s"]), "the consumer's\n");
        check_generation(&older, 0);

        // A manifest that doesn't parse is an argument wrong: EINVAL, not an I/O error, nor
        // corruption (TASK-160.21).
        assert_eq!(try_import("garbage", &[0xff; 64]).unwrap_err(), nix::Error::EINVAL);
        assert_eq!(try_import("garbage", &[0xff; 2]).unwrap_err(), nix::Error::EINVAL);
        unmount(&older);
        unmount(&newest);
    }

    fn rebooted() {
        let (older, newest) = (mounted(0), mounted(1));
        mount(Some("generation-1"), &newest);
        mount(Some("generation-0"), &older);
        check_written_over(&newest);
        assert_eq!(git(&newest, &["log", "-1", "--format=%s"]), "the consumer's\n");
        check_unreachable(&newest);
        check_status(&older);
        check_generation(&older, 0);
        check_new_owner_hashes(&older);
        unmount(&older);
        unmount(&newest);
    }

    impl Test for TestFxfsGuestSharedVolumes {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount(None, MNT);
            give(HOME);
            let role = std::env::var("FXDAX_SHARED_VOLUMES").unwrap();
            match role.as_str() {
                "publisher" => publisher(),
                "consumer" => consumer(),
                "rebooted" => rebooted(),
                role => panic!("no role {role}"),
            }
            unmount(MNT);
            println!("OK {role}");
        }
    }
}
