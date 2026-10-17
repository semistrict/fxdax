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
}

/// The guest kernel's command line, before the guest's environment and the test's name: the
/// console on the serial port, saying what is a warning or worse (to the test's standard error:
/// `split_console`); a reset on a panic, which ends the VM; and the guest agent as init.
const CMDLINE: &str = "console=ttyS0 loglevel=5 reboot=k panic=-1 8250.nr_uarts=1 i8042.noaux i8042.nomux \
     i8042.dumbkbd init=/guest-agent";

impl Vm {
    /// A VM of `cpus` vCPUs and `ram_mib` MiB of memory.
    pub fn new(cpus: u8, ram_mib: u32) -> anyhow::Result<Self> {
        let config = serde_json::json!({
            "machine-config": { "vcpu_count": cpus, "mem_size_mib": ram_mib },
            "drives": [],
            "pmem": [],
            "extent": [],
        });
        Ok(Self { config, env: Vec::new(), disks: 0, pmems: 0, read_only_root: false })
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

    /// Sets `var`, `NAME=value`, in the guest agent's environment.
    pub fn env(&mut self, var: &CStr) {
        self.env.push(var.to_owned());
    }

    /// Boots the guest, with `root` as its root filesystem, running the guest agent there
    /// (`common::setup_rootfs` copies it) for `test_case`. It returns only if the VM cannot boot.
    pub fn enter(self, root: &Path, test_case: &str) -> anyhow::Result<()> {
        let dir = root.parent().context("the root has no parent")?.to_owned();
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join("firecracker.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        let mut args = vec!["--no-api".into(), "--config-file".into(), config_path.into()];
        args.extend(log_args(&dir.join("firecracker.log")));
        let console_path = dir.join("console.txt");
        let status = spawn(&args, &console_path)?.wait()?;
        split_console(std::fs::File::open(&console_path)?)?;
        std::process::exit(status.code().unwrap_or(1));
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
        let config = self.prepare(root, test_case)?;
        let config_path = dir.join(format!("{name}.json"));
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        Firecracker::spawn(&dir, name, api, Some(&config_path))
    }

    /// Makes an ext4 image of `root`, and returns Firecracker's configuration, booting from it.
    fn prepare(mut self, root: &Path, test_case: &str) -> anyhow::Result<serde_json::Value> {
        ensure!(
            self.config["boot-source"]["kernel_image_path"].is_string(),
            "Firecracker boots fxdax's guest kernel only"
        );
        let dir = root.parent().context("the root has no parent")?;
        // What init mounts on.
        for mount_point in ["dev", "proc", "sys", "tmp"] {
            std::fs::create_dir_all(root.join(mount_point))?;
        }
        let image = dir.join("rootfs.ext4");
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

        let mut boot_args = CMDLINE.to_owned();
        for var in &self.env {
            let var = var.to_str().context("the guest's environment is not UTF-8")?;
            ensure!(
                !var.contains(char::is_whitespace),
                "the kernel's command line cannot pass {var:?}"
            );
            boot_args.push(' ');
            boot_args.push_str(var);
        }
        boot_args.push_str(" -- ");
        boot_args.push_str(test_case);
        self.config["boot-source"]["boot_args"] = boot_args.into();
        self.config["drives"].as_array_mut().expect("an array").insert(
            0,
            serde_json::json!({
                "drive_id": "rootfs",
                "path_on_host": image,
                "is_root_device": true,
                "is_read_only": self.read_only_root,
            }),
        );
        Ok(self.config)
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
    /// yet, as one that loads a snapshot is.
    pub fn spawn(dir: &Path, name: &str, api: &Path, config: Option<&Path>) -> anyhow::Result<Self> {
        let mut args: Vec<std::ffi::OsString> = vec!["--api-sock".into(), api.into()];
        if let Some(config) = config {
            args.extend(["--config-file".into(), config.into()]);
        }
        args.extend(log_args(&dir.join(format!("{name}.log"))));
        let console = dir.join(format!("{name}.console.txt"));
        let child = spawn(&args, &console)?;
        let this = Self { child, api: api.to_owned(), console };
        // Once it listens.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::os::unix::net::UnixStream::connect(api).is_err() {
            ensure!(std::time::Instant::now() < deadline, "Firecracker's API at {} never came up", api.display());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(this)
    }

    /// Sends Firecracker's API a `method` request of `path`, with `body`, and fails unless it
    /// succeeds.
    pub fn request(&self, method: &str, path: &str, body: &serde_json::Value) -> anyhow::Result<()> {
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

    /// Waits for the VM to end, and returns its exit status and its console.
    pub fn wait(mut self) -> anyhow::Result<(std::process::ExitStatus, PathBuf)> {
        Ok((self.child.wait()?, self.console))
    }

    /// Ends the VM, at once.
    pub fn kill(mut self) -> anyhow::Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
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

/// Starts fxdax's Firecracker with `args`, its console, its serial port, going to `console`.
fn spawn(args: &[std::ffi::OsString], console: &Path) -> anyhow::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    let firecracker = std::env::var_os("FXDAX_FIRECRACKER").context("FXDAX_FIRECRACKER not set")?;
    let mut command = std::process::Command::new(&firecracker);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        // A file, not a pipe: Firecracker drops what its serial port says when its standard
        // output would block.
        .stdout(std::fs::File::create(console)?);
    // Firecracker goes when this process does, as when the runner times the test out.
    // SAFETY: prctl is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    };
    command
        .spawn()
        .with_context(|| format!("executing {}", PathBuf::from(&firecracker).display()))
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
