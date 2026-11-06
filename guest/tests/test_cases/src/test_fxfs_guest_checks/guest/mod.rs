// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The guest's half of fxfs-guest-checks: what the checks share, each area's checks in a module of
//! its own, and the running of them, each in a process of its own ([`run`]).

use super::*;
pub(crate) use crate::guest_fxfs::evict;
use crate::guest_fxfs::{cow_base_file, is_dax, read_in_sealed};
use crate::guest_init;
use crate::posix_acl;
use crate::script::{self, Rng};
use crate::script_run::{self, Run};

use nix::libc;
use std::ffi::{CStr, CString};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

mod appends;
mod attributes;
mod copies;
mod cow;
mod failing;
mod files;
mod filesystem;
mod namespace;
mod page_cache;
mod random;
mod run;
mod seals;
mod window;

use appends::*;
use attributes::*;
use copies::*;
use cow::*;
use failing::*;
use files::*;
use filesystem::*;
use namespace::*;
use page_cache::*;
pub(crate) use random::seed;
use random::*;
use seals::*;
use window::*;

const MNT: &str = "/mnt/fxdax";
/// The device's tag, which the image is mounted by, and its mount's counts are named by
/// (/proc/fs/fxfs/<tag>).
const TAG: &str = "fxdax";

pub(crate) fn path(name: &str) -> String {
    format!("{MNT}/{name}")
}

/// Mounts the image with the kernel's Fxfs.
pub(crate) fn mount() {
    try_mount().expect("mount");
}

/// Mounts the image with the kernel's Fxfs, if it can.
fn try_mount() -> std::io::Result<()> {
    mount_with(0)
}

/// Mounts the image with the kernel's Fxfs, with the mount's `flags`, such as `MS_RDONLY`; or, with
/// `MS_REMOUNT`, mounts it again so.
fn mount_with(flags: libc::c_ulong) -> std::io::Result<()> {
    fs::create_dir_all(MNT).unwrap();
    let source = CString::new(TAG).unwrap();
    let target = CString::new(MNT).unwrap();
    let fstype = CString::new("fxfs").unwrap();
    let ret = unsafe {
        libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), flags, std::ptr::null())
    };
    if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// fxdax: `FIFREEZE` and `FITHAW`: `_IOWR('X', 119, int)` and `_IOWR('X', 120, int)`.
const FIFREEZE: libc::c_ulong = 0xc004_5877;
const FITHAW: libc::c_ulong = 0xc004_5878;

