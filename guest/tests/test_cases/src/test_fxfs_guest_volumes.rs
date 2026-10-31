// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Volumes of one filesystem mounted side by side (fxdax's TASK-136.2): the host makes a store with
//! several volumes, as it does a repo's base, and the guest mounts them on directories of the
//! default volume, each with a superblock of its own, on one device. It checks that they have their
//! own `st_dev` and `EXDEV`, and that a file of each, made alike, has the same inode number and
//! its own data; that one unmounting leaves the others working, and the last closes the filesystem;
//! and that the filesystem freezes once all its volumes are, and thaws with the first.
//!
//! fxfs-guest-volumes-dax: the same with a DAX window, which all the volumes share.

use macros::{guest, host};

pub struct TestFxfsGuestVolumes {
    /// Whether the device has a DAX window.
    pub dax: bool,
}

/// The file the guest writes in each of the volumes it writes.
const FILE: &str = "file";

/// What `volume`'s file holds: several of the DAX window's chunks, no block alike.
fn contents(volume: &str) -> Vec<u8> {
    let seed = volume.as_bytes()[0];
    (0..5 << 20).map(|i: usize| (i % 251) as u8 ^ (i / 4096) as u8 ^ seed).collect()
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::{Image, Storage};
    use std::path::Path;

    const CAPACITY: u64 = 256 << 20;
    const WINDOW: u64 = 64 << 20;
    const VOLUMES: [&str; 4] = ["default", "a", "b", "c"];

    /// Makes the store at `path`, with `VOLUMES`, empty.
    fn format(path: &Path) -> anyhow::Result<()> {
        block_on(async {
            let image = Image::format(Storage::create(path, CAPACITY)?).await?;
            for volume in VOLUMES {
                image.create_volume(volume).await?.unmount().await?;
            }
            image.close().await?;
            Ok(())
        })
    }

    /// What `volume`'s file holds in the store at `path`.
    fn read(path: &Path, volume: &str) -> anyhow::Result<Vec<u8>> {
        block_on(async {
            let image = Image::mount(Storage::open(path)?).await?;
            let fs = image.open_volume(volume).await?;
            let (file, attr) = fs.root().lookup(FILE.as_bytes()).await?;
            let mut data = vec![0; attr.size as usize];
            let mut read = 0;
            while read < data.len() {
                read += file.read(read as u64, &mut data[read..]).await?;
            }
            drop(file);
            fs.unmount().await?;
            image.close().await?;
            Ok(data)
        })
    }

    impl Test for TestFxfsGuestVolumes {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let path = test_setup.tmp_dir.join("extents");
            format(&path)?;
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", &path, CAPACITY, if self.dax { WINDOW } else { 0 })?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
            }
            let path = test_setup.tmp_dir.join("extents");
            for volume in ["a", "b"] {
                match read(&path, volume) {
                    Ok(data) if data == contents(volume) => {}
                    Ok(_) => return TestOutcome::Fail(format!("{volume}'s file differs")),
                    Err(error) => return TestOutcome::Fail(format!("{volume}: {error:#}")),
                }
            }
            TestOutcome::Pass
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::io::{Error, ErrorKind};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    /// The default volume, on whose directories the others mount.
    const MNT: &str = "/mnt/fxfs";
    const A: &str = "/mnt/fxfs/a";
    const B: &str = "/mnt/fxfs/b";
    const C: &str = "/mnt/fxfs/c";
    /// `FIFREEZE` and `FITHAW`: `_IOWR('X', 119, int)` and `_IOWR('X', 120, int)`.
    const FIFREEZE: libc::c_ulong = 0xc004_5877;
    const FITHAW: libc::c_ulong = 0xc004_5878;

    /// Mounts `volume`, the default if none, at `target`.
    fn try_mount(volume: Option<&str>, target: &str) -> std::io::Result<()> {
        let source = CString::new("extents").unwrap();
        let (target, fstype) = (CString::new(target).unwrap(), CString::new("fxfs").unwrap());
        let options = volume.map(|volume| CString::new(format!("volume={volume}")).unwrap());
        let data = options.as_ref().map_or(std::ptr::null(), |options| options.as_ptr());
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, data.cast())
        };
        if ret == 0 { Ok(()) } else { Err(Error::last_os_error()) }
    }

    fn mount(volume: Option<&str>, target: &str) {
        try_mount(volume, target).unwrap_or_else(|error| panic!("mount {target}: {error}"));
    }

    fn unmount(target: &str) {
        let target = CString::new(target).unwrap();
        let ret = unsafe { libc::umount(target.as_ptr()) };
        assert_eq!(ret, 0, "umount {target:?}: {}", Error::last_os_error());
    }

    fn ioctl(target: &str, request: libc::c_ulong) {
        let dir = File::open(target).unwrap();
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
        assert_eq!(ret, 0, "{request:#x} on {target}: {}", Error::last_os_error());
    }

    fn dev(path: &str) -> u64 {
        fs::metadata(path).unwrap().dev()
    }

    /// The path of the file of the volume mounted at `mount`.
    fn file(mount: &str) -> String {
        format!("{mount}/{FILE}")
    }

    fn check_file(mount: &str, volume: &str) {
        assert!(fs::read(file(mount)).unwrap() == contents(volume), "{volume}'s file differs");
    }

    /// Mounting `C` fails, the filesystem being frozen.
    fn check_frozen() {
        let error = try_mount(Some("c"), C).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBUSY), "mounting while frozen: {error}");
    }

    /// Mounting `C` works, the filesystem not being frozen.
    fn check_not_frozen() {
        mount(Some("c"), C);
        unmount(C);
    }

    fn check_side_by_side(dax: bool) {
        assert_ne!(dev(MNT), dev(A));
        assert_ne!(dev(A), dev(B));

        // Made alike, their files have the same number, which is no more than each volume's own.
        fs::write(file(A), contents("a")).unwrap();
        fs::write(file(B), contents("b")).unwrap();
        let ino = fs::metadata(file(A)).unwrap().ino();
        assert_eq!(ino, fs::metadata(file(B)).unwrap().ino());
        check_file(A, "a");
        check_file(B, "b");
        fs::write("/proc/sys/vm/drop_caches", b"3").unwrap();
        check_file(A, "a");
        check_file(B, "b");

        let across = fs::rename(file(A), format!("{B}/moved")).unwrap_err();
        assert_eq!(across.raw_os_error(), Some(libc::EXDEV), "rename: {across}");
        let across = fs::hard_link(file(A), format!("{B}/linked")).unwrap_err();
        assert_eq!(across.raw_os_error(), Some(libc::EXDEV), "link: {across}");

        let mounts = fs::read_to_string("/proc/mounts").unwrap();
        assert!(mounts.contains("volume=a"), "{mounts}");
        assert!(mounts.contains("volume=b"), "{mounts}");
        assert_eq!(mounts.contains(",dax"), dax, "{mounts}");

        // A volume mounted again shares its superblock.
        fs::create_dir_all("/mnt/again").unwrap();
        mount(Some("a"), "/mnt/again");
        assert_eq!(dev("/mnt/again"), dev(A));
        unmount("/mnt/again");

        let missing = try_mount(Some("none"), C).unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::NotFound, "mounting no volume: {missing}");
    }

    fn check_unmount_leaves_the_others() {
        let ino = fs::metadata(file(A)).unwrap().ino();
        unmount(A);
        check_file(B, "b");
        fs::write(format!("{B}/after"), b"written with a unmounted").unwrap();
        fs::write(format!("{MNT}/after"), b"written with a unmounted").unwrap();
        // What the default volume has there.
        assert_eq!(fs::read_dir(A).unwrap().count(), 0);
        mount(Some("a"), A);
        check_file(A, "a");
        assert_eq!(fs::metadata(file(A)).unwrap().ino(), ino);
        assert_eq!(fs::read(format!("{B}/after")).unwrap(), b"written with a unmounted");
    }

    fn check_freezing() {
        // One frozen, the filesystem goes on.
        ioctl(A, FIFREEZE);
        fs::write(format!("{B}/with-a-frozen"), b"written").unwrap();
        check_not_frozen();
        ioctl(B, FIFREEZE);
        check_not_frozen();
        // All frozen, it is.
        ioctl(MNT, FIFREEZE);
        check_frozen();
        check_file(A, "a");
        // It thaws with the first.
        ioctl(A, FITHAW);
        check_not_frozen();
        ioctl(B, FITHAW);
        ioctl(MNT, FITHAW);

        // The last not frozen unmounting leaves it frozen.
        ioctl(A, FIFREEZE);
        ioctl(MNT, FIFREEZE);
        unmount(B);
        check_frozen();
        ioctl(MNT, FITHAW);
        ioctl(A, FITHAW);
        mount(Some("b"), B);
        fs::write(format!("{A}/thawed"), b"written").unwrap();
        fs::write(format!("{B}/thawed"), b"written").unwrap();
        check_file(B, "b");
    }

    impl Test for TestFxfsGuestVolumes {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount(None, MNT);
            for dir in [A, B, C] {
                fs::create_dir(dir).unwrap();
            }
            mount(Some("a"), A);
            mount(Some("b"), B);
            check_side_by_side(self.dax);
            check_unmount_leaves_the_others();
            check_freezing();

            // The last unmount closes the filesystem, which opens again on one volume.
            unmount(A);
            unmount(B);
            unmount(MNT);
            fs::create_dir_all(A).unwrap();
            mount(Some("a"), A);
            check_file(A, "a");
            unmount(A);
            println!("OK");
        }
    }
}
