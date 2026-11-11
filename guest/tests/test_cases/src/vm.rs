// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The VM an fxdax test boots: fxdax's Firecracker (vendor/firecracker, with its fxdax feature),
//! as FXDAX_FIRECRACKER names it, booting fxdax's guest kernel, which this process runs until it
//! exits.
//!
//! The root filesystem is an ext4 image of the root directory, on the first virtio-blk disk, and
//! the guest agent is the guest's init (`guest_init`), which gets its environment and the test's
//! name on the kernel's command line. The guest's console, its serial port, goes to console.txt in
//! the test's directory, and once the VM is done, the guest agent's standard output, framed there,
//! to this process's standard output, and the rest, the agent's standard error and what the kernel
//! says, to its standard error. Firecracker logs to its own file there too.

use crate::loop_device::{LoopDevice, LoopIo};
use anyhow::{Context, ensure};
use std::ffi::{CStr, CString, OsStr};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Whether the VMM, fxdax's Firecracker, which FXDAX_FIRECRACKER names, is there to boot.
pub fn has_vmm() -> bool {
    std::env::var_os("FXDAX_FIRECRACKER").is_some()
}

/// fxdax: what a virtio-blk disk does with the guest's flushes, Firecracker's `cache_type`
/// (TASK-91.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskCache {
    /// Each flush a host fsync of the image: durable.
    Writeback,
    /// Flushes dropped: the sandbox team's runtime, which a host crash loses writes of.
    Unsafe,
}

impl DiskCache {
    /// Its name in Firecracker's configuration.
    fn name(self) -> &'static str {
        match self {
            DiskCache::Writeback => "Writeback",
            DiskCache::Unsafe => "Unsafe",
        }
    }
}

/// A VM, as it is configured, until it boots: Firecracker's configuration.
pub struct Vm {
    config: serde_json::Value,
    /// The kernel's arguments the test adds: the guest agent's environment (`NAME=value`), and
    /// module parameters (`module.name=value`).
    args: Vec<CString>,
    /// The disks and pmem devices added.
    disks: usize,
    pmems: usize,
    /// Whether the root filesystem is read-only.
    read_only_root: bool,
    /// The root filesystem's image's name, in the root's parent directory.
    root_image: String,
    /// The host's CPUs its Firecracker runs on, if not any.
    cpus: Vec<usize>,
    /// fxdax: the extent store whose Fxfs is the root filesystem, by its tag, if not an image.
    fxfs_root: Option<String>,
    /// fxdax: a line of the guest agent's output at which the VM process is killed ([`Vm::kill_on`]).
    kill_on: Option<String>,
    /// fxdax: how the VM process is to exit ([`Vm::stops_with`]).
    stops_with: Option<i32>,
    /// fxdax: a process killed at that line in place of the VM process.
    kill_instead: Option<u32>,
    /// fxdax: the loop devices its disks are on, held until the VM is done ([`Vm::loop_disk`]).
    loops: Vec<LoopDevice>,
}

/// The guest kernel's command line, before the guest's environment and the test's name: the
/// console on the serial port, saying what is a warning or worse (to the test's standard error:
/// `split_console`); a reset on a panic, which ends the VM; and the guest agent as init. fxdax: a
/// task blocked for 10 s says so, with its stack, on the console, before the runner's timeout
/// kills a hung test (TASK-44.13); a soft lockup panics, printing every task's state first
/// (panic_print's bit 0), runnable ones too: two vCPUs looping in the kernel starve the task they
/// wait on, which no guest watchdog then runs to name, and the test waited for its timeout
/// (TASK-113). It says what every CPU is doing before that, as the task holding what the stuck
/// CPU waits for is on another, whose state the tasks' dump doesn't show (TASK-133). An oops
/// panics too, as fxdax's kernel is configured to (fxdax.config), rather than leave the locks its
/// task held held, and every CPU locked up on them (TASK-311).
const CMDLINE: &str = "console=ttyS0 loglevel=5 reboot=k panic=-1 8250.nr_uarts=1 i8042.noaux i8042.nomux \
     i8042.dumbkbd sysctl.kernel.hung_task_timeout_secs=10 softlockup_panic=1 panic_print=0x1 \
     sysctl.kernel.softlockup_all_cpu_backtrace=1 init=/guest-agent";

impl Vm {
    /// A VM of `cpus` vCPUs and `ram_mib` MiB of memory.
    pub fn new(cpus: u8, ram_mib: u32) -> anyhow::Result<Self> {
        // fxdax: FXDAX_GUEST_HUGE_PAGES=2M backs its memory with the host's huge pages
        // (scripts/test-e2e.sh, TASK-91.16).
        let huge_pages = match std::env::var("FXDAX_GUEST_HUGE_PAGES").as_deref() {
            Ok("2M") => "2M",
            Ok(other) => anyhow::bail!("FXDAX_GUEST_HUGE_PAGES is 2M or unset, not {other:?}"),
            Err(_) => "None",
        };
        let config = serde_json::json!({
            "machine-config": {
                "vcpu_count": cpus,
                "mem_size_mib": ram_mib,
                "huge_pages": huge_pages,
            },
            "drives": [],
            "pmem": [],
            "extent": [],
        });
        Ok(Self {
            config,
            args: Vec::new(),
            disks: 0,
            pmems: 0,
            read_only_root: false,
            root_image: "rootfs.ext4".into(),
            cpus: Vec::new(),
            fxfs_root: None,
            kill_on: None,
            stops_with: None,
            kill_instead: None,
            loops: Vec::new(),
        })
    }

    /// Boots the kernel fxdax's scripts/build-guest-kernel.sh builds, which FXDAX_GUEST_KERNEL
    /// names.
    pub fn guest_kernel(&mut self) -> anyhow::Result<()> {
        let kernel = std::env::var("FXDAX_GUEST_KERNEL").context("FXDAX_GUEST_KERNEL not set")?;
        self.config["boot-source"]["kernel_image_path"] = kernel.into();
        Ok(())
    }

    /// Adds an extent-store device tagged `tag`, on the store at `path`, which it makes, holding
    /// at most `capacity` bytes, if it doesn't exist; with a DAX window of `window` bytes if not
    /// 0. The extent daemon at FXDAX_EXTENTD serves the store.
    pub fn extent_store(
        &mut self,
        tag: &str,
        path: &Path,
        capacity: u64,
        window: u64,
    ) -> anyhow::Result<()> {
        let extentd = std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?;
        self.extent_store_served(tag, path, capacity, window, Path::new(&extentd));
        Ok(())
    }

    /// fxdax: adds an extent-store device tagged `tag` that follows the store at `path`, which
    /// another VM writes, read-only (backlog doc-8), with a DAX window of `window` bytes if not 0.
    /// The extent daemon at FXDAX_EXTENTD serves the store.
    pub fn extent_store_following(
        &mut self,
        tag: &str,
        path: &Path,
        window: u64,
    ) -> anyhow::Result<()> {
        let extentd = std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?;
        self.extent_store_following_served(tag, path, window, Path::new(&extentd));
        Ok(())
    }

    /// Adds a device as [`Vm::extent_store_following`] does, served by the extent daemon at
    /// `extentd`.
    pub fn extent_store_following_served(
        &mut self,
        tag: &str,
        path: &Path,
        window: u64,
        extentd: &Path,
    ) {
        self.extent_store_served(tag, path, 0, window, extentd);
        let devices = self.config["extent"].as_array_mut().expect("an array");
        devices.last_mut().expect("the device added")["read_only"] = true.into();
    }

    /// Adds an extent-store device as [`Vm::extent_store`] does, served by the extent daemon at
    /// `extentd`.
    pub fn extent_store_served(
        &mut self,
        tag: &str,
        path: &Path,
        capacity: u64,
        window: u64,
        extentd: &Path,
    ) {
        let mut device = serde_json::json!({
            "id": tag,
            "tag": tag,
            "path_on_host": path,
            "capacity": capacity,
            "extentd_socket": extentd,
        });
        if window != 0 {
            device["dax_window_size"] = window.into();
            // fxdax: the window's private pages at most, FXDAX_DAX_PRIVATE_MAX if set; the
            // device's default otherwise, half its pages (TASK-164.11).
            if let Some(most) =
                std::env::var("FXDAX_DAX_PRIVATE_MAX").ok().filter(|m| !m.is_empty())
            {
                let most: u64 = most.parse().expect("FXDAX_DAX_PRIVATE_MAX is a count of pages");
                device["dax_private_max"] = most.into();
            }
        }
        push(&mut self.config["extent"], device);
    }

