// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `git fetch` moved onto a newer generation of a shared volume (fxdax's TASK-136.7, backlog
//! doc-10, Moving to a newer generation). A publisher VM keeps a clone of an upstream repo in a
//! volume and publishes it twice, at upstream's first commit (A) and its second (B); upstream then
//! makes a third (C), and its own volume is published too, as the remote. A consumer VM, on a
//! store of its own, runs the automounter on `/repos`, whose asks the host answers as the sandbox
//! manager would: `repo` is the first generation, the consumer's checkout; `repo-g1` the second,
//! the newest generation of `/repos/upstream`, as `git-remote-fxfs` asks; `upstream` the remote.
//! In the consumer, its checkout at A, with a commit of its own, a change staged, one not and a
//! file untracked:
//!
//! - `git fetch`, through `git-remote-fxfs`, points the checkout's alternates at the second
//!   generation's objects, and receives exactly the objects C added, none the generation holds;
//! - `git pull --ff-only` writes the files B and C changed, and no other, and the next `git status`
//!   reads no other file than those and the one changed and unstaged;
//! - its commit, its changes and its untracked file are there still, and the first generation's
//!   objects read;
//! - a fetch the sandbox manager has no generation for (`/mnt/fxfs/elsewhere`, upstream by another
//!   path) is an ordinary one: a clone through it has all of upstream, and no alternates;
//!
//! then the consumer is snapshotted, its volumes frozen, and restored, and the checkout reads
//! through its alternates still; and rebooted, which reopens its store, and the alternates' path
//! automounts again. The host checks what the sandbox manager was asked.

use macros::{guest, host};

