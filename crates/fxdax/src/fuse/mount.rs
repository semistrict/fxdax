// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A FUSE mount on Linux: the server, serving the kernel's requests through `/dev/fuse`.
//!
//! The mount is made as libfuse makes it: mount(2) of a `fuse` filesystem whose options name an
//! open `/dev/fuse`, which a process may do with `CAP_SYS_ADMIN`; refused that (`EPERM`), through
//! `fusermount3`, setuid root, which mounts it and hands the device back over a socket
//! (`_FUSE_COMMFD`, `SCM_RIGHTS`). Either way the kernel checks permissions itself
//! (`default_permissions`), as for any filesystem.
//!
//! Threads of the mount's own each read a request from the device, have the server handle it, and
//! write its reply, until the filesystem is unmounted, which ends the connection: reads then fail
//! with `ENODEV`.

use super::server::{MAX_WRITE, Server};
use crate::error::Error;
use crate::filesystem::Filesystem;
use crate::storage::Storage;
use anyhow::{Context as _, bail};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::num::NonZeroUsize;
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread::JoinHandle;

/// The setuid program that mounts FUSE filesystems for users without `CAP_SYS_ADMIN`: libfuse 3's.
const FUSERMOUNT: &str = "fusermount3";

/// The room a request is read into: the largest, a write's, its headers, and more. The kernel
/// refuses reads into less than its headers and `max_write`.
const REQUEST_ROOM: usize = MAX_WRITE as usize + 4096;

/// How a filesystem is mounted with FUSE.
#[derive(Clone, Debug)]
pub struct MountOptions {
    allow_other: bool,
    threads: NonZeroUsize,
}

impl MountOptions {
    /// The default options: the mounting user's alone, served by 8 threads.
    pub fn new() -> Self {
        Self { allow_other: false, threads: NonZeroUsize::new(8).unwrap() }
    }

    /// Whether users other than the mounting one may use the filesystem, as its permissions say
    /// (FUSE's `allow_other`). A user other than root needs `user_allow_other` in
    /// `/etc/fuse.conf` for it.
    pub fn allow_other(&mut self, allow_other: bool) -> &mut Self {
        self.allow_other = allow_other;
        self
    }

    /// How many threads serve the kernel's requests, each one at a time.
    pub fn threads(&mut self, threads: NonZeroUsize) -> &mut Self {
        self.threads = threads;
        self
    }

    /// Mounts `fs` at `mountpoint`, a directory, and serves it until it is unmounted. The kernel
    /// sends no request until the mount is made, and holds those made meanwhile until the mount
    /// answers: the filesystem may be used from the moment this returns.
    ///
    /// The filesystem is its store's only writer, as the store's directory is its process's
    /// alone, which the kernel's caches count on: they are kept as long as it likes.
    pub fn mount(&self, fs: Filesystem, mountpoint: impl AsRef<Path>) -> Result<Mount, Error> {
        let mountpoint = mountpoint.as_ref().to_owned();
        let (device, how) = mount(&mountpoint, self.allow_other).map_err(Error::new)?;
        let unmounter = Unmounter { mountpoint, how };
        let (device, server) = (Arc::new(device), Arc::new(Server::new(fs)));
        let mut workers = Vec::new();
        for index in 0..self.threads.get() {
            let (theirs, served) = (device.clone(), server.clone());
            let spawned =
                std::thread::Builder::new().name(format!("fuse {index}")).spawn(move || {
                    serve(&theirs, &served);
                });
            match spawned {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    // The workers started end once it is unmounted.
                    let mount = Mount { unmounter, server: Some(server), workers };
                    drop(mount);
                    return Err(Error::new(anyhow::Error::new(error).context("starting a thread")));
                }
            }
        }
        Ok(Mount { unmounter, server: Some(server), workers })
    }
}

impl Default for MountOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// A filesystem mounted with FUSE, served until it is unmounted: by [`unmount`](Self::unmount),
/// an [`Unmounter`], or anyone (`umount`, `fusermount3 -u`). [`join`](Self::join) then closes it.
///
/// Dropped before it is joined, it unmounts it lazily, waits until the kernel is done with it,
/// and closes it.
pub struct Mount {
    unmounter: Unmounter,
    /// The server, which the workers share until they end: taken by `join`.
    server: Option<Arc<Server>>,
    workers: Vec<JoinHandle<()>>,
}

impl Mount {
    /// Where the filesystem is mounted.
    pub fn mountpoint(&self) -> &Path {
        &self.unmounter.mountpoint
    }