    /// Adds a writable virtio-pmem device on `image`, and returns its device in the guest.
    pub fn pmem(&mut self, id: &str, image: &Path) -> anyhow::Result<String> {
        let device = format!("/dev/pmem{}", self.pmems);
        self.pmems += 1;
        push(&mut self.config["pmem"], serde_json::json!({ "id": id, "path_on_host": image }));
        Ok(device)
    }

    /// Adds a writable virtio-blk disk on the raw `image`, which the host's page cache caches and
    /// whose flushes reach the host's disk, and returns its device in the guest.
    pub fn disk(&mut self, id: &str, image: &Path) -> anyhow::Result<String> {
        self.disk_cached(id, image, DiskCache::Writeback)
    }

    /// fxdax: as [`Vm::disk`], its flushes as `cache` has them (TASK-91.8).
    pub fn disk_cached(
        &mut self,
        id: &str,
        image: &Path,
        cache: DiskCache,
    ) -> anyhow::Result<String> {
        // The root's disk is the first.
        let device = format!("/dev/vd{}", disk_letter(self.disks + 1));
        self.disks += 1;
        push(
            &mut self.config["drives"],
            serde_json::json!({
                "drive_id": id,
                "path_on_host": image,
                "is_root_device": false,
                "is_read_only": false,
                "cache_type": cache.name(),
            }),
        );
        Ok(device)
    }

    /// fxdax: as [`Vm::disk_cached`], on a loop device over the raw `image`, attached as `io`
    /// says, as a VM host's local disk images are (TASK-350.1). Says on standard error how: the
    /// device, whether it bypasses the page cache, the image's filesystem, and the cache mode.
    /// The device, which the VM holds, clears itself once the VM and this process are done.
    pub fn loop_disk(
        &mut self,
        id: &str,
        image: &Path,
        cache: DiskCache,
        io: LoopIo,
    ) -> anyhow::Result<String> {
        ensure!(
            std::env::var_os("FXDAX_FIRECRACKER_JAILER").is_none(),
            "a jailed Firecracker sees no loop device"
        );
        let device = LoopDevice::attach(image, io)?;
        eprintln!(
            "fxdax: disk {id} on {} over {} on {}, direct I/O {}, cache_type {}",
            device.path().display(),
            image.display(),
            crate::loop_device::filesystem_type(image)?,
            device.direct_io()?,
            cache.name(),
        );
        let guest = self.disk_cached(id, device.path(), cache)?;
        self.loops.push(device);
        Ok(guest)
    }

    /// Adds a virtio-vsock device, of which the guest is CID 3, and whose connections from the
    /// guest to the host (CID 2) on port P reach the Unix socket `uds_P` on the host, as
    /// Firecracker has them. `uds` must be short: Unix sockets' paths are at most 107 bytes.
    pub fn vsock(&mut self, uds: &Path) {
        self.config["vsock"] = serde_json::json!({ "guest_cid": 3, "uds_path": uds });
    }

    /// Has the root filesystem's disk read-only: as VMs restored from one snapshot share it.
    pub fn read_only_root(&mut self) {
        self.read_only_root = true;
    }

    /// Names the root filesystem's image, in the root's parent directory: one of its own for each
    /// of a test's VMs that run at once.
    pub fn root_image(&mut self, name: &str) {
        self.root_image = name.into();
    }

    /// fxdax: boots on the Fxfs of the extent store tagged `tag` as the root filesystem, mounted
    /// read-write, rather than on an ext4 image of the root directory: it holds the guest agent,
    /// at /guest-agent, which runs as init.
    pub fn fxfs_root(&mut self, tag: &str) {
        self.fxfs_root = Some(tag.into());
    }

    /// Runs its Firecracker, every thread of it, on the host's CPUs `cpus` only.
    pub fn cpus(&mut self, cpus: &[usize]) {
        self.cpus = cpus.to_owned();
    }

    /// fxdax: kills the VM process (SIGKILL) at once as `line` comes on its console, a line of the
    /// guest agent's output, as a host's crash would end it, wherever its guest's writes and its
    /// own requests were (TASK-119.6): [`Vm::enter`] returns as if it had exited of itself.
    pub fn kill_on(&mut self, line: &str) {
        self.kill_on = Some(line.to_owned());
    }

    /// fxdax: the VM stops itself, its VM process exiting with `code`, which [`Vm::enter`] takes as
    /// an exit of its own, and any other as a failure.
    pub fn stops_with(&mut self, code: i32) {
        self.stops_with = Some(code);
    }

    /// fxdax: kills the process `daemon`, the VM's extent daemon, at `line`, rather than the VM
    /// process, which exits as it finds its daemon gone: status 158, or [`Vm::enter`] fails.
    pub fn kill_daemon_on(&mut self, line: &str, daemon: u32) {
        self.kill_on = Some(line.to_owned());
        self.kill_instead = Some(daemon);
    }

    /// Sets `var`, `NAME=value`, in the guest agent's environment.
    pub fn env(&mut self, var: &CStr) {
        self.args.push(var.to_owned());
    }

    /// Sets the kernel's module parameter `param`, `module.name=value`, as it boots.
    pub fn module_param(&mut self, param: &CStr) {
        self.args.push(param.to_owned());
    }

    /// Boots the guest, with `root` as its root filesystem, running the guest agent there
    /// (`common::setup_rootfs` copies it) for `test_case`. It returns only if the VM cannot boot.
    pub fn enter(mut self, root: &Path, test_case: &str) -> anyhow::Result<()> {
        let dir = root.parent().context("the root has no parent")?.to_owned();
        let cpus = self.cpus.clone();
        let (kill_on, kill_instead) = (self.kill_on.clone(), self.kill_instead);
        let stops_with = self.stops_with;
        let loops = std::mem::take(&mut self.loops);
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join("firecracker.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        let mut args = vec!["--no-api".into(), "--config-file".into(), config_path.into()];
        args.extend(log_args(&dir.join("firecracker.log")));
        let console_path = dir.join("console.txt");
        let mut child = spawn(&args, &console_path, &cpus)?;
        let code = match kill_on {
            Some(line) => kill_on_line(&mut child, &console_path, &line, kill_instead)?
                .map_or(0, exit_code),
            None => exit_code(child.wait()?),
        };
        split_console(std::fs::File::open(&console_path)?)?;
        // Firecracker's done with them: they clear themselves.
        drop(loops);
        match stops_with {
            Some(expected) if code == expected => std::process::exit(0),
            Some(expected) => panic!("the VM process exited with {code}, not {expected}"),
            None => std::process::exit(code),
        }
    }

    /// A path for the API socket of the VM `name` of this process: in the temporary directory, as
    /// a socket's path is short (SUN_LEN), and a test's directory, named after its case, may not be.
    pub fn api_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-{}-{name}.api", std::process::id()))
    }

    /// Boots the guest as [`Vm::enter`] does, but in a Firecracker the test drives through its
    /// API, at `api`, whose files in the root's parent directory are named after `name`.
    pub fn spawn_api(
        mut self,
        root: &Path,
        test_case: &str,
        name: &str,
        api: &Path,
    ) -> anyhow::Result<Firecracker> {
        let dir = root.parent().context("the root has no parent")?.to_owned();
        let cpus = self.cpus.clone();
        let loops = std::mem::take(&mut self.loops);
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join(format!("{name}.json"));
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        let mut firecracker = Firecracker::spawn(&dir, name, api, Some(&config_path), &cpus)?;
        firecracker.loops = loops;
        Ok(firecracker)
    }

