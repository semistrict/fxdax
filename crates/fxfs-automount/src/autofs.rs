// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Linux's autofs, protocol 5, as `include/uapi/linux/auto_fs.h` has it: an indirect map mounted
//! on a directory, the packets the kernel writes to its pipe, and the ioctls that answer them.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

/// The protocol's version, the only one taken.
const PROTOCOL: i32 = 5;

/// `_IOC(dir, 0x93, nr, size)`, as asm-generic's ioctl.h encodes it, which x86-64 and arm64 share.
/// Passed `as _`: glibc's `ioctl` takes an `unsigned long`, musl's an `int`.
const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (0x93 << 8) | nr
}

const AUTOFS_IOC_READY: u32 = ioc(0, 0x60, 0);
const AUTOFS_IOC_FAIL: u32 = ioc(0, 0x61, 0);
const AUTOFS_IOC_PROTOVER: u32 = ioc(2, 0x63, 4);
const AUTOFS_IOC_SETTIMEOUT: u32 = ioc(3, 0x64, 8);
const AUTOFS_IOC_EXPIRE_MULTI: u32 = ioc(1, 0x66, 4);

/// `autofs_ptype_missing_indirect` and `autofs_ptype_expire_indirect`.
const MISSING_INDIRECT: i32 = 3;
const EXPIRE_INDIRECT: i32 = 4;

/// `struct autofs_v5_packet`: its wait queue token is an `unsigned int` on 64-bit Linux but
/// alpha's and ia64's.
#[repr(C)]
#[derive(Clone, Copy)]
struct Packet {
    proto_version: i32,
    kind: i32,
    wait_queue_token: u32,
    dev: u32,
    ino: u64,
    uid: u32,
    gid: u32,
    pid: u32,
    tgid: u32,
    len: u32,
    name: [u8; 256],
}

const _: () = assert!(std::mem::size_of::<Packet>() == 304);

/// What the kernel asks of the map.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// A lookup of `name`, which waits until it is mounted, or failed.
    Missing { token: u32, name: String },
    /// `name`'s mount has been idle for the timeout: unmount it, and say so.
    Expire { token: u32, name: String },
}

/// An indirect map mounted on a directory: the kernel's pipe to it, and the directory open, for
/// the ioctls. Once the pipe closes, with this process, the kernel fails lookups rather than wait.
pub struct Map {
    pipe: File,
    root: File,
}

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

impl Map {
    /// Mounts an indirect map on `root`, a directory, served by this process's group, its mounts
    /// expiring once idle for `timeout`.
    pub fn mount(root: &Path, timeout: Duration) -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: Makes a pipe, whose ends `fds` gets.
        check(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })?;
        // SAFETY: The pipe's ends, which these own from here on.
        let (pipe, kernels) = unsafe { (File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        // SAFETY: Reads the process's group.
        let group = unsafe { libc::getpgrp() };
        let options = format!(
            "fd={},pgrp={group},minproto={PROTOCOL},maxproto={PROTOCOL},indirect",
            kernels.as_raw_fd()
        );
        let (source, fstype) = (c"fxfs-automount", c"autofs");
        let target = CString::new(root.as_os_str().as_bytes())?;
        let options = CString::new(options)?;
        // SAFETY: The strings outlive the call.
        check(unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                0,
                options.as_ptr().cast(),
            )
        })?;
        // The kernel keeps its own end of the pipe.
        drop(kernels);
        let map = Self { pipe, root: File::open(root)? };
        let mut version: libc::c_int = 0;
        // SAFETY: Writes the protocol's version into `version`.
        check(unsafe {
            libc::ioctl(map.root.as_raw_fd(), AUTOFS_IOC_PROTOVER as _, &mut version)
        })?;
        if version != PROTOCOL {
            return Err(io::Error::other(format!("autofs protocol {version}, not {PROTOCOL}")));
        }
        let mut seconds: libc::c_ulong = timeout.as_secs().max(1);
        // SAFETY: Reads the timeout from `seconds`, and writes the one before into it.
        check(unsafe {
            libc::ioctl(map.root.as_raw_fd(), AUTOFS_IOC_SETTIMEOUT as _, &mut seconds)
        })?;
        Ok(map)
    }

    /// The kernel's next request: blocks until it comes.
    pub fn next(&mut self) -> io::Result<Request> {
        loop {
            let mut bytes = [0u8; std::mem::size_of::<Packet>()];
            self.pipe.read_exact(&mut bytes)?;
            // SAFETY: A packet is plain data, of any bytes.
            let packet: Packet = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            let len = (packet.len as usize).min(packet.name.len());
            let name = String::from_utf8_lossy(&packet.name[..len]).into_owned();
            let token = packet.wait_queue_token;
            match packet.kind {
                MISSING_INDIRECT => return Ok(Request::Missing { token, name }),
                EXPIRE_INDIRECT => return Ok(Request::Expire { token, name }),
                // Not of an indirect map's: failed, so that nothing waits on it.
                kind => {
                    eprintln!("fxfs-automount: an autofs packet of kind {kind}, failed");
                    self.answer(token, false)?;
                }
            }
        }
    }

    /// Answers the request of `token`: done, or failed.
    pub fn answer(&self, token: u32, done: bool) -> io::Result<()> {
        let request = if done { AUTOFS_IOC_READY } else { AUTOFS_IOC_FAIL };
        // SAFETY: Takes the token by value.
        check(unsafe { libc::ioctl(self.root.as_raw_fd(), request as _, token as libc::c_ulong) })
    }

    /// What asks for expiries, from another thread than the one taking requests.
    pub fn expirer(&self) -> io::Result<Expirer> {
        Ok(Expirer(self.root.try_clone()?))
    }
}

/// Asks the map's kernel for expiries: the map's directory, open.
pub struct Expirer(File);

impl Expirer {
    /// Asks the kernel to expire one mount idle for the timeout, which it asks the map to
    /// unmount (`Request::Expire`) and waits for: whether there was one.
    pub fn expire_one(&self) -> io::Result<bool> {
        let how: libc::c_int = 0;
        // SAFETY: Reads `how`, which outlives the call.
        let ret = unsafe { libc::ioctl(self.0.as_raw_fd(), AUTOFS_IOC_EXPIRE_MULTI as _, &how) };
        match check(ret) {
            Ok(()) => Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::EAGAIN) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ioctls' numbers, as Linux's headers have them on x86-64 and arm64.
    #[test]
    fn ioctl_numbers() {
        assert_eq!(AUTOFS_IOC_READY, 0x9360);
        assert_eq!(AUTOFS_IOC_FAIL, 0x9361);
        assert_eq!(AUTOFS_IOC_PROTOVER, 0x8004_9363);
        assert_eq!(AUTOFS_IOC_SETTIMEOUT, 0xc008_9364);
        assert_eq!(AUTOFS_IOC_EXPIRE_MULTI, 0x4004_9366);
    }
}