    /// What unmounts the filesystem, from another thread, such as a signal's.
    pub fn unmounter(&self) -> Unmounter {
        self.unmounter.clone()
    }

    /// Unmounts the filesystem (see [`Unmounter::unmount`]).
    pub fn unmount(&self, lazily: bool) -> Result<(), Error> {
        self.unmounter.unmount(lazily)
    }

    /// Waits until the filesystem is unmounted and the kernel is done with it, then closes it,
    /// writing back what is dirty, and gives its storage back, as [`Filesystem::close`] does.
    pub fn join(mut self) -> Result<Storage, Error> {
        self.join_workers();
        let server = self.server.take().expect("joined once");
        let server = Arc::into_inner(server).expect("the workers shared the server alone");
        futures::executor::block_on(server.into_filesystem().close())
    }

    /// Waits for the workers, which end once the filesystem is unmounted.
    fn join_workers(&mut self) {
        for worker in self.workers.drain(..) {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        if let Err(error) = self.unmounter.unmount(true) {
            log::error!("fuse: unmounting {}: {error:#}", self.unmounter.mountpoint.display());
        }
        self.join_workers();
        // The filesystem closes as the server goes.
    }
}

impl std::fmt::Debug for Mount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mount").field("mountpoint", &self.unmounter.mountpoint).finish()
    }
}

/// What unmounts a [`Mount`]'s filesystem.
#[derive(Clone, Debug)]
pub struct Unmounter {
    mountpoint: PathBuf,
    how: How,
}

/// How the filesystem was mounted, so how it is unmounted.
#[derive(Clone, Copy, Debug)]
enum How {
    /// By mount(2): umount2(2) unmounts it.
    Syscall,
    /// By `fusermount3`: `fusermount3 -u` unmounts it.
    Fusermount,
}

impl Unmounter {
    /// Unmounts the filesystem. Unless `lazily`, it fails with `EBUSY` while it is in use, as a
    /// file open in it, and stays mounted; `lazily`, it is detached from its mount point at once,
    /// and unmounted once it is no longer in use (`MNT_DETACH`).
    pub fn unmount(&self, lazily: bool) -> Result<(), Error> {
        match self.how {
            How::Syscall => umount(&self.mountpoint, lazily),
            How::Fusermount => fusermount_unmount(&self.mountpoint, lazily),
        }
        .with_context(|| format!("unmounting {}", self.mountpoint.display()))
        .map_err(Error::new)
    }
}

/// Serves the kernel's requests from `device` until the filesystem is unmounted.
fn serve(device: &File, server: &Server) {
    let mut request = vec![0; REQUEST_ROOM];
    loop {
        let len = match (&*device).read(&mut request) {
            Ok(len) => len,
            Err(error) => match error.raw_os_error() {
                // Unmounted: the kernel is done with the filesystem.
                Some(libc::ENODEV) => return,
                // A request interrupted before it was read, or a signal.
                Some(libc::ENOENT | libc::EINTR | libc::EAGAIN) => continue,
                _ => {
                    log::error!("fuse: reading a request: {error}");
                    return;
                }
            },
        };
        let reply = match futures::executor::block_on(server.handle(&request[..len])) {
            Ok(Some(reply)) => reply,
            Ok(None) => continue,
            Err(error) => {
                log::error!("fuse: handling a request: {error:#}");
                continue;
            }
        };
        // A reply is written whole, in one write, or not at all.
        match (&*device).write(&reply) {
            Ok(written) if written == reply.len() => {}
            Ok(written) => log::error!("fuse: {written} bytes of a {}-byte reply", reply.len()),
            // The request was interrupted, and its reply is not wanted.
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
            Err(error) => log::error!("fuse: writing a reply: {error}"),
        }
    }
}