    /// Makes an ext4 image of `root`, and returns Firecracker's configuration, booting from it;
    /// or from its Fxfs root, if it has one.
    fn prepare(mut self, root: &Path, test_case: &str) -> anyhow::Result<serde_json::Value> {
        ensure!(
            self.config["boot-source"]["kernel_image_path"].is_string(),
            "Firecracker boots fxdax's guest kernel only"
        );
        let mut boot_args = match &self.fxfs_root {
            // Not a block device's: the kernel mounts it by its source, the device's tag.
            Some(tag) => format!("root={tag} rootfstype=fxfs rw {CMDLINE}"),
            None => {
                self.add_root_image(root)?;
                CMDLINE.to_owned()
            }
        };
        for var in &self.args {
            let var = var.to_str().context("the kernel's arguments are not UTF-8")?;
            ensure!(
                !var.contains(char::is_whitespace),
                "the kernel's command line cannot pass {var:?}"
            );
            boot_args.push(' ');
            boot_args.push_str(var);
        }
        // fxdax: FXDAX_GUEST_CMDLINE, the kernel's arguments to measure, such as
        // `cpuidle_haltpoll.force=1` (TASK-41).
        if let Ok(extra) = std::env::var("FXDAX_GUEST_CMDLINE")
            && !extra.trim().is_empty()
        {
            boot_args.push(' ');
            boot_args.push_str(extra.trim());
        }
        let deadline = std::env::var_os(crate::TEST_DEADLINE);
        if let Some(secs) = watchdog_secs(deadline.as_deref(), SystemTime::now())? {
            boot_args.push_str(&format!(" {}={secs}", crate::GUEST_WATCHDOG));
        }
        boot_args.push_str(" -- ");
        boot_args.push_str(test_case);
        self.config["boot-source"]["boot_args"] = boot_args.into();
        Ok(self.config)
    }

    /// Makes an ext4 image of `root`, and boots from it, its first disk.
    fn add_root_image(&mut self, root: &Path) -> anyhow::Result<()> {
        let dir = root.parent().context("the root has no parent")?;
        // What init mounts on.
        for mount_point in ["dev", "proc", "sys", "tmp"] {
            std::fs::create_dir_all(root.join(mount_point))?;
        }
        let image = dir.join(&self.root_image);
        // Room for the files, and some for the guest's own.
        let size = (tree_size(root)? * 5 / 4 + (256 << 20)).next_multiple_of(1 << 20);
        std::fs::File::create(&image)?.set_len(size)?;
        let status = std::process::Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(root)
            .arg(&image)
            .status()
            .context("running mkfs.ext4")?;
        ensure!(status.success(), "mkfs.ext4 of {} failed: {status}", root.display());
        self.config["drives"].as_array_mut().expect("an array").insert(
            0,
            serde_json::json!({
                "drive_id": "rootfs",
                "path_on_host": image,
                "is_root_device": true,
                "is_read_only": self.read_only_root,
            }),
        );
        Ok(())
    }
}

/// A Firecracker process a test drives through its API (TASK-39.1): its console in
/// `<name>.console.txt` and its log in `<name>.log`, in the test's directory.
pub struct Firecracker {
    child: std::process::Child,
    api: PathBuf,
    console: PathBuf,
    /// fxdax: the loop devices its disks are on, held as long as it is ([`Vm::loop_disk`]).
    #[expect(dead_code, reason = "never read: held, they stay attached as long as it runs")]
    loops: Vec<LoopDevice>,
}

impl Firecracker {
    /// A Firecracker with its API at `api`, booting the VM `config` names, if any: otherwise none
    /// yet, as one that loads a snapshot is; on the host's CPUs `cpus`, or any if none.
    pub fn spawn(
        dir: &Path,
        name: &str,
        api: &Path,
        config: Option<&Path>,
        cpus: &[usize],
    ) -> anyhow::Result<Self> {
        let mut args: Vec<std::ffi::OsString> = vec!["--api-sock".into(), api.into()];
        if let Some(config) = config {
            args.extend(["--config-file".into(), config.into()]);
        }
        args.extend(log_args(&dir.join(format!("{name}.log"))));
        let console = dir.join(format!("{name}.console.txt"));
        let child = spawn(&args, &console, cpus)?;
        let this = Self { child, api: api.to_owned(), console, loops: Vec::new() };
        // Once it listens.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::os::unix::net::UnixStream::connect(api).is_err() {
            ensure!(
                std::time::Instant::now() < deadline,
                "Firecracker's API at {} never came up",
                api.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(this)
    }

    /// The process ID of the VM's process, whose threads' times a test may sample.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Sends Firecracker's API a `method` request of `path`, with `body`, and fails unless it
    /// succeeds.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<()> {
        use std::io::{BufRead, BufReader, Read, Write};

        let body = serde_json::to_vec(body)?;
        let mut stream = std::os::unix::net::UnixStream::connect(&self.api)
            .with_context(|| format!("connecting to {}", self.api.display()))?;
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n",
            body.len()
        )?;
        stream.write_all(&body)?;
        let mut reply = BufReader::new(stream);
        let mut status = String::new();
        reply.read_line(&mut status)?;
        let mut len = 0;
        loop {
            let mut header = String::new();
            reply.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                len = value.trim().parse()?;
            }
        }
        let mut content = vec![0; len];
        reply.read_exact(&mut content)?;
        let code = status.split_whitespace().nth(1).unwrap_or_default();
        ensure!(
            code.starts_with('2'),
            "{method} {path}: {} {}",
            status.trim_end(),
            String::from_utf8_lossy(&content)
        );
        Ok(())
    }

    /// Whether the VM is running still.
    pub fn running(&mut self) -> anyhow::Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    /// The connection the VM's guest makes to `listener`: an error, rather than a wait for ever,
    /// once the VM exits without making it, as one whose guest panics, resetting it, does, which
    /// Firecracker exits from well (TASK-79.59). Its guest's console, which says why, goes to this
    /// process's output then. It waits for the one or the other, the listener's connection or the
    /// VM process's exit, which its pidfd says.
    pub fn accept(
        &mut self,
        listener: &std::os::unix::net::UnixListener,
    ) -> anyhow::Result<std::os::unix::net::UnixStream> {
        use nix::libc;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // SAFETY: Opens a pidfd of the VM's process, this one's child, which it hasn't waited for.
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.child.id(), 0) };
        ensure!(pidfd >= 0, "the VM process's pidfd: {}", std::io::Error::last_os_error());
        // SAFETY: Just opened, and owned here.
        let pidfd = unsafe { OwnedFd::from_raw_fd(pidfd as libc::c_int) };
        let ready = |fd: libc::c_int| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let mut polled = [ready(listener.as_raw_fd()), ready(pidfd.as_raw_fd())];
        loop {
            // SAFETY: Polls the two descriptors, which outlive the call.
            if unsafe { libc::poll(polled.as_mut_ptr(), 2, -1) } < 0 {
                let err = std::io::Error::last_os_error();
                ensure!(err.kind() == std::io::ErrorKind::Interrupted, "polling: {err}");
                continue;
            }
            // A guest that connected, then its VM exited, is told of with the exit: it is first.
            if polled[0].revents != 0 {
                return Ok(listener.accept()?.0);
            }
            if polled[1].revents != 0 {
                let status = self.child.wait()?;
                split_console(std::fs::File::open(&self.console)?)?;
                anyhow::bail!(
                    "the VM exited {status} before its guest connected: see {}",
                    self.console.display()
                );
            }
        }
    }

    /// Waits for the VM to end, and returns its exit status and its console.
    pub fn wait(mut self) -> anyhow::Result<(std::process::ExitStatus, PathBuf)> {
        Ok((self.child.wait()?, self.console))
    }

    /// Ends the VM, at once, and returns its console.
    pub fn kill(mut self) -> anyhow::Result<PathBuf> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(self.console)
    }
}

/// Firecracker's arguments that have it log to `log`.
fn log_args(log: &Path) -> Vec<std::ffi::OsString> {
    let level = std::env::var("FXDAX_FIRECRACKER_LOG_LEVEL")
        .ok()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "Warning".into());
    vec!["--log-path".into(), log.into(), "--level".into(), level.into()]
}

/// Starts fxdax's Firecracker with `args`, its console, its serial port, going to `console`, on
/// the host's CPUs `cpus`, or any if none; in a jail, as production runs it, with
/// FXDAX_FIRECRACKER_JAILER set ([`Jail`]).
/// The exit code a VM process's exit `status` is said with.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

