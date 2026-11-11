// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a loop device over an image file, the host's block device a VM's virtio-blk disk is
//! on, as a VM host's local disk images are (TASK-350.1).
//!
//! The device clears itself (LO_FLAGS_AUTOCLEAR): the kernel detaches it once its last opener
//! closes it, this process, which holds it open until it's dropped or exits, or the Firecracker
//! that opened it. Nothing is left attached, whatever the test's outcome, even if it's killed.
//! Attaching one needs root (CAP_SYS_ADMIN).

use anyhow::Context;
use nix::libc;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The loop driver's ioctls and flags (linux/loop.h).
const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4C82;
const LOOP_CONFIGURE: libc::c_ulong = 0x4C0A;
const LO_FLAGS_AUTOCLEAR: u32 = 4;
const LO_FLAGS_DIRECT_IO: u32 = 16;

/// linux/loop.h's `struct loop_info64`.
#[repr(C)]
struct LoopInfo64 {
    lo_device: u64,
    lo_inode: u64,
    lo_rdevice: u64,
    lo_offset: u64,
    lo_sizelimit: u64,
    lo_number: u32,
    lo_encrypt_type: u32,
    lo_encrypt_key_size: u32,
    lo_flags: u32,
    lo_file_name: [u8; 64],
    lo_crypt_name: [u8; 64],
    lo_encrypt_key: [u8; 32],
    lo_init: [u64; 2],
}

/// linux/loop.h's `struct loop_config`, LOOP_CONFIGURE's argument (Linux 5.8).
#[repr(C)]
struct LoopConfig {
    fd: u32,
    block_size: u32,
    info: LoopInfo64,
    reserved: [u64; 8],
}

/// How a loop device reads and writes its image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopIo {
    /// Through the host's page cache, as `losetup` attaches one by default.
    Buffered,
    /// Around it, O_DIRECT (`losetup --direct-io=on`).
    Direct,
}

/// A loop device attached to an image, detached once neither this nor anything else holds it.
pub struct LoopDevice {
    /// The device, held open: until it's closed, the device stays.
    #[expect(dead_code, reason = "never read: held open, it keeps the device attached")]
    device: File,
    path: PathBuf,
}

impl LoopDevice {
    /// Attaches a free loop device to `image`, read and written as `io` says.
    pub fn attach(image: &Path, io: LoopIo) -> anyhow::Result<Self> {
        let backing = OpenOptions::new()
            .read(true)
            .write(true)
            .open(image)
            .with_context(|| format!("opening {}", image.display()))?;
        let control = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/loop-control")
            .context("opening /dev/loop-control: attaching a loop device needs root")?;
        let mut name = [0u8; 64];
        let given = image.as_os_str().as_encoded_bytes();
        let kept = given.len().min(name.len() - 1);
        name[..kept].copy_from_slice(&given[given.len() - kept..]);
        let flags = LO_FLAGS_AUTOCLEAR
            | match io {
                LoopIo::Buffered => 0,
                LoopIo::Direct => LO_FLAGS_DIRECT_IO,
            };
        // Another process may take the free device first: then it's busy, and another is free.
        for _ in 0..16 {
            // SAFETY: an ioctl of no argument on the control device this function opened.
            let number = unsafe { libc::ioctl(control.as_raw_fd(), LOOP_CTL_GET_FREE as _) };
            anyhow::ensure!(
                number >= 0,
                "LOOP_CTL_GET_FREE: {}",
                std::io::Error::last_os_error()
            );
            let path = PathBuf::from(format!("/dev/loop{number}"));
            let device = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            let config = LoopConfig {
                fd: backing.as_raw_fd() as u32,
                block_size: 0,
                info: LoopInfo64 {
                    lo_device: 0,
                    lo_inode: 0,
                    lo_rdevice: 0,
                    lo_offset: 0,
                    lo_sizelimit: 0,
                    lo_number: 0,
                    lo_encrypt_type: 0,
                    lo_encrypt_key_size: 0,
                    lo_flags: flags,
                    lo_file_name: name,
                    lo_crypt_name: [0; 64],
                    lo_encrypt_key: [0; 32],
                    lo_init: [0; 2],
                },
                reserved: [0; 8],
            };
            // SAFETY: LOOP_CONFIGURE reads a `struct loop_config`, which `config` is, alive for
            // the call.
            let configured =
                unsafe { libc::ioctl(device.as_raw_fd(), LOOP_CONFIGURE as _, &config) };
            if configured == 0 {
                let this = Self { device, path };
                // Asked for, but not to be taken for granted: the backing filesystem may not
                // allow O_DIRECT, and the kernel then falls back to buffered.
                let direct = this.direct_io()?;
                anyhow::ensure!(
                    direct == (io == LoopIo::Direct),
                    "{} attached with direct I/O {direct}, asked for {io:?}",
                    this.path.display()
                );
                return Ok(this);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBUSY) {
                return Err(error)
                    .with_context(|| format!("attaching {} to {}", path.display(), image.display()));
            }
        }
        anyhow::bail!("no loop device was free for {} in 16 tries", image.display())
    }

    /// The device's path, /dev/loopN.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the device reads and writes its image around the page cache, as sysfs says.
    pub fn direct_io(&self) -> anyhow::Result<bool> {
        let name = self.path.file_name().context("a loop device has a name")?;
        let dio = Path::new("/sys/block").join(name).join("loop/dio");
        let dio = std::fs::read_to_string(&dio)
            .with_context(|| format!("reading {}", dio.display()))?;
        Ok(dio.trim() == "1")
    }
}

/// The type of the filesystem `path` is on, as `stat -f` names it: zfs, ext4, xfs, ...
pub fn filesystem_type(path: &Path) -> anyhow::Result<String> {
    let output = std::process::Command::new("stat")
        .args(["-f", "-c", "%T"])
        .arg(path)
        .output()
        .context("running stat -f")?;
    anyhow::ensure!(output.status.success(), "stat -f {}: {}", path.display(), output.status);
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structs_match_the_kernels() {
        assert_eq!(std::mem::size_of::<LoopInfo64>(), 232);
        assert_eq!(std::mem::size_of::<LoopConfig>(), 304);
    }
}
