// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the tests of shared volumes (backlog doc-10) have in common. A publisher VM keeps a repo
//! current with Git in a volume and publishes it: it freezes, flushes and asks for the volume's
//! manifest, and the host forks its store, with no `snapshot_volume`, and keeps the manifest, which
//! it doesn't read. Consumer VMs, on stores of their own, ask the host for a generation as they
//! run, and import it as a volume.

use macros::{guest, host};

/// The vsock port the guests talk to the host on.
const PORT: u32 = 5053;
/// The volume the publisher publishes.
const VOLUME: &str = "repo";

#[host]
pub mod host {
    use super::*;
    use crate::TestSetup;
    use crate::measure::{HostMemory, Requests, Served};
    use crate::vm::{Firecracker, Vm};
    use anyhow::Context;
    use futures::executor::block_on;
    use fxdax::extentd::control::Client;
    use fxdax::{Image, Storage};
    use std::cell::Cell;
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    /// Makes the publisher's store at `path`: its default volume, and the volume it publishes.
    pub fn format(path: &Path, capacity: u64) -> anyhow::Result<()> {
        block_on(async {
            let image = Image::format(Storage::create(path, capacity)?).await?;
            for volume in ["default", VOLUME] {
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

    /// A published generation: a fork of the publisher's store, and the manifest of its volume.
    pub struct Generation {
        path: PathBuf,
        manifest: Vec<u8>,
    }

    /// The next generation of the publisher talking on `stream`, whose store is at `publisher`, at
    /// `path`: public, and the publisher told that it is forked.
    pub fn next_generation(
        stream: &mut UnixStream,
        control: &Client,
        publisher: &Path,
        path: PathBuf,
    ) -> anyhow::Result<Generation> {
        let manifest = receive(stream)?;
        control.fork_store(publisher, &path)?;
        control.make_generation_public(&path)?;
        stream.write_all(b"forked\n")?;
        Ok(Generation { path, manifest })
    }

    /// Attaches the `generations` the consumer on `stream`, whose store is at `consumer`, asks for,
    /// by number, each time it does, and gives it their manifests, until it hangs up.
    pub fn attach_generations(
        stream: UnixStream,
        control: &Client,
        consumer: &Path,
        generations: &[Generation],
    ) -> anyhow::Result<()> {
        let mut answers = stream.try_clone()?;
        for line in BufReader::new(stream).lines() {
            let generation = &generations[line?.parse::<usize>()?];
            control.attach_base(consumer, &generation.path)?;
            send(&mut answers, &generation.manifest)?;
        }
        Ok(())
    }

    /// A test's VMs, which run one after the other, with `cpus` and `memory_mib` each.
    pub struct Vms<'a> {
        pub test_setup: &'a TestSetup,
        pub root: PathBuf,
        pub cpus: u8,
        pub memory_mib: u32,
        /// Their stores' capacity, and their DAX windows' size, none if 0.
        pub capacity: u64,
        pub window: u64,
        /// The socket of the extent daemon that serves their stores.
        pub extentd: PathBuf,
        /// What says the host's memory as their guests ask ([`crate::measure`]), if they do.
        pub memory: Option<HostMemory>,
        /// How many have booted.
        booted: Cell<usize>,
    }

    /// A VM of [`Vms`], running.
    pub struct Running {
        firecracker: Firecracker,
        /// The thread answering its guest's requests to be measured, if the VMs are; or their
        /// vsock's sockets, kept until it ends, if not.
        served: Result<Served, Requests>,
    }

    impl<'a> Vms<'a> {
        /// VMs of 2 vCPUs and 1 GiB each, on stores of `capacity` that the daemon at `extentd`
        /// serves, with DAX windows of `window`, not measured.
        pub fn new(
            test_setup: &'a TestSetup,
            root: PathBuf,
            extentd: PathBuf,
            capacity: u64,
            window: u64,
        ) -> Self {
            let booted = Cell::new(0);
            let (cpus, memory_mib, memory) = (2, 1024, None);
            Self { test_setup, root, cpus, memory_mib, capacity, window, extentd, memory, booted }
        }

        /// A VM of the guest's `role`, on the store at `store`, and the host's end of the
        /// connection it opens, if `talks`.
        pub fn run(
            &self,
            role: &str,
            store: &Path,
            talks: bool,
        ) -> anyhow::Result<(Running, Option<UnixStream>)> {
            self.boot(role, talks, |vm| {
                vm.extent_store_served("extents", store, self.capacity, self.window, &self.extentd);
                Ok(())
            })
        }

        /// A VM of the guest's `role`, with what `devices` gives it, as [`Vms::run`].
        pub fn boot(
            &self,
            role: &str,
            talks: bool,
            devices: impl FnOnce(&mut Vm) -> anyhow::Result<()>,
        ) -> anyhow::Result<(Running, Option<UnixStream>)> {
            let mut vm = Vm::new(self.cpus, self.memory_mib)?;
            vm.guest_kernel()?;
            devices(&mut vm)?;
            vm.env(&CString::new(format!("FXDAX_SHARED_VOLUMES={role}"))?);
            // One vsock, its Unix socket's path short, as they must be, and one for each VM's
            // Firecracker: for measuring on its port, and talking on `PORT`.
            let requests = Requests::new(&format!("shared-{role}"))?;
            vm.vsock(requests.uds());
            let listener = (talks)
                .then(|| UnixListener::bind(format!("{}_{PORT}", requests.uds().display())))
                .transpose()?;
            let api = Vm::api_socket(role);
            let mut firecracker =
                vm.spawn_api(&self.root, &self.test_setup.test_case, role, &api)?;
            let number = self.booted.get() + 1;
            self.booted.set(number);
            let pid = firecracker.pid();
            let served = match self.memory {
                Some(memory) => Ok(requests.serve(memory, number, pid)),
                None => Err(requests),
            };
            let stream = listener.map(|listener| firecracker.accept(&listener)).transpose()?;
            Ok((Running { firecracker, served }, stream))
        }

        /// A VM of `role` on the store at `store`, run to its end: a consumer of `generations`.
        pub fn run_consumer(
            &self,
            role: &str,
            store: &Path,
            control: &Client,
            generations: &[Generation],
        ) -> anyhow::Result<()> {
            let (vm, stream) = self.run(role, store, true)?;
            let stream = stream.context("the consumer's connection")?;
            attach_generations(stream, control, store, generations)?;
            vm.wait()
        }
    }

    impl Running {
        /// Waits for its VM to end, and passes on what its guest said.
        pub fn wait(self) -> anyhow::Result<()> {
            let (status, console) = self.firecracker.wait()?;
            crate::vm::split_console(std::fs::File::open(&console)?)?;
            if let Ok(served) = self.served {
                served.stop()?;
            }
            anyhow::ensure!(status.success(), "a VM exited {status}");
            Ok(())
        }
    }
}

#[guest]
pub mod guest {
    use super::*;

    use nix::libc;
    use std::collections::HashSet;
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Error, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd as _, IntoRawFd as _};
    use std::os::unix::fs::chown;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::Duration;

    /// The default volume, on whose directories the others mount.
    pub const MNT: &str = "/mnt/fxfs";
    /// Who runs Git: the publisher's user is the consumers'.
    pub const USER: u32 = 1000;
    /// `USER`'s home, in the default volume, where traces go too.
    pub const HOME: &str = "/mnt/fxfs/home";
    /// Where the publisher mounts the volume.
    pub const REPO: &str = "/mnt/fxfs/repo";
    /// `FIFREEZE` and `FITHAW`: `_IOWR('X', 119, int)` and `_IOWR('X', 120, int)`.
    pub const FIFREEZE: libc::c_ulong = 0xc004_5877;
    pub const FITHAW: libc::c_ulong = 0xc004_5878;
    /// fs/fxfs's FXFS_MANIFEST_MAX.
    const MANIFEST_MAX: usize = 65536;

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

    pub fn mount(volume: Option<&str>, target: &str) {
        try_mount(volume, target).unwrap_or_else(|error| panic!("mount {target}: {error}"));
    }

    pub fn unmount(target: &str) {
        let path = CString::new(target).unwrap();
        // SAFETY: The string outlives the call.
        let ret = unsafe { libc::umount(path.as_ptr()) };
        assert_eq!(ret, 0, "umount {target}: {}", Error::last_os_error());
    }

    pub fn ioctl(target: &str, request: libc::c_ulong) {
        let dir = File::open(target).unwrap();
        // SAFETY: Neither takes an argument it reads.
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
        assert_eq!(ret, 0, "{request:#x} on {target}: {}", Error::last_os_error());
    }

    pub fn try_flush(volume: &str) -> nix::Result<()> {
        let dir = File::open(volume).unwrap();
        // SAFETY: It takes nothing.
        unsafe { flush_ioctl(dir.as_raw_fd()) }.map(drop)
    }

    pub fn try_manifest(volume: &str) -> nix::Result<Vec<u8>> {
        let dir = File::open(volume).unwrap();
        let mut manifest = vec![0; MANIFEST_MAX];
        let mut request =
            Manifest { data: manifest.as_mut_ptr() as u64, size: MANIFEST_MAX as u64 };
        // SAFETY: `request` says what `manifest` has room for, and both outlive the call.
        unsafe { manifest_ioctl(dir.as_raw_fd(), &mut request) }?;
        manifest.truncate(request.size as usize);
        Ok(manifest)
    }

    pub fn try_import(name: &str, manifest: &[u8]) -> nix::Result<()> {
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
    pub fn git(dir: &str, args: &[&str]) -> String {
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
    pub fn connect() -> File {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        // SAFETY: The socket's descriptor, which the file owns from here on.
        unsafe { File::from_raw_fd(fd.into_raw_fd()) }
    }

    /// Has `dir`, and what is made in it, belong to `USER`.
    pub fn give(dir: &str) {
        fs::create_dir_all(dir).unwrap();
        chown(dir, Some(USER), Some(USER)).unwrap();
    }

    /// Mounts the volume to publish at `REPO`, for `USER`.
    pub fn mount_repo() {
        give(REPO);
        mount(Some(VOLUME), REPO);
        give(REPO);
    }

    /// What the publisher does for a first `git status` elsewhere to trust the index (doc-10): Git
    /// doesn't, for a file changed in the second the index was written or after, so the index is
    /// written again once that second is over.
    pub fn settle_index() {
        std::thread::sleep(Duration::from_millis(1100));
        git(REPO, &["update-index", "--refresh"]);
        git(REPO, &["update-index", "--force-write-index"]);
    }

    pub fn sync() {
        // SAFETY: It takes nothing.
        unsafe { libc::sync() };
    }

    /// Publishes the volume at `REPO` to the host, which forks the store, and tells this it did.
    /// Returns the size of its manifest.
    pub fn publish(host: &mut File) -> usize {
        sync();
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
        manifest.len()
    }

    /// Has the host attach generation `number`, and returns its manifest.
    pub fn attach_generation(host: &mut File, number: usize) -> Vec<u8> {
        writeln!(host, "{number}").unwrap();
        let mut size = [0; 8];
        host.read_exact(&mut size).unwrap();
        let mut manifest = vec![0; u64::from_le_bytes(size) as usize];
        host.read_exact(&mut manifest).unwrap();
        manifest
    }

    /// Runs `git status` on the repo at `path`, which must find it clean, and returns the paths it
    /// statted and those it opened.
    pub fn traced_status(path: &str) -> (HashSet<String>, HashSet<String>) {
        let trace = format!("{HOME}/status-{}", path.rsplit('/').next().unwrap());
        let status = command(
            "strace",
            &[
                "-f",
                "-qq",
                "-s",
                "4096",
                "-e",
                "trace=file",
                "-o",
                &trace,
                "git",
                "status",
                "--porcelain",
            ],
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

    /// The files the repo at `path` tracks.
    pub fn tracked_files(path: &str) -> HashSet<String> {
        git(path, &["ls-files", "-z"]).split_terminator('\0').map(str::to_owned).collect()
    }

    /// That the first `git status` of the repo at `path` finds it clean, lstatting every tracked
    /// file and opening none, so that it hashes none, as `USER`. Git reads `.gitignore`s, as its
    /// rules, tracked or not.
    pub fn check_first_status(path: &str) {
        let tracked = tracked_files(path);
        let (statted, opened) = traced_status(path);
        let missed: Vec<_> = tracked.difference(&statted).collect();
        assert!(missed.is_empty(), "not lstatted: {missed:?}");
        let hashed: Vec<_> =
            tracked.intersection(&opened).filter(|file| !file.ends_with(".gitignore")).collect();
        assert!(hashed.is_empty(), "opened: {hashed:?}");
    }

    pub fn drop_caches() {
        fs::write("/proc/sys/vm/drop_caches", b"3").unwrap();
    }
}