/// fxdax: waits for the VM process `child` to exit, killing it (SIGKILL) at once if `line`, a line
/// of the guest agent's output, comes on its console at `console` first ([`Vm::kill_on`]): or, with
/// `daemon`, killing that, and waiting for the VM process to exit, as it does once its daemon goes
/// ([`Vm::kill_daemon_on`]). Returns its exit status, if it exited of itself.
fn kill_on_line(
    child: &mut std::process::Child,
    console: &Path,
    line: &str,
    daemon: Option<u32>,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    use std::io::Read;
    let line = format!("{line}\n").into_bytes();
    let mut file = std::fs::File::open(console)?;
    let mut seen = Vec::new();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let from = seen.len().saturating_sub(line.len());
        if file.read_to_end(&mut seen)? == 0 {
            // Its console grows as the VM runs: looked at again soon.
            std::thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }
        if seen[from..].windows(line.len()).any(|window| window == line) {
            let Some(daemon) = daemon else {
                child.kill()?;
                child.wait()?;
                return Ok(None);
            };
            // SAFETY: A plain system call, of a process of this test's.
            anyhow::ensure!(unsafe { nix::libc::kill(daemon as i32, nix::libc::SIGKILL) } == 0);
            let status = child.wait()?;
            anyhow::ensure!(status.code() == Some(158), "the VM exited {status}, not ExtentdLost");
            return Ok(None);
        }
    }
}

fn spawn(
    args: &[std::ffi::OsString],
    console: &Path,
    cpus: &[usize],
) -> anyhow::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    let firecracker = std::env::var_os("FXDAX_FIRECRACKER").context("FXDAX_FIRECRACKER not set")?;
    let mut jail = Jail::from_env()?;
    let mut command = match &mut jail {
        Some(jail) => jail.command(&firecracker, console)?,
        None => std::process::Command::new(&firecracker),
    };
    // A seccomp filter of its own, compiled (scripts/test-e2e.sh's FXDAX_FIRECRACKER_SECCOMP=log).
    if let Some(filter) = std::env::var_os("FXDAX_FIRECRACKER_SECCOMP_FILTER") {
        command.arg("--seccomp-filter").arg(filter);
    }
    // Every device on virtio-PCI, on every architecture: the extent device attaches over PCI
    // alone, its DAX window a BAR of its own, and MSI-X costs the guest no exits to read and
    // acknowledge an interrupt's status, which MMIO's do (TASK-91.15, TASK-267, TASK-273).
    command.arg("--enable-pci");
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        // A file, not a pipe: Firecracker drops what its serial port says when its standard
        // output would block.
        .stdout(std::fs::File::create(console)?);
    // Its threads, all made after it starts, run where it does.
    // SAFETY: a cpu_set_t is plain data, which CPU_ZERO and CPU_SET only write to.
    let mut set: nix::libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &cpu in cpus {
        ensure!(cpu < nix::libc::CPU_SETSIZE as usize, "no CPU {cpu}");
        // SAFETY: as above, cpu within the set.
        unsafe { nix::libc::CPU_SET(cpu, &mut set) };
    }
    let pinned = !cpus.is_empty();
    let binds = jail.map(|jail| jail.binds).unwrap_or_default();
    // Firecracker goes when this process does, as when the runner times the test out (but for a
    // jailed one, which the jailer's change of user makes forget it).
    // SAFETY: prctl, sched_setaffinity, unshare and mount are async-signal-safe, and the binds'
    // paths were made before.
    unsafe {
        command.pre_exec(move || {
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if pinned && nix::libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if !binds.is_empty() {
                Jail::mount(&binds)?;
            }
            Ok(())
        })
    };
    command.spawn().with_context(|| format!("executing {}", PathBuf::from(&firecracker).display()))
}

/// fxdax: a jail for Firecracker, as production runs it (TASK-50.1, TASK-50.4): Firecracker's
/// jailer, which FXDAX_FIRECRACKER_JAILER names, chroots it, makes its /dev/kvm and
/// /dev/userfaultfd, owned by the user it runs it as, FXDAX_JAIL_UID and FXDAX_JAIL_GID, and
/// drops every privilege. The directories Firecracker's paths are in, the test's, the guest
/// kernel's, the extent daemon's socket's and the temporary one, are bound into the jail at their
/// own paths, in a mount namespace of its own, so that its configuration's paths, and those its API
/// is told, are the same in it, and go with it; the test's files are given to its user.
struct Jail {
    jailer: std::ffi::OsString,
    uid: u32,
    gid: u32,
    /// The directories bound into the jail, from where, to where.
    binds: Vec<(CString, CString)>,
}

impl Jail {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(jailer) = std::env::var_os("FXDAX_FIRECRACKER_JAILER") else { return Ok(None) };
        let id = |name: &str| -> anyhow::Result<u32> {
            let value = std::env::var(name).with_context(|| format!("{name} not set"))?;
            value.parse().with_context(|| format!("{name} is not a number"))
        };
        let (uid, gid) = (id("FXDAX_JAIL_UID")?, id("FXDAX_JAIL_GID")?);
        Ok(Some(Self { jailer, uid, gid, binds: Vec::new() }))
    }

    /// The jailer's command running `firecracker`, whose console is `console`, in the test's
    /// directory, `console`'s.
    fn command(
        &mut self,
        firecracker: &std::ffi::OsStr,
        console: &Path,
    ) -> anyhow::Result<std::process::Command> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static JAILED: AtomicUsize = AtomicUsize::new(0);

        let dir = console.parent().context("the console is in no directory")?;
        let id = format!("{}-{}", std::process::id(), JAILED.fetch_add(1, Ordering::Relaxed));
        let base = dir.join("jail");
        let name = Path::new(firecracker).file_name().context("Firecracker has no file name")?;
        let root = base.join(name).join(&id).join("root");
        let mut dirs = vec![dir.to_owned(), std::env::temp_dir()];
        for var in ["FXDAX_GUEST_KERNEL", "FXDAX_EXTENTD", "FXDAX_FIRECRACKER_SECCOMP_FILTER"] {
            if let Some(path) = std::env::var_os(var) {
                dirs.extend(Path::new(&path).parent().map(Path::to_owned));
            }
        }
        // Each once, and none in another, which binds it already.
        dirs.sort();
        let mut bound: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if bound.iter().any(|outer| dir.starts_with(outer)) {
                continue;
            }
            let at = root.join(dir.strip_prefix("/").context("a relative path")?);
            std::fs::create_dir_all(&at)?;
            let path = |path: &Path| CString::new(path.as_os_str().as_encoded_bytes());
            self.binds.push((path(&dir)?, path(&at)?));
            bound.push(dir);
        }
        // Firecracker writes the test's files, and its log, as the jail's user.
        own(dir, self.uid, self.gid)?;
        let mut command = std::process::Command::new(&self.jailer);
        command
            .args(["--id", &id, "--uid", &self.uid.to_string(), "--gid", &self.gid.to_string()])
            .arg("--exec-file")
            .arg(firecracker)
            .arg("--chroot-base-dir")
            .arg(&base)
            .arg("--");
        Ok(command)
    }

    /// In a mount namespace of the child's own, binds `binds`' directories into the jail.
    fn mount(binds: &[(CString, CString)]) -> std::io::Result<()> {
        use nix::libc::{CLONE_NEWNS, MS_BIND, MS_PRIVATE, MS_REC, mount, unshare};
        let check = |ret: i32| if ret < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) };
        // SAFETY: plain system calls on paths that live as long as the child.
        unsafe {
            check(unshare(CLONE_NEWNS))?;
            let none = std::ptr::null();
            check(mount(none, c"/".as_ptr(), none, MS_REC | MS_PRIVATE, none.cast()))?;
            for (from, to) in binds {
                check(mount(from.as_ptr(), to.as_ptr(), none, MS_BIND | MS_REC, none.cast()))?;
            }
        }
        Ok(())
    }
}

