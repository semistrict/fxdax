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

/// The vsock port the guests talk to the host on.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5053;
/// The generations the publisher publishes.
#[cfg_attr(feature = "guest", allow(dead_code))]
const GENERATIONS: usize = 2;

#[host]
mod host {
    use super::*;
    use crate::common::{control, fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use futures::executor::block_on;
    use fxdax::{Image, Storage};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    const CONTAINERFILE: &str = "FROM fedora:43\nRUN dnf install -y git strace && dnf clean all\n";
    const CAPACITY: u64 = 1 << 30;
    const WINDOW: u64 = 1 << 28;

    /// Makes the publisher's store at `path`: its default volume, and the volume it publishes.
    fn format(path: &Path) -> anyhow::Result<()> {
        block_on(async {
            let image = Image::format(Storage::create(path, CAPACITY)?).await?;
            for volume in ["default", "repo"] {
                image.create_volume(volume).await?.unmount().await?;
            }
            image.close().await?;
            Ok(())
        })
    }

    /// A manifest from `stream`, its size first.
    fn receive(stream: &mut impl Read) -> anyhow::Result<Vec<u8>> {
        let mut size = [0; 8];
        stream.read_exact(&mut size)?;
        let mut manifest = vec![0; u64::from_le_bytes(size) as usize];
        stream.read_exact(&mut manifest)?;
        Ok(manifest)
    }

    fn send(stream: &mut impl Write, manifest: &[u8]) -> anyhow::Result<()> {
        stream.write_all(&(manifest.len() as u64).to_le_bytes())?;
        stream.write_all(manifest)?;
        Ok(())
    }

    /// The test's VMs, which run one after the other.
    struct Vms<'a> {
        test_setup: &'a TestSetup,
        root: PathBuf,
        window: u64,
    }