/// fxdax: freezes (`FIFREEZE`) or thaws (`FITHAW`), as `request` says, the volume mounted at
/// `target`.
fn freeze_at(target: &str, request: libc::c_ulong) -> std::io::Result<()> {
    let dir = fs::File::open(target).unwrap();
    let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
    if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// fxdax: how many requests of the type `name` the device has taken, as its driver counts them
/// (its `requests` in sysfs): `freeze` those telling it the filesystem froze or thawed.
fn device_requests(name: &str) -> u64 {
    let device = crate::extent_driver::device();
    let path = format!("/sys/bus/virtio/drivers/virtio_extent/{device}/requests");
    let requests = fs::read_to_string(&path).unwrap();
    let count = requests.lines().find_map(|line| line.strip_prefix(&format!("{name} ")));
    count.unwrap_or_else(|| panic!("no {name} in {path}: {requests}")).parse().unwrap()
}

/// fxdax: a fault injection counting down, `name`, has run out.
fn assert_injected(name: &str) {
    let left = fs::read_to_string(format!("{PARAMS}/{name}")).unwrap();
    assert_eq!(left.trim(), "0", "{name} never failed anything");
}

/// Unmounts the image.
fn unmount() {
    let target = CString::new(MNT).unwrap();
    let ret = unsafe { libc::umount(target.as_ptr()) };
    assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
}

/// fxdax: what a coverage kernel counted of fs/fxfs so far, copied to the store for the host:
/// a kernel without gcov has no such directory in debugfs, and leaves nothing (TASK-119.14).
fn save_coverage() {
    const DEBUGFS: &CStr = c"/sys/kernel/debug";
    fn fxfs_counts(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        (fs::read_dir(dir).ok()?.flatten()).find_map(|entry| {
            let path = entry.path();
            if path.ends_with("fs/fxfs") { Some(path) } else { fxfs_counts(&path) }
        })
    }
    let debugfs = c"debugfs".as_ptr();
    // Mounted already, by a check before, or not a kernel with one.
    unsafe { libc::mount(debugfs, DEBUGFS.as_ptr(), debugfs, 0, std::ptr::null()) };
    let Some(counts) = fxfs_counts(std::path::Path::new("/sys/kernel/debug/gcov")) else {
        return;
    };
    fs::create_dir(path(COVERAGE)).unwrap();
    for source in COVERED {
        let name = format!("{source}.gcda");
        fs::write(path(&format!("{COVERAGE}/{name}")), fs::read(counts.join(&name)).unwrap())
            .unwrap();
    }
}

fn fsync_dir(dir: &str) {
    fs::File::open(dir).unwrap().sync_all().unwrap();
}

/// FICLONERANGE: makes `dst`'s `len` bytes at `dst_offset` share `src`'s at `src_offset`.
fn clone_range(dst: &fs::File, src: &fs::File, src_offset: u64, len: u64, dst_offset: u64) {
    /// `_IOW(0x94, 13, struct file_clone_range)`.
    const FICLONERANGE: libc::c_ulong = 0x4020_940d;
    /// `struct file_clone_range`.
    #[repr(C)]
    struct CloneRange {
        src_fd: i64,
        src_offset: u64,
        src_length: u64,
        dest_offset: u64,
    }
    let range = CloneRange {
        src_fd: src.as_raw_fd() as i64,
        src_offset,
        src_length: len,
        dest_offset: dst_offset,
    };
    let cloned = unsafe { libc::ioctl(dst.as_raw_fd(), FICLONERANGE as _, &range) };
    assert_eq!(cloned, 0, "FICLONERANGE: {}", std::io::Error::last_os_error());
}

/// fxdax: has the kernel log what it traced of `path`'s page `page`, if it traces
/// (fxfs.dax_trace, TASK-121): before a check fails on the page.
fn trace_dump(path: &str, page: u64) {
    use std::os::unix::fs::MetadataExt;
    let traced = fs::read_to_string("/sys/module/fxfs/parameters/dax_trace");
    if traced.is_ok_and(|traced| traced.trim() == "Y") {
        let ino = fs::metadata(path).unwrap().ino();
        let _ = fs::write("/sys/module/fxfs/parameters/dax_trace_dump", format!("{ino} {page}"));
    }
}

/// fxdax: what the mount counts ([`crate::guest_fxfs::stats`]), by name.
fn stats() -> std::collections::BTreeMap<String, u64> {
    crate::guest_fxfs::stats(TAG)
}

fn set_xattr(path: &str, name: &str, value: &[u8]) {
    try_set_xattr(path, name, value).unwrap_or_else(|error| panic!("setxattr: {error}"));
}

fn try_set_xattr(path: &str, name: &str, value: &[u8]) -> std::io::Result<()> {
    let (path, name) = (CString::new(path).unwrap(), CString::new(name).unwrap());
    let ret = unsafe {
        libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
    };
    if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

fn get_xattr(path: &str, name: &str) -> Vec<u8> {
    let (path, name) = (CString::new(path).unwrap(), CString::new(name).unwrap());
    let mut value = vec![0u8; 256];
    let len = unsafe {
        libc::getxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
    };
    assert!(len >= 0, "getxattr: {}", std::io::Error::last_os_error());
    value.truncate(len as usize);
    value
}

/// What each thread of the process `pid` is doing, as /proc says.
fn threads_of(pid: libc::pid_t) -> String {
    let read = |path: String| fs::read_to_string(&path).unwrap_or_else(|err| format!("{err}"));
    (fs::read_dir(format!("/proc/{pid}/task")).unwrap().flatten())
        .map(|task| {
            let task = task.path().display().to_string();
            let (wchan, stack) = (read(format!("{task}/wchan")), read(format!("{task}/stack")));
            format!("{task}: {} in {wchan}\n{stack}", read(format!("{task}/stat")))
        })
        .collect()
}

/// Waits for the process `child`, whose status this returns; after `limit`, says what it is
/// doing, kills it, for the checks after to find what it holds let go of, and panics.
fn wait_for_child(child: libc::pid_t, limit: std::time::Duration) -> i32 {
    let deadline = std::time::Instant::now() + limit;
    loop {
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        assert!(waited >= 0, "waitpid: {}", std::io::Error::last_os_error());
        if waited == child {
            return status;
        }
        if std::time::Instant::now() >= deadline {
            let threads = threads_of(child);
            unsafe { libc::kill(child, libc::SIGKILL) };
            panic!("process {child} still runs:\n{threads}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// fxdax: where fxfs's module parameters are.
const PARAMS: &str = "/sys/module/fxfs/parameters";

/// fxdax: one of fxfs's module parameters set for the rest of a check, or a part of it, as it was
/// again once the guard goes. What a check leaves set, as it crashes or hangs, the agent sets back
/// as the guest booted with it, before the next check ([`run`], TASK-160.30).
struct Param {
    path: String,
    was: String,
}

impl Param {
    fn set(name: &str, value: &str) -> Self {
        let path = format!("{PARAMS}/{name}");
        let was = fs::read_to_string(&path).unwrap().trim().to_string();
        fs::write(&path, value).unwrap();
        Self { path, was }
    }
}

impl Drop for Param {
    fn drop(&mut self) {
        // Not a panic, which in a panic's unwinding would abort the check's process before the
        // first panic's message said why it failed.
        if let Err(err) = fs::write(&self.path, &self.was) {
            eprintln!("setting {} back to {}: {err}", self.path, self.was);
        }
    }
}

/// fxdax: pause points (fxfs.dax_pause, TASK-119.9): sets the steps tasks wait at, and waits
/// for a task to wait at `step`.
struct Pauses;

impl Pauses {
    const PAUSED: &str = "/sys/module/fxfs/parameters/dax_paused";
    /// A write(2) with its iomap made, before its copy (FXFS_DAX_STEP_WRITE_MAPPED).
    const WRITE_MAPPED: u64 = 1 << 5;
    /// A rebind about to MAP over a file's private pages, holding its semaphore for writing
    /// (FXFS_DAX_STEP_REBIND, fxfs_dax_rebind).
    const REBIND: u64 = 1 << 7;

    /// Pauses `steps`, as `begin`, those of a file only the file `path`'s tasks.
    fn begin_of(steps: u64, path: &str) -> (Param, Param) {
        use std::os::unix::fs::MetadataExt;
        let only = Param::set("dax_pause_ino", &fs::metadata(path).unwrap().ino().to_string());
        (Self::begin(steps), only)
    }

    /// Pauses `steps` until the guard goes, which lets every task waiting go on.
    fn begin(steps: u64) -> Param {
        // A hung task's stack on the console, at KERN_INFO.
        fs::write("/proc/sys/kernel/printk", "7").unwrap();
        Param::set("dax_pause", &steps.to_string())
    }

    fn wait(step: u64) {
        Self::wait_or(step, None);
    }

    /// Waits for a task to wait at `step`, as `wait`, failing if the process `child` ends
    /// first, with how it ended.
    fn wait_or(step: u64, child: Option<libc::pid_t>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let paused: u64 = fs::read_to_string(Self::PAUSED).unwrap().trim().parse().unwrap();
            if paused & step != 0 {
                return;
            }
            if let Some(child) = child {
                let mut status = 0;
                let ended = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
                assert_ne!(ended, child, "the storer ended ({status:#x}) before step {step:#x}");
            }
            assert!(std::time::Instant::now() < deadline, "no task waits at step {step:#x}");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// fxdax: [`checks`]'s bodies, in [`CHECKS`]'s order, each given its suite. A body's given
/// its suite's fields as it takes them, and most take none.
macro_rules! check_bodies {
    ($($name:ident: $suites:ident, $budget:literal => $body:ident $(($field:ident))?
        $(, $verify:ident)?;)*) => {
        #[allow(unused_variables)]
        const BODIES: &[fn(&TestFxfsGuestChecks)] =
            &[$(|suite: &TestFxfsGuestChecks| $body($(suite.$field)?),)*];
    };
}
checks!(check_bodies);