/// Gives `path`, and what is under it, to `uid` and `gid`: but what goes meanwhile, as the test's
/// other VMs and daemons remove their files.
fn own(path: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    let gone = |err: &std::io::Error| err.kind() == std::io::ErrorKind::NotFound;
    match std::os::unix::fs::lchown(path, Some(uid), Some(gid)) {
        Err(err) if gone(&err) => return Ok(()),
        given => given.with_context(|| format!("giving {} to the jail's user", path.display()))?,
    }
    let entries = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::read_dir(path),
        Err(err) if !gone(&err) => return Err(err.into()),
        _ => return Ok(()),
    };
    let entries = match entries {
        Err(err) if gone(&err) => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        match entry {
            Ok(entry) => own(&entry.path(), uid, gid)?,
            Err(err) if gone(&err) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// The letter of the virtio-blk disk at `index`: the guest names them vda, vdb, and so on.
fn disk_letter(index: usize) -> char {
    char::from(b'a' + u8::try_from(index).expect("few disks"))
}

/// How long before the runner's deadline a guest still running dumps its stacks: time enough for
/// them to cross the console.
const WATCHDOG_MARGIN: Duration = Duration::from_secs(15);

/// fxdax: the seconds a guest booting at `now` has before it dumps its stacks, if the runner set a
/// deadline, `deadline` ([`crate::TEST_DEADLINE`]): a guest hung in the kernel can't say where
/// otherwise. None too close to the deadline.
fn watchdog_secs(deadline: Option<&OsStr>, now: SystemTime) -> anyhow::Result<Option<u64>> {
    let Some(var) = deadline else { return Ok(None) };
    let secs: u64 = var
        .to_str()
        .and_then(|secs| secs.parse().ok())
        .with_context(|| format!("{} is {var:?}, not seconds", crate::TEST_DEADLINE))?;
    let deadline = UNIX_EPOCH + Duration::from_secs(secs);
    Ok(deadline
        .duration_since(now)
        .ok()
        .and_then(|left| left.checked_sub(WATCHDOG_MARGIN))
        .map(|secs| secs.as_secs())
        .filter(|&secs| secs > 0))
}

fn push(array: &mut serde_json::Value, value: serde_json::Value) {
    array.as_array_mut().expect("an array").push(value);
}

/// Copies the guest agent's standard output, which `guest_init` frames on the guest's console,
/// `console`, to standard output, and the rest, its standard error and what the kernel says, to
/// standard error.
pub fn split_console(mut console: impl std::io::Read) -> anyhow::Result<()> {
    use std::io::Write;

    let (mut stdout, mut stderr) = (std::io::stdout().lock(), std::io::stderr().lock());
    let mut framing = Framing::default();
    let mut buffer = vec![0; 4096];
    loop {
        let n = match console.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        framing.split(&buffer[..n], &mut stdout, &mut stderr)?;
        stdout.flush()?;
        stderr.flush()?;
    }
}

/// fxdax: the guest agent's standard output, of what its console holds, `console`, as
/// [`split_console`] passes it on: for a test that reads what each of several VMs said
/// (fxfs-guest-soak, TASK-67).
pub fn guest_stdout(console: &[u8]) -> String {
    let (mut stdout, mut rest) = (Vec::new(), Vec::new());
    Framing::default().split(console, &mut stdout, &mut rest).expect("writes to memory");
    String::from_utf8_lossy(&stdout).into_owned()
}

/// Where a console's bytes go, as `guest_init` frames the agent's standard output on it: between
/// STDOUT_START and STDOUT_END to the agent's, all else to the rest.
#[derive(Default)]
struct Framing {
    in_stdout: bool,
}

impl Framing {
    /// Passes `bytes`, the console's next, to `stdout` and `rest`.
    fn split(
        &mut self,
        bytes: &[u8],
        stdout: &mut impl std::io::Write,
        rest: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        use crate::{STDERR_END, STDERR_START, STDOUT_END, STDOUT_START};
        let is_marker =
            |b: &u8| matches!(*b, STDOUT_START | STDOUT_END | STDERR_START | STDERR_END);
        for run in bytes.split_inclusive(is_marker) {
            let (bytes, marker) = match run.split_last() {
                Some((b, bytes)) if is_marker(b) => (bytes, Some(*b)),
                _ => (run, None),
            };
            if self.in_stdout {
                stdout.write_all(bytes)?;
            } else {
                rest.write_all(bytes)?;
            }
            if let Some(marker) = marker {
                self.in_stdout = marker == STDOUT_START;
            }
        }
        Ok(())
    }
}

/// fxdax: `output`, the guest agent's standard output, without the kernel's lines that came out in
/// the midst of it: the console carries the kernel's lines as it says them, between the bytes of
/// output it is still passing on, so that one, a stop's, went into the middle of an output line
/// (GCE r215, r221, r248). The kernel's begin with its timestamp, `[` seconds `.` microseconds `]`,
/// and end with "\r\n", which the agent's output, passed on as written, never has.
pub fn without_kernel_lines(output: &str) -> String {
    let mut kept = String::with_capacity(output.len());
    let mut rest = output;
    while let Some(at) = rest.find('[') {
        let (before, from) = rest.split_at(at);
        kept.push_str(before);
        match kernel_line(from) {
            Some(len) => rest = &from[len..],
            None => {
                kept.push('[');
                rest = &from[1..];
            }
        }
    }
    kept.push_str(rest);
    kept
}

/// The length of the kernel's line `text` begins with, if it does ([`without_kernel_lines`]).
fn kernel_line(text: &str) -> Option<usize> {
    let stamp = text.strip_prefix('[')?.trim_start_matches(' ');
    let (seconds, rest) = stamp.split_once('.')?;
    let (micros, _) = rest.split_once(']')?;
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(seconds) || micros.len() != 6 || !digits(micros) {
        return None;
    }
    Some(text.find("\r\n")? + 2)
}

/// The bytes the files under `dir` hold.
fn tree_size(dir: &Path) -> anyhow::Result<u64> {
    let mut size = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        size += metadata.len();
        if metadata.is_dir() {
            size += tree_size(&entry.path())?;
        }
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guest dumps its stacks the margin before the runner's deadline, if it has time to.
    #[test]
    fn watchdog_secs_leave_the_margin() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let at = |secs: &str| watchdog_secs(Some(OsStr::new(secs)), now);
        assert_eq!(at("1180").unwrap(), Some(165));
        assert_eq!(at("1016").unwrap(), Some(1));
        assert_eq!(at("1015").unwrap(), None);
        assert_eq!(at("900").unwrap(), None);
        assert_eq!(watchdog_secs(None, now).unwrap(), None);
        assert_eq!(
            at("soon").unwrap_err().to_string(),
            "FXDAX_TEST_DEADLINE is \"soon\", not seconds"
        );
    }

    /// fxdax: the agent's output, framed on the console among the kernel's lines and the agent's
    /// standard error, read back alone, across frames (TASK-67).
    #[test]
    fn guest_stdout_is_what_its_frames_hold() {
        use crate::{STDERR_END, STDERR_START, STDOUT_END, STDOUT_START};
        let mut console = b"[    0.1] booting\n".to_vec();
        console.extend([STDOUT_START]);
        console.extend(b"synced 0 1\nsyn");
        console.extend([STDOUT_END, STDERR_START]);
        console.extend(b"an error\n");
        console.extend([STDERR_END]);
        console.extend(b"[    0.2] the kernel\n");
        console.extend([STDOUT_START]);
        console.extend(b"ced 0 2\n");
        console.extend([STDOUT_END]);
        assert_eq!(guest_stdout(&console), "synced 0 1\nsynced 0 2\n");
        assert_eq!(guest_stdout(b"nothing framed\n"), "");
    }
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    /// A Firecracker of the test's: `program`, as its process, with a console of its own.
    fn firecracker(dir: &Path, program: &[&str]) -> Firecracker {
        let console = dir.join(format!("{}.console.txt", program[0]));
        std::fs::write(&console, "the guest's panic\n").unwrap();
        let child = std::process::Command::new(program[0]).args(&program[1..]).spawn().unwrap();
        Firecracker { child, api: dir.join("api"), console }
    }

    /// TASK-79.59: a VM that exits before its guest connects is an error at once, which names its
    /// console, not a wait for ever; one whose guest connects, its connection.
    #[test]
    fn accepts_end_once_their_vms_do() {
        let dir = tempdir::TempDir::new("accept").unwrap();
        let path = dir.path().join("vsock");
        let listener = UnixListener::bind(&path).unwrap();
        let mut gone = firecracker(dir.path(), &["true"]);
        let refused = gone.accept(&listener).map(drop).map_err(|err| err.to_string());
        let console = dir.path().join("true.console.txt");
        let said = format!(
            "the VM exited exit status: 0 before its guest connected: see {}",
            console.display()
        );
        assert_eq!(refused, Err(said));

        let mut running = firecracker(dir.path(), &["sleep", "60"]);
        let mut connecting = UnixStream::connect(&path).unwrap();
        let mut accepted = running.accept(&listener).unwrap();
        connecting.write_all(b"ready\n").unwrap();
        let mut said = [0; 6];
        accepted.read_exact(&mut said).unwrap();
        assert_eq!(&said, b"ready\n");
        running.kill().unwrap();
    }
    /// fxdax: the kernel's lines that came out in the midst of the agent's output go, the output
    /// whole again around them; brackets of the output's own stay.
    #[test]
    fn output_loses_the_kernels_lines_in_its_midst() {
        let output = "resized 904 27009\nresized 905 240[    3.814723] fxfs: Terminating journal\r\n08\n\
                      resizing 906 [    3.816289] fxfs: flush failed\r\n[    3.816289] \r\n21007\n\
                      [not the kernel's]\n[1.5] neither\r\n";
        assert_eq!(
            without_kernel_lines(output),
            "resized 904 27009\nresized 905 24008\nresizing 906 21007\n[not the kernel's]\n\
             [1.5] neither\r\n"
        );
    }

    /// TASK-311: a console's first report is its first complaint's, from the lines before it to
    /// its trace's end, however many soft lockups follow; an oops without "BUG:" a complaint.
    #[test]
    fn a_consoles_first_report_is_kept_whole() {
        let boot: String = (0..20).map(|at| format!("[    0.{at:02}] booting\n")).collect();
        let oops = "[   27.90] list_del corruption. prev->next should be ffff8880049e5a28, but was ffff8880049e5a00\n\
                    [   27.90] ------------[ cut here ]------------\n\
                    [   27.90] kernel BUG at lib/list_debug.c:62!\n\
                    [   27.90] Oops: invalid opcode: 0000 [#1] SMP NOPTI\n\
                    [   27.90] CPU: 3 UID: 0 PID: 812 Comm: docker-buildx Not tainted 6.18.54+ #1\n\
                    [   27.90] RIP: 0010:__list_del_entry_valid_or_report+0xb2/0xc0\n\
                    [   27.90] Call Trace:\n\
                    [   27.90]  list_lru_del+0x6a/0x110\n\
                    [   27.90]  workingset_update_node+0x5c/0x90\n\
                    [   27.90] ---[ end trace 0000000000000000 ]---\n";
        let lockups: String = (0..2000)
            .map(|at| {
                format!(
                    "[   {}.00] watchdog: BUG: soft lockup - CPU#{} stuck for 22s! [dockerd:{at}]\n\
                     [   {}.00]  smp_call_function_many_cond+0x11e/0x4b0\n",
                    50 + at,
                    at % 8,
                    50 + at
                )
            })
            .collect();
        let console = format!("{boot}{oops}{lockups}");
        // REPORT_BEFORE lines before the oops' own: the corruption's, BUG()'s and boot's.
        let before: String = (15..20).map(|at| format!("[    0.{at:02}] booting\n")).collect();
        assert_eq!(first_report(&console), Some(format!("{before}{oops}")));
        assert_eq!(first_report(&boot), None);
        // A report with no end, as a panic cuts one off, kept to REPORT_BYTES.
        let endless = format!("[ 1.0] BUG: unable to handle page fault\n{lockups}");
        let kept = first_report(&endless).unwrap();
        assert!(kept.len() <= REPORT_BYTES, "{} bytes kept", kept.len());
        assert!(kept.starts_with("[ 1.0] BUG: unable to handle page fault\n"), "{kept}");
        assert!(kept.ends_with("(the rest of the report, past 16 KiB, is in the console)\n"));
        let dir = tempdir::TempDir::new("console").unwrap();
        let path = dir.path().join("gpf.console.txt");
        let gpf = "[ 3.0] Oops: general protection fault, probably for non-canonical address 0xdead000000000122: 0000 [#1] SMP NOPTI";
        std::fs::write(&path, format!("{gpf}\n")).unwrap();
        assert_eq!(
            complaints(&path).map_err(|err| err.to_string()),
            Err(format!("a kernel complained: {:#?}", vec![gpf]))
        );
    }

    /// TASK-44.8: a console's complaints fail its test: a WARN_ON's, lockdep's, a KCSAN report of
    /// a race in fxdax's code; not KCSAN's of a race of the rest of the kernel's, nor of the race
    /// looked into (KCSAN_REVIEWED_CHANGE, KCSAN_REVIEWED_PEERS).
    #[test]
    fn consoles_fail_on_their_kernels_complaints() {
        let dir = tempdir::TempDir::new("console").unwrap();
        let checked = |name: &str, console: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, console).unwrap();
            complaints(&path).map_err(|err| err.to_string())
        };
        // A report of `functions`, with each access of `accesses`, "read (marked)" or the like,
        // and the function at its stack's top.
        let kcsan_of = |functions: &str, accesses: &[(&str, &str)], change: &str| {
            let accesses: String = (accesses.iter())
                .map(|(access, function)| {
                    format!(
                        "[ 1.0] {access} to 0xffffea0000306840 of 8 bytes by task 7 on cpu 0:\n\
                         [ 1.0]  {function}+0x10/0x20\n\
                         [ 1.0]  worker_thread+0x413/0x730\n\
                         [ 1.0] \n"
                    )
                })
                .collect();
            format!(
                "[ 1.0] ==================================================================\n\
                 [ 1.0] BUG: KCSAN: data-race in {functions}\n\
                 [ 1.0] \n\
                 {accesses}\
                 [ 1.0] value changed: {change}\n\
                 [ 1.0] ==================================================================\n"
            )
        };
        let kcsan = |functions: &str, change: &str| kcsan_of(functions, &[], change);
        assert_eq!(checked("quiet.console.txt", "[ 0.1] Linux version 6.18\n"), Ok(()));
        let warned = "[ 2.0] WARNING: CPU: 1 PID: 7 at fs/fxfs/dax.c:120 fxfs_dax_seal+0x1/0x2\n";
        assert_eq!(
            checked("warned.console.txt", warned),
            Err(format!("a kernel complained: {:#?}", vec![warned.trim_end()]))
        );
        // TASK-335: x86's CPU-vulnerability advisories, of the host's CPU, are none, on GitHub's
        // AMD runners as anywhere; a WARN_ON's report, the caller's id printed, and the
        // watchdog's soft lockup, behind its prefix, are.
        let advisories = "[    0.033593] Speculative Return Stack Overflow: WARNING: See \
                          https://kernel.org/doc/html/latest/admin-guide/hw-vuln/srso.html for \
                          mitigation options.\n\
                          [    0.033601] RETBleed: WARNING: Spectre v2 mitigation leaves CPU \
                          vulnerable to RETBleed attacks, data leaks possible!\n";
        assert_eq!(checked("advisories.console.txt", advisories), Ok(()));
        let caller =
            "[    3.5][    T1] WARNING: CPU: 0 PID: 1 at fs/fxfs/super.c:9 fxfs_fill+0x1/0x2\n";
        assert_eq!(
            checked("caller.console.txt", caller),
            Err(format!("a kernel complained: {:#?}", vec![caller.trim_end()]))
        );
        let lockup = "[   52.1] watchdog: BUG: soft lockup - CPU#3 stuck for 26s! [dockerd:812]\n";
        assert_eq!(
            checked("lockup.console.txt", lockup),
            Err(format!("a kernel complained: {:#?}", vec![lockup.trim_end()]))
        );
        let theirs = kcsan("alloc_pid / copy_process", "0x1 -> 0x2");
        assert_eq!(checked("theirs.console.txt", &theirs), Ok(()));
        let reviewed = kcsan("fxfs_dax_seal+0x10/0x20 / kthread_is_per_cpu", "0x40000 -> 0x0");
        assert_eq!(checked("reviewed.console.txt", &reviewed), Ok(()));
        // A folio's flags, which a reviewed peer reads plainly as fxdax's code changes them,
        // marked: its reads finishing (r565, r611), or a page_mkwrite's lock as writeback holds it
        // (r611), each changing the bits its peer's review allows.
        let peer = kcsan_of(
            "folio_batch_move_lru / fxfs_vfs_read_folios+0x1b3/0x1f0",
            &[("write (marked)", "fxfs_vfs_read_folios"), ("read", "folio_batch_move_lru")],
            "0x1 -> 0x8",
        );
        assert_eq!(checked("peer.console.txt", &peer), Ok(()));
        // The same race, as KCSAN named it for lru_add inlined into folio_batch_move_lru (r565).
        let inlined = kcsan_of(
            "fxfs_vfs_read_folios / lru_add",
            &[("write (marked)", "fxfs_vfs_read_folios"), ("read", "lru_add")],
            "0x000fffe000000001 -> 0x000fffe000000008",
        );
        assert_eq!(checked("inlined-peer.console.txt", &inlined), Ok(()));
        let generations = kcsan_of(
            "fxfs_vfs_read_folios / lru_gen_add_folio",
            &[("write (marked)", "fxfs_vfs_read_folios"), ("read", "lru_gen_add_folio")],
            "0x000fffe000000001 -> 0x000fffe000000008",
        );
        assert_eq!(checked("generations-peer.console.txt", &generations), Ok(()));
        let put = kcsan_of(
            "folios_put_refs / fxfs_vfs_read_folios",
            &[("write (marked)", "fxfs_vfs_read_folios"), ("read", "folios_put_refs")],
            "0x000fffe000000021 -> 0x000fffe000000028",
        );
        assert_eq!(checked("put-peer.console.txt", &put), Ok(()));
        let mkwrite = kcsan_of(
            "__lruvec_stat_mod_folio / fxfs_vfs_page_mkwrite",
            &[
                ("read-write (marked)", "fxfs_vfs_page_mkwrite"),
                ("read", "__lruvec_stat_mod_folio"),
            ],
            "0x000fffe00000412d -> 0x000fffe0000041ad",
        );
        assert_eq!(checked("mkwrite-peer.console.txt", &mkwrite), Ok(()));
        // The same, KCSAN naming zone_stat_mod_folio's read, inlined (TASK-286).
        let writeback = |change: &str| {
            kcsan_of(
                "folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite",
                &[
                    ("read-write (marked)", "fxfs_vfs_page_mkwrite"),
                    ("read", "folio_clear_dirty_for_io"),
                ],
                change,
            )
        };
        let waits = writeback("0x000fffe00000412d -> 0x000fffe0000041ad");
        assert_eq!(checked("writeback-peer.console.txt", &waits), Ok(()));
        // A page_mkwrite dirtying a folio that writeback holds locked, cleaning it: a race of
        // fxdax's, whatever its side's marking.
        let dirties = writeback("0x000fffe00000412d -> 0x000fffe00000413d");
        assert_eq!(
            checked("writeback-dirtied.console.txt", &dirties),
            Err(format!(
                "a kernel complained: {:#?}",
                vec![
                    "[ 1.0] BUG: KCSAN: data-race in folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite"
                ]
            ))
        );
        // fxdax's side plain, or not in the report: a race of its own, whatever its peer.
        let plain = kcsan_of(
            "fxfs_vfs_read_folios / lru_gen_add_folio",
            &[("write", "fxfs_vfs_read_folios"), ("read", "lru_gen_add_folio")],
            "0x1 -> 0x8",
        );
        let unsaid = kcsan("fxfs_vfs_read_folios / lru_gen_add_folio", "0x1 -> 0x8");
        for (name, console) in [("plain-peer.console.txt", plain), ("unsaid.console.txt", unsaid)] {
            assert_eq!(
                checked(name, &console),
                Err(format!(
                    "a kernel complained: {:#?}",
                    vec![
                        "[ 1.0] BUG: KCSAN: data-race in fxfs_vfs_read_folios / lru_gen_add_folio"
                    ]
                ))
            );
        }
        let ours =
            kcsan("fxfs_dax_region_open+0x10/0x20 / fxfs_dax_map_to+0x30/0x40", "0x0 -> 0x1");
        assert_eq!(
            checked("ours.console.txt", &ours),
            Err(format!(
                "a kernel complained: {:#?}",
                vec![
                    "[ 1.0] BUG: KCSAN: data-race in fxfs_dax_region_open+0x10/0x20 / fxfs_dax_map_to+0x30/0x40"
                ]
            ))
        );
    }
}