/// Mounts a FUSE filesystem at `mountpoint`, and returns the device it is served through.
fn mount(mountpoint: &Path, allow_other: bool) -> anyhow::Result<(File, How)> {
    // Opened close-on-exec, as std opens files.
    let device = File::options().read(true).write(true).open("/dev/fuse").context("/dev/fuse")?;
    // SAFETY: Plain calls returning the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut options = format!(
        "fd={},rootmode=40000,user_id={uid},group_id={gid},default_permissions",
        device.as_raw_fd()
    );
    if allow_other {
        options.push_str(",allow_other");
    }
    let target = CString::new(mountpoint.as_os_str().as_bytes()).context("the mount point")?;
    let options = CString::new(options).expect("no NUL in the options");
    // SAFETY: mount(2), with NUL-terminated strings that outlive the call.
    let mounted = unsafe {
        libc::mount(
            c"fxdax".as_ptr(),
            target.as_ptr(),
            c"fuse.fxdax".as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV,
            options.as_ptr().cast(),
        )
    };
    if mounted == 0 {
        return Ok((device, How::Syscall));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::EPERM) {
        return Err(anyhow::Error::new(error).context(format!("mounting {}", mountpoint.display())));
    }
    drop(device);
    let device = fusermount(mountpoint, allow_other)
        .with_context(|| format!("mounting {} through {FUSERMOUNT}", mountpoint.display()))?;
    Ok((device, How::Fusermount))
}

/// Has `fusermount3` mount a FUSE filesystem at `mountpoint`, and returns the device it hands
/// back, which it opened.
fn fusermount(mountpoint: &Path, allow_other: bool) -> anyhow::Result<File> {
    // Both ends close-on-exec, as std makes sockets: the child's is kept open across its exec
    // alone, below.
    let (ours, theirs) = UnixStream::pair().context("a socket for it")?;
    let theirs_fd = theirs.as_raw_fd();
    let mut options = String::from("default_permissions,fsname=fxdax,subtype=fxdax");
    if allow_other {
        options.push_str(",allow_other");
    }
    let mut command = Command::new(FUSERMOUNT);
    command
        .arg("-o")
        .arg(options)
        .arg("--")
        .arg(mountpoint)
        .env("_FUSE_COMMFD", theirs_fd.to_string());
    // SAFETY: fcntl(2) is async-signal-safe, and the descriptor is the socket's, open in the child
    // as in this process.
    unsafe {
        command.pre_exec(move || match libc::fcntl(theirs_fd, libc::F_SETFD, 0) {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    let output = command.output().with_context(|| format!("running {FUSERMOUNT}"))?;
    drop(theirs);
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!("{FUSERMOUNT} failed, {}: {}", output.status, said.trim());
    }
    // It sent the device before it exited.
    receive_fd(&ours).context("the device it opened")
}

/// The file whose descriptor the peer of `socket` sent, as `fusermount3` sends it: one byte, with
/// the descriptor in an `SCM_RIGHTS` message. Received close-on-exec.
fn receive_fd(socket: &UnixStream) -> io::Result<File> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: byte.len() };
    // Room for a control message of a descriptor, aligned as its header needs.
    let mut control = [0u64; 8];
    // SAFETY: A message header, filled below.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control) as _;
    // SAFETY: recvmsg(2) into buffers the header points to, which outlive the call.
    let received =
        unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Walks the control messages recvmsg filled in, within `control`.
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !cmsg.is_null() {
        // SAFETY: A control message header recvmsg wrote.
        let header = unsafe { &*cmsg };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            // SAFETY: An SCM_RIGHTS message's data is the descriptors sent, now this process's.
            let fd = unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>()) };
            // SAFETY: The descriptor is this process's, and nothing else owns it.
            return Ok(unsafe { File::from_raw_fd(fd) });
        }
        // SAFETY: As above.
        cmsg = unsafe { libc::CMSG_NXTHDR(&message, cmsg) };
    }
    Err(io::Error::other("no descriptor was sent"))
}

/// Unmounts what is mounted at `mountpoint`, which mount(2) mounted.
fn umount(mountpoint: &Path, lazily: bool) -> anyhow::Result<()> {
    let target = CString::new(mountpoint.as_os_str().as_bytes()).context("the mount point")?;
    let flags = if lazily { libc::MNT_DETACH } else { 0 };
    // SAFETY: umount2(2), with a NUL-terminated string that outlives the call.
    match unsafe { libc::umount2(target.as_ptr(), flags) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error().into()),
    }
}

/// Has `fusermount3` unmount what it mounted at `mountpoint`.
fn fusermount_unmount(mountpoint: &Path, lazily: bool) -> anyhow::Result<()> {
    let mut command = Command::new(FUSERMOUNT);
    command.arg("-u");
    if lazily {
        command.arg("-z");
    }
    let output = command
        .arg("--")
        .arg(mountpoint)
        .output()
        .with_context(|| format!("running {FUSERMOUNT}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!("{FUSERMOUNT} -u failed, {}: {}", output.status, said.trim());
    }
    Ok(())
}
