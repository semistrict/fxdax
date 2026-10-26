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

use anyhow::{Context, ensure};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

/// Whether the VMM, fxdax's Firecracker, which FXDAX_FIRECRACKER names, is there to boot.
pub fn has_vmm() -> bool {
    std::env::var_os("FXDAX_FIRECRACKER").is_some()
}

/// A VM, as it is configured, until it boots: Firecracker's configuration.
pub struct Vm {
    config: serde_json::Value,
    /// The guest's environment.
    env: Vec<CString>,
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
}

/// The guest kernel's command line, before the guest's environment and the test's name: the
/// console on the serial port, saying what is a warning or worse (to the test's standard error:
/// `split_console`); a reset on a panic, which ends the VM; and the guest agent as init. fxdax: a
/// task blocked for 10 s says so, with its stack, on the console, before the runner's timeout
/// kills a hung test (TASK-44.13).
const CMDLINE: &str = "console=ttyS0 loglevel=5 reboot=k panic=-1 8250.nr_uarts=1 i8042.noaux i8042.nomux \
     i8042.dumbkbd sysctl.kernel.hung_task_timeout_secs=10 init=/guest-agent";

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
            env: Vec::new(),
            disks: 0,
            pmems: 0,
            read_only_root: false,
            root_image: "rootfs.ext4".into(),
            cpus: Vec::new(),
            fxfs_root: None,
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
        self.extent_store_served(tag, path, 0, window, Path::new(&extentd));
        let devices = self.config["extent"].as_array_mut().expect("an array");
        devices.last_mut().expect("the device added")["read_only"] = true.into();
        Ok(())
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
                "cache_type": "Writeback",
            }),
        );
        Ok(device)
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

    /// Sets `var`, `NAME=value`, in the guest agent's environment.
    pub fn env(&mut self, var: &CStr) {
        self.env.push(var.to_owned());
    }

    /// Boots the guest, with `root` as its root filesystem, running the guest agent there
    /// (`common::setup_rootfs` copies it) for `test_case`. It returns only if the VM cannot boot.
    pub fn enter(self, root: &Path, test_case: &str) -> anyhow::Result<()> {
        let dir = root.parent().context("the root has no parent")?.to_owned();
        let cpus = self.cpus.clone();
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join("firecracker.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        let mut args = vec!["--no-api".into(), "--config-file".into(), config_path.into()];
        args.extend(log_args(&dir.join("firecracker.log")));
        let console_path = dir.join("console.txt");
        let status = spawn(&args, &console_path, &cpus)?.wait()?;
        split_console(std::fs::File::open(&console_path)?)?;
        std::process::exit(status.code().unwrap_or(1));
    }

    /// A path for the API socket of the VM `name` of this process: in the temporary directory, as
    /// a socket's path is short (SUN_LEN), and a test's directory, named after its case, may not be.
    pub fn api_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-{}-{name}.api", std::process::id()))
    }

    /// Boots the guest as [`Vm::enter`] does, but in a Firecracker the test drives through its
    /// API, at `api`, whose files in the root's parent directory are named after `name`.
    pub fn spawn_api(
        self,
        root: &Path,
        test_case: &str,
        name: &str,
        api: &Path,
    ) -> anyhow::Result<Firecracker> {
        let dir = root.parent().context("the root has no parent")?.to_owned();
        let cpus = self.cpus.clone();
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join(format!("{name}.json"));
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        Firecracker::spawn(&dir, name, api, Some(&config_path), &cpus)
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
        for var in &self.env {
            let var = var.to_str().context("the guest's environment is not UTF-8")?;
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
        let this = Self { child, api: api.to_owned(), console };
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

fn push(array: &mut serde_json::Value, value: serde_json::Value) {
    array.as_array_mut().expect("an array").push(value);
}

/// Copies the guest agent's standard output, which `guest_init` frames on the guest's console,
/// `console`, to standard output, and the rest, its standard error and what the kernel says, to
/// standard error.
pub fn split_console(mut console: impl std::io::Read) -> anyhow::Result<()> {
    use crate::{STDERR_END, STDERR_START, STDOUT_END, STDOUT_START};
    use std::io::Write;

    let (mut stdout, mut stderr) = (std::io::stdout().lock(), std::io::stderr().lock());
    let mut in_stdout = false;
    let mut buffer = vec![0; 4096];
    loop {
        let n = match console.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        let is_marker =
            |b: &u8| matches!(*b, STDOUT_START | STDOUT_END | STDERR_START | STDERR_END);
        for run in buffer[..n].split_inclusive(is_marker) {
            let (bytes, marker) = match run.split_last() {
                Some((b, bytes)) if is_marker(b) => (bytes, Some(*b)),
                _ => (run, None),
            };
            if in_stdout {
                stdout.write_all(bytes)?;
            } else {
                stderr.write_all(bytes)?;
            }
            if let Some(marker) = marker {
                in_stdout = marker == STDOUT_START;
            }
        }
        stdout.flush()?;
        stderr.flush()?;
    }
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
    /// TASK-44.8: a console's complaints fail its test: a WARN_ON's, lockdep's, a KCSAN report of
    /// a race in fxdax's code; not KCSAN's of a race of the rest of the kernel's, nor of the race
    /// looked into (KCSAN_REVIEWED_CHANGE).
    #[test]
    fn consoles_fail_on_their_kernels_complaints() {
        let dir = tempdir::TempDir::new("console").unwrap();
        let checked = |name: &str, console: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, console).unwrap();
            complaints(&path).map_err(|err| err.to_string())
        };
        let kcsan = |functions: &str, change: &str| {
            format!(
                "[ 1.0] ==================================================================\n\
                 [ 1.0] BUG: KCSAN: data-race in {functions}\n\
                 [ 1.0] value changed: {change}\n\
                 [ 1.0] ==================================================================\n"
            )
        };
        assert_eq!(checked("quiet.console.txt", "[ 0.1] Linux version 6.18\n"), Ok(()));
        let warned = "[ 2.0] WARNING: CPU: 1 PID: 7 at fs/fxfs/dax.c:120 fxfs_dax_seal+0x1/0x2\n";
        assert_eq!(
            checked("warned.console.txt", warned),
            Err(format!("a kernel complained: {:#?}", vec![warned.trim_end()]))
        );
        let theirs = kcsan("alloc_pid / copy_process", "0x1 -> 0x2");
        assert_eq!(checked("theirs.console.txt", &theirs), Ok(()));
        let reviewed = kcsan("fxfs_dax_seal+0x10/0x20 / kthread_is_per_cpu", "0x40000 -> 0x0");
        assert_eq!(checked("reviewed.console.txt", &reviewed), Ok(()));
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

/// What a kernel says as it finds itself wrong: a WARN_ON's, a BUG's, lockdep's, KCSAN's, and
/// a sleep in atomic context.
const COMPLAINTS: [&str; 7] = [
    "WARNING:",
    "BUG:",
    "possible circular locking",
    "possible recursive locking",
    "inconsistent lock state",
    "suspicious RCU usage",
    "sleeping function called from invalid context",
];

/// Fails with the lines of the console at `console` in which its kernel complains, but for
/// KCSAN's reports of races that aren't fxdax's code's, or were reviewed (see [`ours`]). The
/// runner checks every test's consoles so (TASK-44.8): a release kernel's WARN_ON in fs/fxfs, which
/// once hid a bug a stress test alone found (TASK-44.2), fails its test as a debug kernel's
/// lockdep, KCSAN and atomic-sleep reports do.
pub fn complaints(console: &Path) -> anyhow::Result<()> {
    let console = String::from_utf8_lossy(&std::fs::read(console)?).into_owned();
    let lines: Vec<&str> = console.lines().collect();
    let mut complained = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if !COMPLAINTS.iter().any(|complaint| line.contains(complaint)) {
            continue;
        }
        // "BUG: KCSAN: data-race in FUNCTION[+OFFSET/LENGTH][ / FUNCTION[+...]]", then the
        // report, to a line of '='s, which says "value changed: 0xOLD -> 0xNEW".
        if let Some((_, functions)) = line.split_once("BUG: KCSAN: data-race in ") {
            let of_ours = (functions.split(" / "))
                .any(|function| ours(function.trim().split('+').next().unwrap_or_default()));
            let report = lines[at + 1..].iter().take_while(|line| !line.contains("=========="));
            let changed = report.filter_map(|line| line.split_once("value changed: ")).find_map(
                |(_, change)| {
                    let (old, new) = change.split_once(" -> ")?;
                    let word =
                        |hex: &str| u64::from_str_radix(hex.trim().strip_prefix("0x")?, 16).ok();
                    Some(word(old)? ^ word(new)?)
                },
            );
            if !of_ours || changed == Some(KCSAN_REVIEWED_CHANGE) {
                continue;
            }
        }
        complained.push(*line);
    }
    anyhow::ensure!(complained.is_empty(), "a kernel complained: {complained:#?}");
    Ok(())
}