// fxdax: what a guest's kernel says of itself on its console (TASK-44.8).

/// The change of a word in a race of fxdax's code that KCSAN reports, looked into: a task's
/// flags, its PF_MEMALLOC_NOFS set or cleared, as fs/fxfs's NOFS scopes around its calls into its
/// Rust do (memalloc_nofs_save and restore, inlined), which other tasks read for other bits of the
/// word, read whole (the scheduler's kthread_is_per_cpu, for PF_KTHREAD; mem_cgroup_from_task).
const KCSAN_REVIEWED_CHANGE: u64 = 0x0004_0000;

/// A folio's flag bits (include/linux/page-flags.h's enum pageflags): its lock, which a holder
/// releases, and which a task that would take it marks waited for, waiting; and its contents' being
/// read in.
const PG_LOCKED: u64 = 1 << 0;
const PG_UPTODATE: u64 = 1 << 3;
const PG_WAITERS: u64 = 1 << 7;

/// The functions of the kernel's whose races with fxdax's code KCSAN reports, looked into, each
/// with the bits of the word that the race may change: each reads a folio's flags plainly, for a
/// field of the word that doesn't change while the folio is in the page cache (its zone, its
/// node), as fxdax's code changes the folio's flag bits through the folio API, atomically, as any
/// filesystem's does.
///
/// folio_batch_move_lru reads a folio new to the LRU as the worker that read it
/// (fxfs_vfs_read_folios) marks it uptodate and unlocks it, waking its waiters, as the reads of
/// any filesystem finishing on another thread race it; KCSAN names lru_add for the same race where
/// it's inlined into folio_batch_move_lru (GCE r565, TASK-168), and lru_gen_add_folio, the
/// multi-gen LRU's, for its folio_zonenum (r611, r613), and folios_put_refs, where
/// folio_batch_move_lru puts its batch, for its folio_is_zone_device (r617).
///
/// Writeback cleans a folio holding its lock (writeback_get_folio; folio_clear_dirty_for_io
/// asserts it) and, the folio clean, counts it so: __lruvec_stat_mod_folio reads its node
/// (folio_pgdat, r611), and zone_stat_mod_folio, inlined into folio_clear_dirty_for_io, its zone
/// (folio_zone, TASK-286). A store's page_mkwrite (fxfs_vfs_page_mkwrite) meanwhile takes the lock,
/// as filemap_page_mkwrite does, to dirty the folio under it (the exclusion
/// folio_clear_dirty_for_io's comment asks of a fault handler): its folio_trylock fails, marked,
/// and it waits, setting PG_waiters. Only the lock's bits change: a dirtying, or any change of the
/// folio's bits but its lock's, while writeback holds the lock, would be a race of fxdax's.
///
/// Reviewed only while fxdax's side of the race is marked: a plain access of its own to a folio's
/// flags would be a race of fxdax's, whichever of the kernel's functions met it (TASK-44.8.1).
const KCSAN_REVIEWED_PEERS: [(&str, u64); 6] = [
    ("folio_batch_move_lru", PG_LOCKED | PG_UPTODATE | PG_WAITERS),
    ("lru_add", PG_LOCKED | PG_UPTODATE | PG_WAITERS),
    ("lru_gen_add_folio", PG_LOCKED | PG_UPTODATE | PG_WAITERS),
    ("folios_put_refs", PG_LOCKED | PG_UPTODATE | PG_WAITERS),
    ("__lruvec_stat_mod_folio", PG_LOCKED | PG_WAITERS),
    ("folio_clear_dirty_for_io", PG_LOCKED | PG_WAITERS),
];