    impl Vms<'_> {
        /// A VM of the guest's `role`, on the store at `store`, and the host's end of the
        /// connection it opens, if `talks`.
        fn run(
            &self,
            role: &str,
            store: &Path,
            talks: bool,
        ) -> anyhow::Result<(Firecracker, Option<UnixStream>)> {
            let mut vm = Vm::new(2, 1024)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", store, CAPACITY, self.window)?;
            vm.env(&CString::new(format!("FXDAX_SHARED_VOLUMES={role}"))?);
            // Short, as Unix sockets' paths must be, and one for each VM's Firecracker.
            let uds = std::env::temp_dir()
                .join(format!("fxdax-shared-{}-{role}.vsock", std::process::id()));
            let listener = talks
                .then(|| {
                    vm.vsock(&uds);
                    UnixListener::bind(format!("{}_{PORT}", uds.display()))
                })
                .transpose()?;
            let root = &self.root;
            let api = Vm::api_socket(role);
            let mut vm = vm.spawn_api(root, &self.test_setup.test_case, role, &api)?;
            let stream = listener.map(|listener| vm.accept(&listener)).transpose()?;
            Ok((vm, stream))
        }
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}");
        Ok(())
    }

    impl TestFxfsGuestSharedVolumes {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let extentd: PathBuf =
                std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into();
            let control = control(&extentd)?;
            let window = if self.dax { WINDOW } else { 0 };
            let vms = Vms { test_setup, root: setup_rootfs(test_setup)?, window };
            let publisher = dir.join("publisher");
            let consumer = dir.join("consumer");
            let generation = |number: usize| dir.join(format!("generation-{number}"));
            format(&publisher)?;

            let (vm, stream) = vms.run("publisher", &publisher, true)?;
            let mut stream = stream.context("the publisher's connection")?;
            let mut manifests = Vec::new();
            for number in 0..GENERATIONS {
                manifests.push(receive(&mut stream)?);
                control.fork_store(&publisher, &generation(number))?;
                control.make_generation_public(&generation(number))?;
                stream.write_all(b"forked\n")?;
            }
            wait(vm)?;

            let (vm, stream) = vms.run("consumer", &consumer, true)?;
            let stream = stream.context("the consumer's connection")?;
            let mut answers = stream.try_clone()?;
            for line in BufReader::new(stream).lines() {
                let number = line?.parse::<usize>()?;
                control.attach_base(&consumer, &generation(number))?;
                send(&mut answers, &manifests[number])?;
            }
            wait(vm)?;

            wait(vms.run("rebooted", &consumer, false)?.0)
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

    use nix::libc;
    use std::collections::HashSet;
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Error, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd as _, IntoRawFd as _};
    use std::os::unix::fs::{MetadataExt, chown};
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::Duration;

    /// The default volume, on whose directories the others mount.
    const MNT: &str = "/mnt/fxfs";
    /// The repo the publisher fetches from, in the default volume, so outside what it publishes.
    const UPSTREAM: &str = "/mnt/fxfs/upstream";
    /// Where the publisher mounts the volume.
    const REPO: &str = "/mnt/fxfs/repo";
    /// Who runs Git: the publisher's user is the consumers'.
    const USER: u32 = 1000;
    /// A user who owns nothing the publisher wrote.
    const OTHER_USER: u32 = 1001;
    /// How many files the repo tracks, in each of `DIRECTORIES` directories.
    const FILES: usize = 50;
    const DIRECTORIES: usize = 4;
    /// `FIFREEZE` and `FITHAW`: `_IOWR('X', 119, int)` and `_IOWR('X', 120, int)`.
    const FIFREEZE: libc::c_ulong = 0xc004_5877;
    const FITHAW: libc::c_ulong = 0xc004_5878;
    /// fs/fxfs's FXFS_MANIFEST_MAX.
    const MANIFEST_MAX: usize = 65536;
    /// `USER`'s home, in the default volume, where traces go too.
    const HOME: &str = "/mnt/fxfs/home";
    /// Holds the commit the publisher's second fetch leaves unreachable.
    const UNREACHABLE: &str = ".git/published-unreachable";

    /// fs/fxfs's `struct fxfs_manifest`.
    #[repr(C)]
    struct Manifest {
        data: u64,
        size: u64,
    }

    /// fs/fxfs's `struct fxfs_import`.
    #[repr(C)]
    struct Import {
        name: u64,
        name_size: u64,
        manifest: u64,
        manifest_size: u64,
    }

    nix::ioctl_none!(flush_ioctl, b'x', 0x45);
    nix::ioctl_readwrite!(manifest_ioctl, b'x', 0x46, Manifest);
    nix::ioctl_write_ptr!(import_ioctl, b'x', 0x47, Import);

    fn try_mount(volume: Option<&str>, target: &str) -> std::io::Result<()> {
        let source = CString::new("extents").unwrap();
        let (target, fstype) = (CString::new(target).unwrap(), CString::new("fxfs").unwrap());
        let options = volume.map(|volume| CString::new(format!("volume={volume}")).unwrap());
        let data = options.as_ref().map_or(std::ptr::null(), |options| options.as_ptr());
        // SAFETY: The strings outlive the call.
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, data.cast())
        };
        if ret == 0 { Ok(()) } else { Err(Error::last_os_error()) }
    }

    fn mount(volume: Option<&str>, target: &str) {
        try_mount(volume, target).unwrap_or_else(|error| panic!("mount {target}: {error}"));
    }

    fn unmount(target: &str) {
        let path = CString::new(target).unwrap();
        // SAFETY: The string outlives the call.
        let ret = unsafe { libc::umount(path.as_ptr()) };
        assert_eq!(ret, 0, "umount {target}: {}", Error::last_os_error());
    }

    fn ioctl(target: &str, request: libc::c_ulong) {
        let dir = File::open(target).unwrap();
        // SAFETY: Neither takes an argument it reads.
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
        assert_eq!(ret, 0, "{request:#x} on {target}: {}", Error::last_os_error());
    }

    fn try_flush(volume: &str) -> nix::Result<()> {
        let dir = File::open(volume).unwrap();
        // SAFETY: It takes nothing.
        unsafe { flush_ioctl(dir.as_raw_fd()) }.map(drop)
    }

    fn try_manifest(volume: &str) -> nix::Result<Vec<u8>> {
        let dir = File::open(volume).unwrap();
        let mut manifest = vec![0; MANIFEST_MAX];
        let mut request =
            Manifest { data: manifest.as_mut_ptr() as u64, size: MANIFEST_MAX as u64 };
        // SAFETY: `request` says what `manifest` has room for, and both outlive the call.
        unsafe { manifest_ioctl(dir.as_raw_fd(), &mut request) }?;
        manifest.truncate(request.size as usize);
        Ok(manifest)
    }

    fn try_import(name: &str, manifest: &[u8]) -> nix::Result<()> {
        let dir = File::open(MNT).unwrap();
        let request = Import {
            name: name.as_ptr() as u64,
            name_size: name.len() as u64,
            manifest: manifest.as_ptr() as u64,
            manifest_size: manifest.len() as u64,
        };
        // SAFETY: `request` says what `name` and `manifest` hold, and they outlive the call.
        unsafe { import_ioctl(dir.as_raw_fd(), &request) }.map(drop)
    }

    /// Runs `git args` as `USER` in `dir`, and returns what it printed, which it must succeed at.
    fn git(dir: &str, args: &[&str]) -> String {
        command("git", args, dir)
    }

    fn command(program: &str, args: &[&str], dir: &str) -> String {
        let output = Command::new(program)
            .args(args)
            .current_dir(dir)
            .uid(USER)
            .gid(USER)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", HOME)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // A status leaves the index as it is: what one finds, the next finds too.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "publisher")
            .env("GIT_AUTHOR_EMAIL", "publisher@fxdax")
            .env("GIT_COMMITTER_NAME", "publisher")
            .env("GIT_COMMITTER_EMAIL", "publisher@fxdax")
            .output()
            .unwrap_or_else(|error| panic!("{program}: {error}"));
        assert!(
            output.status.success(),
            "{program} {args:?} in {dir}: {:?}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// A connection to the host on `PORT`.
    fn connect() -> File {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        // SAFETY: The socket's descriptor, which the file owns from here on.
        unsafe { File::from_raw_fd(fd.into_raw_fd()) }
    }

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

    /// Has `dir`, and what is made in it, belong to `USER`.
    fn give(dir: &str) {
        fs::create_dir_all(dir).unwrap();
        chown(dir, Some(USER), Some(USER)).unwrap();
    }

    fn make_upstream() {
        give(UPSTREAM);
        git(UPSTREAM, &["init", "--initial-branch=main"]);
        for directory in 0..DIRECTORIES {
            give(&format!("{UPSTREAM}/d{directory}"));
        }
        for path in tracked_files() {
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

    /// What the publisher does for a first `git status` elsewhere to trust the index (doc-10): Git
    /// doesn't, for a file changed in the second the index was written or after, so the index is
    /// written again once that second is over.
    fn settle_index() {
        std::thread::sleep(Duration::from_millis(1100));
        git(REPO, &["update-index", "--refresh"]);
        git(REPO, &["update-index", "--force-write-index"]);
    }

    /// Publishes the volume at `REPO` to the host, which forks the store, and tells this it did.
    fn publish(host: &mut File) {
        // SAFETY: It takes nothing.
        unsafe { libc::sync() };
        ioctl(REPO, FIFREEZE);
        try_flush(REPO).expect("flushing");
        ioctl(MNT, FIFREEZE);
        let manifest = try_manifest(REPO).expect("the manifest");
        host.write_all(&(manifest.len() as u64).to_le_bytes()).unwrap();
        host.write_all(&manifest).unwrap();
        let mut said = [0; 7];
        host.read_exact(&mut said).unwrap();
        assert_eq!(&said, b"forked\n");
        ioctl(MNT, FITHAW);
        ioctl(REPO, FITHAW);
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
        // SAFETY: It takes nothing.
        unsafe { libc::sync() };
        ioctl(REPO, FIFREEZE);
        ioctl(MNT, FIFREEZE);
        assert_eq!(try_manifest(REPO).unwrap_err(), nix::Error::EBUSY);
        ioctl(MNT, FITHAW);
        ioctl(REPO, FITHAW);
        fs::remove_file(format!("{REPO}/.git/in-the-journal")).unwrap();
    }

    fn publisher() {
        give(REPO);
        mount(Some("repo"), REPO);
        give(REPO);
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
        writeln!(host, "{number}").unwrap();
        let mut size = [0; 8];
        host.read_exact(&mut size).unwrap();
        let mut manifest = vec![0; u64::from_le_bytes(size) as usize];
        host.read_exact(&mut manifest).unwrap();

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

    fn tracked_files() -> HashSet<String> {
        (0..DIRECTORIES)
            .flat_map(|directory| (0..FILES).map(move |number| tracked(directory, number)))
            .collect()
    }

    /// Runs `git status` on the repo at `path`, which must find it clean, and returns the paths it
    /// statted and those it opened.
    fn traced_status(path: &str) -> (HashSet<String>, HashSet<String>) {
        let trace = format!("{HOME}/status-{}", path.rsplit('-').next().unwrap());
        let status = command(
            "strace",
            &["-f", "-qq", "-e", "trace=file", "-o", &trace, "git", "status", "--porcelain"],
            path,
        );
        assert_eq!(status, "", "the status of {path}");

        let (mut statted, mut opened) = (HashSet::new(), HashSet::new());
        for line in BufReader::new(File::open(&trace).unwrap()).lines() {
            let line = line.unwrap();
            let Some((call, rest)) = line.split_once('(') else { continue };
            let call = call.rsplit(' ').next().unwrap();
            let Some(path) = rest.split('"').nth(1) else { continue };
            match call {
                "lstat" | "stat" | "newfstatat" | "statx" => statted.insert(path.to_owned()),
                "open" | "openat" | "openat2" => opened.insert(path.to_owned()),
                _ => false,
            };
        }
        (statted, opened)
    }

    /// That the first `git status` of the repo at `path` finds it clean, lstatting every tracked
    /// file and opening none, so that it hashes none, as `USER`.
    fn check_first_status(path: &str) {
        let tracked = tracked_files();
        assert_eq!(git(path, &["ls-files"]).lines().collect::<HashSet<_>>().len(), tracked.len());
        let (statted, opened) = traced_status(path);
        let missed: Vec<_> = tracked.difference(&statted).collect();
        assert!(missed.is_empty(), "not lstatted: {missed:?}");
        let hashed: Vec<_> = tracked.intersection(&opened).collect();
        assert!(hashed.is_empty(), "opened: {hashed:?}");
    }

    /// That `git status` hashes every tracked file of the repo at `path` once they change owners,
    /// which the index names: what makes `check_first_status` mean something, too.
    fn check_new_owner_hashes(path: &str) {
        let tracked = tracked_files();
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

    fn drop_caches() {
        fs::write("/proc/sys/vm/drop_caches", b"3").unwrap();
    }

    fn consumer() {
        let mut host = connect();

        let newest = attach(&mut host, 1);
        check_first_status(&newest);
        check_generation(&newest, 1);
        check_unreachable(&newest);

        // The older, after the publisher moved on, with its own files and history.
        let older = attach(&mut host, 0);
        check_first_status(&older);
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

        assert_eq!(try_import("garbage", &[0xff; 64]).unwrap_err(), nix::Error::EIO);
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
        check_first_status(&older);
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
