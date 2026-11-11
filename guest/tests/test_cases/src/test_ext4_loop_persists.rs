// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: the loop-device baseline's disk keeps what a guest fsyncs (TASK-350.1).
//
// The host attaches a loop device to a raw image file in the test's directory, and boots a VM on
// it, as a virtio-blk disk whose flushes are host fsyncs (cache_type Writeback). Its guest makes
// ext4, writes a file and fsyncs it and its directory, then resets the VM at once, without
// syncing or unmounting, as a crash does. A second VM, on the same loop device, mounts the
// filesystem and reads the file back whole. Once both are done and the host lets go of the
// device, it is detached: nothing is left attached, as the benchmarks on it rely on.

use macros::{guest, host};

pub struct TestExt4LoopPersists;

/// What the guest is: `write`, the first VM, or `read`, the second.
const ROLE: &str = "FXDAX_LOOP_ROLE";
/// The disk's device in the guest.
const DEVICE: &str = "FXDAX_LOOP_DEVICE";
/// The file's size, and the byte at each offset: a pattern a lost or torn write would break.
#[cfg_attr(feature = "host", allow(dead_code))]
const FILE_SIZE: usize = 4 << 20;
#[cfg_attr(feature = "host", allow(dead_code))]
fn byte_at(offset: usize) -> u8 {
    (offset % 251) as u8
}

#[host]
mod host {
    use super::*;

    use crate::common::setup_rootfs;
    use crate::loop_device::{LoopDevice, LoopIo};
    use crate::vm::{DiskCache, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::CString;
    use std::path::Path;

    const CONTAINERFILE: &str = "\
FROM alpine:3.22
RUN apk add --no-cache e2fsprogs
";

    /// The image's size, sparse.
    const IMAGE_SIZE: u64 = 256 << 20;

    /// Boots a VM in role `role` on the disk at `device`, and waits for it to end, passing on what
    /// its guest said.
    fn boot(test_setup: &TestSetup, root: &Path, role: &str, device: &Path) -> anyhow::Result<()> {
        let mut vm = Vm::new(1, 256)?;
        vm.guest_kernel()?;
        let guest = vm.disk_cached("disk", device, DiskCache::Writeback)?;
        vm.env(&CString::new(format!("{DEVICE}={guest}"))?);
        vm.env(&CString::new(format!("{ROLE}={role}"))?);
        vm.root_image(&format!("{role}.rootfs.ext4"));
        let firecracker =
            vm.spawn_api(root, &test_setup.test_case, role, &Vm::api_socket(role))?;
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "the {role} VM exited {status}");
        Ok(())
    }

    /// Waits for the loop device `name` to be detached from `image`: the kernel clears it as its
    /// last opener closes it, which it may finish after the close returns.
    fn detached(name: &std::ffi::OsStr, image: &Path) -> anyhow::Result<()> {
        let backing = Path::new("/sys/block").join(name).join("loop/backing_file");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let attached = match std::fs::read_to_string(&backing) {
                Ok(file) => Path::new(file.trim()) == image,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                Err(err) => return Err(err.into()),
            };
            if !attached {
                return Ok(());
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "{} is still attached to {} 5 s after its last close",
                name.display(),
                image.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    impl Test for TestExt4LoopPersists {
        fn should_run(&self) -> ShouldRun {
            if std::env::var_os("FXDAX_GUEST_KERNEL").is_none() {
                return ShouldRun::No("FXDAX_GUEST_KERNEL does not name a guest kernel");
            }
            if !crate::vm::has_vmm() {
                return ShouldRun::No("FXDAX_FIRECRACKER does not name fxdax's Firecracker");
            }
            // SAFETY: a plain system call, which cannot fail.
            if unsafe { nix::libc::geteuid() } != 0 {
                return ShouldRun::No("attaching a loop device needs root");
            }
            ShouldRun::Yes
        }

        fn rootfs_image(&self) -> Option<crate::rootfs::Image> {
            Some(crate::rootfs::Image::new(CONTAINERFILE))
        }

        fn timeout_secs(&self) -> u64 {
            60
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(&test_setup)?;
            let image = test_setup.tmp_dir.join("disk.img");
            std::fs::File::create(&image)?.set_len(IMAGE_SIZE)?;
            let image = image.canonicalize()?;
            let device = LoopDevice::attach(&image, LoopIo::Buffered)?;
            boot(&test_setup, &root, "write", device.path())?;
            boot(&test_setup, &root, "read", device.path())?;
            let name = device.path().file_name().expect("a loop device has a name").to_owned();
            drop(device);
            detached(&name, &image)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8_lossy(&stdout);
            // say_out begins its line anew.
            let expected = "\nWRITTEN\nOK\n";
            if output == expected {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"))
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;

    const MNT: &str = "/mnt/disk";
    const FILE: &str = "/mnt/disk/fsynced";

    /// Mounts the ext4 on `device` at `MNT`.
    fn mount(device: &str) {
        let source = CString::new(device).unwrap();
        let target = CString::new(MNT).unwrap();
        // SAFETY: plain C strings, alive for the call.
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), c"ext4".as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    impl Test for TestExt4LoopPersists {
        fn in_guest(self: Box<Self>) {
            let device = std::env::var(DEVICE).expect("the disk's device");
            fs::create_dir_all(MNT).unwrap();
            match std::env::var(ROLE).expect("the VM's role").as_str() {
                "write" => {
                    let status = std::process::Command::new("mkfs.ext4")
                        .args(["-q", "-F", &device])
                        .status()
                        .unwrap();
                    assert!(status.success(), "mkfs.ext4: {status}");
                    mount(&device);
                    let data: Vec<u8> = (0..FILE_SIZE).map(byte_at).collect();
                    let mut file = fs::File::create(FILE).unwrap();
                    file.write_all(&data).unwrap();
                    file.sync_all().unwrap();
                    fs::File::open(MNT).unwrap().sync_all().unwrap();
                    crate::guest_init::say_out("WRITTEN");
                    // Neither synced nor unmounted: only what the fsyncs flushed is the disk's.
                    crate::guest_init::crash();
                }
                "read" => {
                    mount(&device);
                    let data = fs::read(FILE).unwrap();
                    assert_eq!(data.len(), FILE_SIZE, "the file's size");
                    let torn = data.iter().enumerate().position(|(i, &b)| b != byte_at(i));
                    assert_eq!(torn, None, "the file's first byte that isn't what was written");
                    let target = CString::new(MNT).unwrap();
                    // SAFETY: a plain C string, alive for the call.
                    let ret = unsafe { libc::umount(target.as_ptr()) };
                    assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
                    println!("OK");
                }
                role => panic!("no role {role:?}"),
            }
        }
    }
}