/// Whether the kernel's function `function` is fxdax's code's: fs/fxfs's, its Rust's
/// (whose mangled names name its crates, fxfs and fxfs_linux), or virtio_extent's.
///
/// A debug kernel's KCSAN reports races in all of the kernel; a test fails on those of fxdax's
/// code: a report naming a function of fs/fxfs's, virtio_extent's or their Rust's,
/// whether both of its accesses are, or one is at unknown origin (one KCSAN doesn't
/// see: uninstrumented, as the Rust and the scheduler are, or the host's, writing shared memory).
/// But for its races whose change is KCSAN_REVIEWED_CHANGE's, looked into. The others are the
/// kernel's, which the tests leave to it. Those seen, each looked into: two of
/// the kernel's accesses, alloc_pid / copy_process (a PID namespace's flags), _find_next_bit /
/// gen_pool_alloc_algo_owner (genalloc's lockless bitmap, of fs/fxfs's DAX window),
/// folio_batch_move_lru / folios_put_refs (a folio's flags); at unknown origin,
/// kthread_is_per_cpu, wq_worker_running and kick_pool (a task's flags, CPU time and wake CPU,
/// which the scheduler writes), process_one_work (a work item of fs/fxfs's Rust runtime, rt.c, as
/// the Rust writes it: looked into apart from the followers' torture test), virtqueue_enable_cb,
/// virtqueue_get_buf_ctx_split and vring_interrupt (the used ring's index, which the device, the
/// host, writes as virtio has it).
fn ours(function: &str) -> bool {
    function.contains("fxfs")
        || function.starts_with("vext_")
        || function.starts_with("virtio_extent")
}

