// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The automounter on Linux: autofs's requests taken and answered, the guest's Fxfs's volumes,
//! and the sandbox manager asked over vsock.

use crate::autofs::{Map, Request};
use crate::daemon::{Attach, Automounter, Volumes};
use crate::upcall::{self, Answer};
use anyhow::Context as _;
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the automounter mounts, and when it expires what it mounted.
pub struct Config {
    /// The directory the names are looked up in, whose entries it mounts.
    pub root: PathBuf,
    /// How long a mount stays idle before it expires.
    pub timeout: Duration,
}

/// Runs the automounter of `config`'s root, of `volumes` that `attach` attaches, until its map
/// fails. This process becomes a group of its own first, as autofs lets its own group alone
/// through the root unmounted: what else ran in the group it was in would see the root empty.
pub fn run(config: &Config, volumes: impl Volumes, attach: impl Attach) -> io::Result<()> {
    // SAFETY: Makes this process a group of its own.
    if unsafe { libc::setpgid(0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut map = Map::mount(&config.root, config.timeout)?;
    let expirer = map.expirer()?;
    let period = (config.timeout / 4).max(Duration::from_secs(1));
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(period);
            loop {
                match expirer.expire_one() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        eprintln!("fxfs-automount: expiring stopped: {error}");
                        return;
                    }
                }
            }
        }
    });
    let mut automounter = Automounter { root: config.root.clone(), volumes, attach };
    loop {
        let (token, done) = match map.next()? {
            Request::Missing { token, name } => (token, automounter.missing(&name)),
            Request::Expire { token, name } => (token, automounter.expire(&name)),
        };
        map.answer(token, done)?;
    }
}

/// The volumes of the guest's Fxfs on the device `tag`, imported through the mounted volume at
/// `filesystem`.
pub struct Fxfs {
    pub tag: String,
    pub filesystem: PathBuf,
}

/// fs/fxfs's `struct fxfs_import`.
#[repr(C)]
struct Import {
    name: u64,
    name_size: u64,
    manifest: u64,
    manifest_size: u64,
}

/// `FXFS_IOC_IMPORT`: `_IOW('x', 0x47, struct fxfs_import)`. Passed `as _`, as autofs's are.
const FXFS_IOC_IMPORT: u32 =
    (1 << 30) | ((std::mem::size_of::<Import>() as u32) << 16) | (0x78 << 8) | 0x47;

fn path(path: &Path) -> io::Result<CString> {
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

impl Volumes for Fxfs {
    fn mount(&self, name: &str, target: &Path) -> io::Result<bool> {
        let (source, fstype) = (CString::new(self.tag.as_str())?, c"fxfs");
        let options = CString::new(format!("volume={name}"))?;
        let target = path(target)?;
        // SAFETY: The strings outlive the call.
        let ret = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                0,
                options.as_ptr().cast(),
            )
        };
        match ret {
            0 => Ok(true),
            _ => match io::Error::last_os_error() {
                error if error.raw_os_error() == Some(libc::ENOENT) => Ok(false),
                error => Err(error),
            },
        }
    }

    fn import(&self, name: &str, manifest: &[u8]) -> io::Result<()> {
        let filesystem = File::open(&self.filesystem)?;
        let request = Import {
            name: name.as_ptr() as u64,
            name_size: name.len() as u64,
            manifest: manifest.as_ptr() as u64,
            manifest_size: manifest.len() as u64,
        };
        // SAFETY: `request` says what `name` and `manifest` hold, which outlive the call.
        match unsafe { libc::ioctl(filesystem.as_raw_fd(), FXFS_IOC_IMPORT as _, &request) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn unmount(&self, target: &Path) -> io::Result<()> {
        let target = path(target)?;
        // SAFETY: The string outlives the call.
        match unsafe { libc::umount2(target.as_ptr(), 0) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// The sandbox manager, asked over vsock, at `port` of `cid`: a connection for each ask, so
/// that a VM restored from a snapshot, whose connections are gone, asks as before; an ask a
/// restore's reset ends is asked again on a new one, as the sandbox manager answers each the
/// same however often it is asked ([`crate::reconnect`], TASK-277).
pub struct Vsock {
    pub cid: u32,
    pub port: u32,
}

impl Vsock {
    fn connect(&self) -> io::Result<File> {
        // SAFETY: Makes a socket, which the file owns from here on.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: As above.
        let socket = unsafe { File::from_raw_fd(fd) };
        // SAFETY: An address of zeros is a valid one to fill in.
        let mut address: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
        address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        address.svm_cid = self.cid;
        address.svm_port = self.port;
        let size = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
        // SAFETY: `address` is a vsock address of `size` bytes, which outlives the call.
        let ret = unsafe { libc::connect(fd, (&raw const address).cast(), size) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }
}

impl Attach for Vsock {
    fn attach(&mut self, name: &str) -> io::Result<Answer> {
        crate::reconnect::exchange(|| self.connect(), |host| upcall::ask(host, name))
    }
}

impl crate::git::Ask for Vsock {
    fn newest(&mut self, url: &str) -> io::Result<upcall::Newest> {
        crate::reconnect::exchange(|| self.connect(), |host| upcall::ask_newest(host, url))
    }
}

/// `git-remote-fxfs REMOTE URL`, as Git runs it, with the repo at `GIT_DIR`: the automounter's root
/// `FXFS_AUTOMOUNT_ROOT` (`/repos`), and the sandbox manager asked at the host's vsock port
/// `FXFS_AUTOMOUNT_PORT` (5054). Returns only if it failed.
pub fn git_main(args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
    let mut args = args.into_iter();
    let (Some(remote), Some(url), None) = (args.next(), args.next(), args.next()) else {
        anyhow::bail!("usage: git-remote-fxfs REMOTE URL, as Git runs it");
    };
    let root = std::env::var_os("FXFS_AUTOMOUNT_ROOT").unwrap_or_else(|| "/repos".into());
    let port = match std::env::var("FXFS_AUTOMOUNT_PORT") {
        Ok(port) => port.parse().context("FXFS_AUTOMOUNT_PORT names a port")?,
        Err(_) => crate::PORT,
    };
    let git_dir = std::env::var_os("GIT_DIR").map(PathBuf::from);
    let mut manager = Vsock { cid: libc::VMADDR_CID_HOST, port };
    crate::git::run(&remote, &url, git_dir.as_deref(), Path::new(&root), &mut manager)
}

/// The binary's arguments: `ROOT FILESYSTEM [--tag TAG] [--timeout SECONDS] [--port PORT]`, and
/// runs it so. The guest agent's tests run it this way too.
pub fn main(args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
    const USAGE: &str =
        "usage: fxfs-automount ROOT FILESYSTEM [--tag TAG] [--timeout SECONDS] [--port PORT]";
    let mut args = args.into_iter();
    let (Some(root), Some(filesystem)) = (args.next(), args.next()) else { anyhow::bail!(USAGE) };
    let (mut tag, mut timeout, mut port) = ("extents".to_owned(), 600, crate::PORT);
    while let Some(flag) = args.next() {
        let value = args.next().context(USAGE)?;
        match flag.as_str() {
            "--tag" => tag = value,
            "--timeout" => timeout = value.parse().context("--timeout takes seconds")?,
            "--port" => port = value.parse().context("--port takes a port")?,
            _ => anyhow::bail!(USAGE),
        }
    }
    let config = Config { root: root.into(), timeout: Duration::from_secs(timeout) };
    let volumes = Fxfs { tag, filesystem: filesystem.into() };
    run(&config, volumes, Vsock { cid: libc::VMADDR_CID_HOST, port })?;
    Ok(())
}