pub struct TestFxfsGuestGitFetch;

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run, setup_rootfs};
    use crate::shared_volumes::host::{Manager, Vms, format_with, next_generation};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;

    const CONTAINERFILE: &str = "FROM fedora:43\nRUN dnf install -y git strace && dnf clean all\n";
    const CAPACITY: u64 = 1 << 30;

    impl TestFxfsGuestGitFetch {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let extentd: PathBuf =
                std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into();
            let control = control(&extentd)?;
            let root = setup_rootfs(test_setup)?;
            let vms = Vms::new(test_setup, root, extentd.clone(), CAPACITY, 0);
            let (publisher, consumer) = (dir.join("publisher"), dir.join("consumer"));
            format_with(&publisher, CAPACITY, &["repo", "upstream"])?;

            let (vm, stream) = vms.run("publisher", &publisher, true)?;
            let mut stream = stream.context("the publisher's connection")?;
            let mut generations = Vec::new();
            for name in ["repo", "repo-g1", "upstream"] {
                let path = dir.join(format!("generation-{name}"));
                generations.push((name, next_generation(&mut stream, &control, &publisher, path)?));
            }
            vm.wait()?;

            let names = generations.into_iter().map(|(name, generation)| (name.into(), generation));
            let newest = BTreeMap::from([("/repos/upstream".to_owned(), "repo-g1".to_owned())]);
            let manager = Manager::new(extentd, names.collect(), newest);

            // The consumer, until it is frozen, snapshotted, and restored.
            manager.serve("consumer", &consumer)?;
            let (vm, stream) = vms.run("consumer", &consumer, true)?;
            let mut said = String::new();
            BufReader::new(stream.context("the consumer's connection")?).read_line(&mut said)?;
            anyhow::ensure!(said == "frozen\n", "the consumer said {said:?}");
            let (snapshot, memory) = (dir.join("snapshot"), dir.join("memory"));
            vm.snapshot(&snapshot, &memory)?;
            manager.serve("restored", &consumer)?;
            let (vm, mut stream) = vms.restore("restored", &snapshot, &memory)?;
            stream.write_all(b"restored\n")?;
            vm.wait()?;

            manager.serve("rebooted", &consumer)?;
            vms.run("rebooted", &consumer, false)?.0.wait()?;

            let asked = manager.asked();
            let expected = [
                ("newest /mnt/fxfs/elsewhere", 1),
                ("newest /repos/upstream", 2),
                ("repo", 1),
                ("repo-g1", 1),
                ("upstream", 1),
            ];
            let expected = BTreeMap::from(expected.map(|(what, asks)| (what.to_owned(), asks)));
            anyhow::ensure!(asked == expected, "the sandbox manager was asked {asked:?}");
            Ok(())
        }
    }

    impl Test for TestFxfsGuestGitFetch {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<crate::rootfs::Image> {
            Some(crate::rootfs::Image::new(CONTAINERFILE))
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
        FIFREEZE, FITHAW, HOME, MNT, REPO, USER, command_with, connect, give, ioctl, mount,
        mount_repo, publish_volume, settle_index, try_connect, unmount,
    };
    use std::collections::HashSet;
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::chown;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// Where the publisher mounts upstream's volume.
    const UPSTREAM: &str = "/mnt/fxfs/upstream";
    /// Where the automounter mounts, in the consumer.
    const ROOT: &str = "/repos";
    /// The consumer's checkout, its remote, and that remote by a path no generation is of.
    const CHECKOUT: &str = "/repos/repo";
    const REMOTE: &str = "/repos/upstream";
    const ELSEWHERE: &str = "/mnt/fxfs/elsewhere";
    /// Where `git-remote-fxfs` is, in the default volume: the guest agent, by that name.
    const BIN: &str = "/mnt/fxfs/bin";
    const PATH: &str = "/mnt/fxfs/bin:/usr/bin:/bin";
    /// The tracked files: `FILES` in each of `DIRECTORIES` directories.
    const FILES: usize = 20;
    const DIRECTORIES: usize = 4;

    fn tracked(directory: usize, number: usize) -> String {
        format!("d{directory}/f{number:03}")
    }

    /// File `number` of each directory as upstream's commit `commit` (0 for A) leaves it.
    fn contents(directory: usize, number: usize, commit: usize) -> String {
        format!("file {directory}/{number} as of commit {commit}\n").repeat(100)
    }

    /// The files upstream's second and third commits change: the first and second of each
    /// directory.
    fn changed() -> HashSet<String> {
        (0..DIRECTORIES)
            .flat_map(|directory| [tracked(directory, 0), tracked(directory, 1)])
            .collect()
    }

    fn git(dir: &str, args: &[&str]) -> String {
        command_with("git", args, dir, PATH)
    }

    /// Upstream's commit `commit`: 0 writes every file, 1 the first of each directory, 2 the
    /// second.
    fn commit(commit: usize) {
        for directory in 0..DIRECTORIES {
            for number in 0..FILES {
                if commit == 0 || number == commit - 1 {
                    let path = format!("{UPSTREAM}/{}", tracked(directory, number));
                    fs::write(&path, contents(directory, number, commit)).unwrap();
                    chown(&path, Some(USER), Some(USER)).unwrap();
                }
            }
        }
        git(UPSTREAM, &["add", "."]);
        git(UPSTREAM, &["commit", "-m", &["first", "second", "third"][commit]]);
    }

    fn publisher() {
        mount_repo();
        give(UPSTREAM);
        mount(Some("upstream"), UPSTREAM);
        give(UPSTREAM);
        git(UPSTREAM, &["init", "--initial-branch=main"]);
        for directory in 0..DIRECTORIES {
            give(&format!("{UPSTREAM}/d{directory}"));
        }
        commit(0);
        git(MNT, &["clone", "--no-hardlinks", UPSTREAM, REPO]);
        git(REPO, &["config", "gc.auto", "0"]);
        settle_index();
        let mut host = connect();
        publish_volume(&mut host, REPO, &[UPSTREAM, MNT]);

        commit(1);
        git(REPO, &["pull", "--ff-only"]);
        settle_index();
        publish_volume(&mut host, REPO, &[UPSTREAM, MNT]);
        unmount(REPO);

        // Upstream moves on past the second generation, and is published as the remote.
        commit(2);
        publish_volume(&mut host, UPSTREAM, &[MNT]);
        unmount(UPSTREAM);
    }

    /// Waits up to a minute for `done`.
    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < deadline, "waited a minute for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn mounted(path: &str) -> Option<String> {
        let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap();
        mountinfo.lines().find_map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let separator = fields.iter().position(|field| *field == "-")?;
            (fields[4] == path).then(|| fields[separator + 1].to_owned())
        })
    }

    /// The automounter on `ROOT`, its mounts kept, as a process of its own, once it serves.
    fn automounter() -> Child {
        fs::create_dir_all(ROOT).unwrap();
        let child = Command::new("/proc/self/exe")
            .args(["automount", ROOT, MNT])
            .spawn()
            .expect("starting the automounter");
        wait_for("the automounter to serve", || mounted(ROOT).as_deref() == Some("autofs"));
        child
    }

    /// The objects in the checkout's own directory of them, loose: what a fetch received, as
    /// the checkout keeps all it receives loose (`fetch.unpackLimit`).
    fn loose() -> HashSet<String> {
        let objects = format!("{CHECKOUT}/.git/objects");
        let mut loose = HashSet::new();
        for entry in fs::read_dir(&objects).unwrap() {
            let entry = entry.unwrap();
            let prefix = entry.file_name().into_string().unwrap();
            if prefix.len() != 2 {
                continue;
            }
            for object in fs::read_dir(entry.path()).unwrap() {
                loose.insert(format!("{prefix}{}", object.unwrap().file_name().to_str().unwrap()));
            }
        }
        loose
    }

    /// The objects `range` reaches in the repo at `dir`.
    fn objects(dir: &str, range: &str) -> HashSet<String> {
        let listed = git(dir, &["rev-list", "--objects", range]);
        listed.lines().map(|line| line.split(' ').next().unwrap().to_owned()).collect()
    }

    /// Runs `git args` in the checkout under strace, and returns the files under it it opened,
    /// those it opened to write, each relative to it, and what it printed. Not directories, which
    /// a status opens to list what is untracked.
    fn traced(args: &[&str]) -> (HashSet<String>, HashSet<String>, String) {
        let trace = format!("{HOME}/trace-{}", args[0]);
        let mut strace =
            vec!["-f", "-qq", "-s", "4096", "-e", "trace=open,openat", "-o", &trace, "git"];
        strace.extend(args);
        let printed = command_with("strace", &strace, CHECKOUT, PATH);
        let (mut opened, mut written) = (HashSet::new(), HashSet::new());
        for line in BufReader::new(File::open(&trace).unwrap()).lines() {
            let line = line.unwrap();
            let Some(path) = line.split('"').nth(1) else { continue };
            let directory = line.contains("O_DIRECTORY");
            if line.contains("= -1")
                || directory
                || path.starts_with(".git")
                || path.starts_with('/')
            {
                continue;
            }
            opened.insert(path.to_owned());
            if line.contains("O_WRONLY") || line.contains("O_RDWR") {
                written.insert(path.to_owned());
            }
        }
        (opened, written, printed)
    }

    /// The checkout's changes of its own, before the fetch: a commit on a branch, a change
    /// staged, one not, and a file untracked.
    fn change_the_checkout() {
        git(CHECKOUT, &["checkout", "-b", "local"]);
        fs::write(format!("{CHECKOUT}/local-only"), b"the consumer's commit\n").unwrap();
        git(CHECKOUT, &["add", "local-only"]);
        git(CHECKOUT, &["commit", "-m", "the consumer's"]);
        git(CHECKOUT, &["checkout", "main"]);
        fs::write(format!("{CHECKOUT}/d2/f010"), b"staged\n").unwrap();
        git(CHECKOUT, &["add", "d2/f010"]);
        fs::write(format!("{CHECKOUT}/d3/f011"), b"not staged\n").unwrap();
        fs::write(format!("{CHECKOUT}/untracked"), b"untracked\n").unwrap();
        // Git trusts no file changed in the second its index was written or after: a status
        // would read the staged file again for that alone, as the publisher's settle_index says.
        std::thread::sleep(Duration::from_millis(1100));
    }

    /// That the checkout's changes of its own are there, and the first generation's objects:
    /// the first commit's files as they were.
    fn check_the_checkout() {
        assert_eq!(git(CHECKOUT, &["log", "-1", "--format=%s", "local"]), "the consumer's\n");
        assert_eq!(git(CHECKOUT, &["diff", "--cached", "--name-only"]), "d2/f010\n");
        assert_eq!(git(CHECKOUT, &["diff", "--name-only"]), "d3/f011\n");
        assert_eq!(fs::read(format!("{CHECKOUT}/untracked")).unwrap(), b"untracked\n");
        let first = git(CHECKOUT, &["show", "main~2:d0/f000"]);
        assert_eq!(first, contents(0, 0, 0));
    }

    /// That the checkout is at upstream's third commit, every object of it read through its
    /// alternates or its own.
    fn check_upstream_reached() {
        assert_eq!(git(CHECKOUT, &["log", "-1", "--format=%s", "main"]), "third\n");
        git(CHECKOUT, &["fsck", "--connectivity-only", "--no-dangling"]);
        assert_eq!(fs::read_to_string(format!("{CHECKOUT}/d1/f001")).unwrap(), contents(1, 1, 2));
    }

    fn fetch_and_pull() {
        let config = [
            ["url.fxfs::/repos/upstream.insteadOf", REMOTE],
            ["url.fxfs::/mnt/fxfs/elsewhere.insteadOf", ELSEWHERE],
            ["fetch.unpackLimit", "1000000"],
        ];
        for [name, value] in config {
            git(HOME, &["config", "--global", name, value]);
        }
        assert_eq!(git(CHECKOUT, &["log", "-1", "--format=%s"]), "first\n");
        git(CHECKOUT, &["remote", "set-url", "origin", REMOTE]);
        change_the_checkout();

        // The fetch: onto the second generation, receiving what upstream made since.
        let before = loose();
        git(CHECKOUT, &["fetch", "origin"]);
        let alternates = fs::read_to_string(format!("{CHECKOUT}/.git/objects/info/alternates"));
        assert_eq!(alternates.unwrap(), "/repos/repo-g1/.git/objects\n");
        let second = git("/repos/repo-g1", &["rev-parse", "HEAD"]);
        let received: HashSet<_> = loose().difference(&before).cloned().collect();
        let since = objects(REMOTE, &format!("{}..main", second.trim()));
        assert_eq!(received, since, "received other than what upstream made since generation 1");
        let generation: HashSet<_> = objects("/repos/repo-g1", "--all");
        assert!(received.is_disjoint(&generation), "received what generation 1 holds");

        // The pull: the files it changed, and no other.
        let (_, written, _) = traced(&["pull", "--ff-only", "origin", "main"]);
        assert_eq!(written, changed(), "the pull wrote other files");
        let (opened, _, status) = traced(&["status", "--porcelain"]);
        let mut expected = changed();
        expected.insert("d3/f011".into());
        let read: Vec<_> = opened.iter().filter(|path| path.starts_with('d')).collect();
        let others: Vec<_> = read.iter().filter(|path| !expected.contains(**path)).collect();
        assert!(others.is_empty(), "status read {others:?}");
        assert_eq!(status, "M  d2/f010\n M d3/f011\n?? untracked\n");
        check_the_checkout();
        check_upstream_reached();

        // No generation for this path: an ordinary clone, of everything, with no alternates.
        std::os::unix::fs::symlink(REMOTE, ELSEWHERE).unwrap();
        git(HOME, &["clone", ELSEWHERE, "plain"]);
        let plain = format!("{HOME}/plain");
        assert_eq!(git(&plain, &["log", "-1", "--format=%s"]), "third\n");
        assert!(fs::metadata(format!("{plain}/.git/objects/info/alternates")).is_err());
    }

    /// The fxfs volumes mounted, the default one last: what freezes the filesystem.
    const FROZEN: [&str; 4] = [CHECKOUT, "/repos/repo-g1", REMOTE, MNT];

    /// Freezes the filesystem, tells the host, which snapshots the VM, and once restored, as the
    /// host says on a connection made anew, thaws it. The host closes its end as soon as it has
    /// read that the guest is frozen, so that the guest may connect again as the snapshot is
    /// taken: restored, that connection is reset, and made again, as any a restore resets
    /// ([`fxfs_automount::reconnect`], TASK-277).
    fn snapshotted() {
        FROZEN.iter().for_each(|volume| ioctl(volume, FIFREEZE));
        let mut host = connect();
        host.write_all(b"frozen\n").unwrap();
        // The snapshot is taken as this waits; restored, the connection is gone.
        let mut nothing = Vec::new();
        let _ = host.read_to_end(&mut nothing);
        // The host listens before it restores the VM (`Vms::restore`): a connection refused is
        // one a reset ended.
        let restored = fxfs_automount::reconnect::exchange(
            || try_connect().map_err(std::io::Error::from),
            |host| {
                let mut said = String::new();
                match BufReader::new(host).read_line(&mut said)? {
                    0 => Err(std::io::ErrorKind::UnexpectedEof.into()),
                    _ => Ok(said),
                }
            },
        );
        assert_eq!(restored.expect("the host's word that the VM is restored"), "restored\n");
        FROZEN.iter().rev().for_each(|volume| ioctl(volume, FITHAW));
    }

    fn consumer() {
        fs::create_dir_all(BIN).unwrap();
        std::os::unix::fs::symlink("/guest-agent", format!("{BIN}/git-remote-fxfs")).unwrap();
        let _automounter = automounter();
        fetch_and_pull();
        snapshotted();
        check_upstream_reached();
        check_the_checkout();
    }

    fn rebooted() {
        let _automounter = automounter();
        check_upstream_reached();
        check_the_checkout();
        assert_eq!(mounted("/repos/repo-g1").as_deref(), Some("fxfs"));
    }

    impl Test for TestFxfsGuestGitFetch {
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
            println!("OK {role}");
        }
    }
}