/// The function of a frame or of a KCSAN report's title, "FUNCTION[+OFFSET/LENGTH]".
fn function_of(frame: &str) -> &str {
    frame.trim().split('+').next().unwrap_or_default()
}

/// One of the accesses a KCSAN report says raced: the function at the top of its stack, and
/// whether the access was marked (an atomic, a bit operation, READ_ONCE) or plain.
struct KcsanAccess<'a> {
    function: &'a str,
    marked: bool,
}

/// The accesses of a KCSAN report, its lines after its title: each a line "read-write (marked) to
/// 0xADDRESS of 8 bytes by task 1 on cpu 0:", or "race at unknown origin, with read to ...", and
/// its stack on the lines after.
fn kcsan_accesses<'a>(report: &[&'a str]) -> Vec<KcsanAccess<'a>> {
    let text = |line: &'a str| line.split_once("] ").map_or(line, |(_, text)| text).trim();
    report
        .windows(2)
        .filter(|pair| text(pair[0]).contains(" to 0x") && text(pair[0]).ends_with(':'))
        .map(|pair| KcsanAccess {
            function: function_of(text(pair[1])),
            marked: text(pair[0]).contains("(marked)"),
        })
        .collect()
}

/// What a kernel says as it finds itself wrong, anywhere in a line: a BUG's, after a prefix too
/// (the watchdog's "watchdog: BUG: soft lockup"), an oops' (a BUG()'s, "kernel BUG at FILE:LINE!"
/// then "Oops: invalid opcode", or a general protection fault's, neither of which says "BUG:"),
/// lockdep's, KCSAN's, and a sleep in atomic context; and [`WARNING`], at a message's start.
const COMPLAINTS: [&str; 7] = [
    "BUG:",
    "Oops:",
    "possible circular locking",
    "possible recursive locking",
    "inconsistent lock state",
    "suspicious RCU usage",
    "sleeping function called from invalid context",
];

/// fxdax: what a WARN_ON's report ("WARNING: CPU: N PID: N at FILE:LINE") and lockdep's begin
/// their message with: a complaint there alone. x86's CPU-vulnerability advisories say it behind
/// their subsystem's prefix, of the host's CPU, not of the guest's code ("Speculative Return
/// Stack Overflow: WARNING: See ...", on GitHub's AMD runners), and failed every case there
/// (TASK-335).
const WARNING: &str = "WARNING:";

/// fxdax: whether a console's `line` complains ([`COMPLAINTS`], [`WARNING`]).
fn complains(line: &str) -> bool {
    message(line).starts_with(WARNING)
        || COMPLAINTS.iter().any(|complaint| line.contains(complaint))
}

/// fxdax: the kernel's message on a console's `line`: past its time, "[    1.234] ", and its
/// caller's, "[    T1] ", if printed.
fn message(line: &str) -> &str {
    let mut text = line.trim_start();
    while let Some((_, rest)) = text.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        text = rest.trim_start();
    }
    text
}

/// Fails with the lines of the console at `console` in which its kernel complains, but for
/// KCSAN's reports of races that aren't fxdax's code's, or were reviewed (see [`ours`]). The
/// runner checks every test's consoles so (TASK-44.8): a release kernel's WARN_ON in fs/fxfs, which
/// once hid a bug a stress test alone found (TASK-44.2), fails its test as a debug kernel's
/// lockdep, KCSAN and atomic-sleep reports do.
pub fn complaints(console: &Path) -> anyhow::Result<()> {
    let console = String::from_utf8_lossy(&std::fs::read(console)?).into_owned();
    let lines: Vec<&str> = console.lines().collect();
    let complained: Vec<&str> = complained(&lines).map(|at| lines[at]).collect();
    anyhow::ensure!(complained.is_empty(), "a kernel complained: {complained:#?}");
    Ok(())
}

/// How many of a console's lines before its first complaint [`first_report`] keeps: what a report
/// says before the line that names it, as a list's corruption does ("list_del corruption. ..."
/// then BUG()'s "cut here" and "kernel BUG at", before its "Oops:"), or lockdep its ruler.
const REPORT_BEFORE: usize = 8;

/// The most of a report [`first_report`] keeps, as LangSmith's host keeps of a guest's first fault
/// report: an oops' registers, its stack and the modules linked, with room to spare.
const REPORT_BYTES: usize = 16 << 10;

/// fxdax: the first report of a console's kernel, of its first complaint (as [`complaints`] finds
/// them): from REPORT_BEFORE lines before it to the end of its trace ("---[ end trace ...]---"),
/// REPORT_BYTES at most. A guest whose task died holding a lock prints its oops once, then soft
/// lockups for minutes, every CPU's, till a console's tail holds them alone: what went wrong
/// first is the one report to keep (TASK-311). None if its kernel didn't complain.
pub fn first_report(console: &str) -> Option<String> {
    let lines: Vec<&str> = console.lines().collect();
    let first = complained(&lines).next()?;
    let cut =
        format!("(the rest of the report, past {} KiB, is in the console)\n", REPORT_BYTES >> 10);
    let mut report = String::new();
    for line in &lines[first.saturating_sub(REPORT_BEFORE)..] {
        if report.len() + line.len() + 1 + cut.len() > REPORT_BYTES {
            report.push_str(&cut);
            break;
        }
        report.push_str(line);
        report.push('\n');
        if line.contains("---[ end ") {
            break;
        }
    }
    Some(report)
}

/// The indices of the lines of a console, `lines`, in which its kernel complains, but for KCSAN's
/// reports of races that aren't fxdax's code's, or were reviewed (see [`ours`]).
fn complained<'a>(lines: &'a [&'a str]) -> impl Iterator<Item = usize> + 'a {
    lines.iter().enumerate().filter_map(|(at, line)| {
        if !complains(line) {
            return None;
        }
        // "BUG: KCSAN: data-race in FUNCTION[+OFFSET/LENGTH][ / FUNCTION[+...]]", then the
        // report, to a line of '='s: its accesses, each with its stack, and "value changed:
        // 0xOLD -> 0xNEW".
        if let Some((_, functions)) = line.split_once("BUG: KCSAN: data-race in ") {
            let names: Vec<&str> = functions.split(" / ").map(function_of).collect();
            let of_ours = names.iter().any(|function| ours(function));
            let rest = &lines[at + 1..];
            let report = &rest
                [..rest.iter().position(|line| line.contains("==========")).unwrap_or(rest.len())];
            let accesses = kcsan_accesses(report);
            let ours_marked = accesses.iter().any(|access| ours(access.function))
                && accesses.iter().all(|access| access.marked || !ours(access.function));
            // The bits that changed, if KCSAN saw the word change.
            let changed = report
                .iter()
                .filter_map(|line| line.split_once("value changed: "))
                .find_map(|(_, change)| {
                    let (old, new) = change.split_once(" -> ")?;
                    let word =
                        |hex: &str| u64::from_str_radix(hex.trim().strip_prefix("0x")?, 16).ok();
                    Some(word(old)? ^ word(new)?)
                });
            let reviewed = ours_marked
                && KCSAN_REVIEWED_PEERS
                    .iter()
                    .any(|(peer, bits)| names.contains(peer) && changed.unwrap_or(0) & !bits == 0);
            if !of_ours || reviewed || changed == Some(KCSAN_REVIEWED_CHANGE) {
                return None;
            }
        }
        Some(at)
    })
}
