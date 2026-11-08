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
    use fxfs_automount::upcall::{Answer, Newest};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    /// Makes the publisher's store at `path`: its default volume, and the volume it publishes.
    pub fn format(path: &Path, capacity: u64) -> anyhow::Result<()> {
        format_with(path, capacity, &[VOLUME])
    }

    /// Makes the publisher's store at `path`: its default volume, and `volumes`.
    pub fn format_with(path: &Path, capacity: u64, volumes: &[&str]) -> anyhow::Result<()> {
        block_on(async {
            let image = Image::format(Storage::create(path, capacity)?).await?;
            for volume in ["default"].iter().chain(volumes) {
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
    #[derive(Clone)]
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

    /// The sandbox manager, as the tests stand in for it, for VMs' automounters and
    /// `git-remote-fxfs` (TASK-136.6, 136.7): it answers their asks ([`fxfs_automount::upcall`]),
    /// attaching to their store the generation each name has, and naming the newest generation of
    /// each repo URL it has, refusing the rest, counting what each name and URL was asked.
    pub struct Manager {
        /// The daemon's control socket's, as [`crate::common::control`] finds it.
        extentd: PathBuf,
        names: BTreeMap<String, Generation>,
        newest: BTreeMap<String, String>,
        asked: Arc<Mutex<BTreeMap<String, usize>>>,
    }

    /// A [`Manager`]'s answers, for one VM's store, through the daemon's control API.
    struct Answering {
        control: Client,
        store: PathBuf,
        names: BTreeMap<String, Generation>,
        newest: BTreeMap<String, String>,
        asked: Arc<Mutex<BTreeMap<String, usize>>>,
    }

    impl Answering {
        fn count(&self, what: String) {
            *self.asked.lock().unwrap().entry(what).or_default() += 1;
        }
    }

    impl fxfs_automount::upcall::Manager for Answering {
        fn attach(&mut self, name: &str) -> Answer {
            self.count(name.to_owned());
            let Some(generation) = self.names.get(name) else {
                return Answer::Refused(format!("no {name} for this sandbox"));
            };
            self.control.attach_base(&self.store, &generation.path).expect("attaching");
            Answer::Manifest(generation.manifest.clone())
        }

        fn newest(&mut self, url: &str) -> Newest {
            self.count(format!("newest {url}"));
            match self.newest.get(url) {
                Some(name) => Newest::Named(name.clone()),
                None => Newest::Refused(format!("no generation of {url}")),
            }
        }
    }

    impl Manager {
        /// A manager attaching, by name, the generations `names` has, and naming the newest of
        /// each URL `newest` has, through the daemon at `extentd`.
        pub fn new(
            extentd: PathBuf,
            names: BTreeMap<String, Generation>,
            newest: BTreeMap<String, String>,
        ) -> Self {
            Self { extentd, names, newest, asked: Default::default() }
        }

        /// Answers, on a thread of its own, the asks of the VM of `role`, on the store at `store`,
        /// at the vsock port they ask at: listening before it boots.
        pub fn serve(&self, role: &str, store: &Path) -> anyhow::Result<()> {
            let path = Vms::vsock_port(role, fxfs_automount::PORT);
            self.serve_at(UnixListener::bind(path)?, store)
        }

        /// Answers, as [`Manager::serve`], the asks that come to `listener`.
        pub fn serve_at(&self, listener: UnixListener, store: &Path) -> anyhow::Result<()> {
            let mut answering = Answering {
                control: crate::common::control(&self.extentd)?,
                store: store.to_owned(),
                names: self.names.clone(),
                newest: self.newest.clone(),
                asked: self.asked.clone(),
            };
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let stream = stream.expect("a guest's connection");
                    if let Err(error) = fxfs_automount::upcall::answer(stream, &mut answering) {
                        eprintln!("a guest's asks: {error}");
                    }
                }
            });
            Ok(())
        }

        /// How many times each name, and each URL (`newest URL`), was asked for, by every VM
        /// served.
        pub fn asked(&self) -> BTreeMap<String, usize> {
            self.asked.lock().unwrap().clone()
        }
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
            let requests = Requests::new(&Self::vsock_name(role))?;
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

        /// The name of the vsock of the VM of `role` ([`Requests::uds_of`]).
        fn vsock_name(role: &str) -> String {
            format!("shared-{role}")
        }

        /// The path of the Unix socket the guest of the VM of `role` reaches the host's `port`
        /// through, as Firecracker's vsock has it: to listen on before it boots.
        pub fn vsock_port(role: &str, port: u32) -> PathBuf {
            PathBuf::from(format!("{}_{port}", Requests::uds_of(&Self::vsock_name(role)).display()))
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

    impl<'a> Vms<'a> {
        /// The snapshot at `snapshot` and `memory`, restored as the VM of `role`, its store where
        /// it was, resumed, and the host's end of the connection its guest makes on `PORT`.
        pub fn restore(
            &self,
            role: &str,
            snapshot: &Path,
            memory: &Path,
        ) -> anyhow::Result<(Running, UnixStream)> {
            let requests = Requests::new(&Self::vsock_name(role))?;
            let listener = UnixListener::bind(format!("{}_{PORT}", requests.uds().display()))?;
            let dir = self.root.parent().context("the root has no parent")?;
            let mut firecracker = Firecracker::spawn(dir, role, &Vm::api_socket(role), None, &[])?;
            let load = serde_json::json!({
                "snapshot_path": snapshot,
                "mem_backend": { "backend_type": "File", "backend_path": memory },
                "resume_vm": true,
                "vsock_override": { "uds_path": requests.uds() },
                "extent_overrides": [],
            });
            firecracker.request("PUT", "/snapshot/load", &load)?;
            let stream = firecracker.accept(&listener)?;
            Ok((Running { firecracker, served: Err(requests) }, stream))
        }
    }

    impl Running {
        /// Pauses its VM, snapshots it into `snapshot` and `memory`, and ends it, passing on what
        /// its guest said until then.
        pub fn snapshot(self, snapshot: &Path, memory: &Path) -> anyhow::Result<()> {
            let paused = serde_json::json!({ "state": "Paused" });
            self.firecracker.request("PATCH", "/vm", &paused)?;
            let create = serde_json::json!({
                "snapshot_type": "Full",
                "snapshot_path": snapshot,
                "mem_file_path": memory,
            });
            self.firecracker.request("PUT", "/snapshot/create", &create)?;
            let console = self.firecracker.kill()?;
            crate::vm::split_console(std::fs::File::open(&console)?)?;
            if let Ok(served) = self.served {
                served.stop()?;
            }
            Ok(())
        }

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

    pub fn try_mount(volume: Option<&str>, target: &str) -> std::io::Result<()> {
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
        command_with(program, args, dir, "/usr/bin:/bin")
    }

    /// Runs `program args` as [`git`] runs Git, with `path` its `PATH`.
    pub fn command_with(program: &str, args: &[&str], dir: &str, path: &str) -> String {
        let output = Command::new(program)
            .args(args)
            .current_dir(dir)
            .uid(USER)
            .gid(USER)
            .env("PATH", path)
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
        try_connect().expect("connecting to the host")
    }

    /// A connection to the host on `PORT`, if it listens.
    pub fn try_connect() -> nix::Result<File> {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)?;
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT))?;
        // SAFETY: The socket's descriptor, which the file owns from here on.
        Ok(unsafe { File::from_raw_fd(fd.into_raw_fd()) })
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
        publish_volume(host, REPO, &[MNT])
    }

    /// Publishes the volume at `volume` to the host, as [`publish`] does, the filesystem frozen
    /// with `others`, every other volume mounted.
    pub fn publish_volume(host: &mut File, volume: &str, others: &[&str]) -> usize {
        sync();
        ioctl(volume, FIFREEZE);
        try_flush(volume).expect("flushing");
        others.iter().for_each(|other| ioctl(other, FIFREEZE));
        let manifest = try_manifest(volume).expect("the manifest");
        host.write_all(&(manifest.len() as u64).to_le_bytes()).unwrap();
        host.write_all(&manifest).unwrap();
        let mut said = [0; 7];
        host.read_exact(&mut said).unwrap();
        assert_eq!(&said, b"forked\n");
        others.iter().rev().for_each(|other| ioctl(other, FITHAW));
        ioctl(volume, FITHAW);
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
