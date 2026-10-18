// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Live forks (fxdax's TASK-39.3): a running VM's store forks at a point its guest's Fxfs agrees
//! to, frozen, while the VM goes on. The guest writes a file, freezes, and tells the host over
//! vsock; the host forks the store through the extent daemon, live, and tells the guest, which
//! thaws, writes another, and remounts, finding both. A second VM boots on the fork and finds the
//! filesystem as of the fork: the first file, not the second; and writes its own.

use macros::{guest, host};

pub struct TestFxfsGuestLiveFork;

/// The vsock port the guest says it is frozen on, and the host answers once it forked.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5052;
/// What the guest writes before the fork, and after.
const BEFORE: &[u8] = b"written before the fork\n";
const AFTER: &[u8] = b"written after the fork\n";

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    const CAPACITY: u64 = 256 << 20;

    /// A VM of the guest's `role`, on the store at `store`, run to its end in Firecracker; its
    /// vsock's socket `uds`, if any.
    fn run(
        test_setup: &TestSetup,
        root: &Path,
        role: &str,
        store: &Path,
        uds: Option<&Path>,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(2, 512)?;
        vm.guest_kernel()?;
        vm.extent_store("extents", store, CAPACITY, 1 << 28)?;
        vm.env(&CString::new(format!("FXDAX_LIVE_FORK={role}"))?);
        if let Some(uds) = uds {
            vm.vsock(uds);
        }
        let dir = &test_setup.tmp_dir;
        vm.spawn_api(root, &test_setup.test_case, role, &dir.join(format!("{role}.api")))
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}");
        Ok(())
    }

    impl TestFxfsGuestLiveFork {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            let extentd: PathBuf =
                std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into();
            let (store, fork) = (dir.join("store"), dir.join("fork"));
            // Short, as Unix sockets' paths must be.
            let uds = std::env::temp_dir().join(format!("fxdax-fork-{}.vsock", std::process::id()));
            let listener = UnixListener::bind(format!("{}_{PORT}", uds.display()))?;

            let source = run(test_setup, &root, "source", &store, Some(&uds))?;
            let (stream, _) = listener.accept()?;
            let mut said = String::new();
            let mut answers = stream.try_clone()?;
            BufReader::new(stream).read_line(&mut said)?;
            anyhow::ensure!(said == "frozen\n", "the guest said {said:?}");
            fxdax::extentd::fork(&extentd, &store, &fork)?;
            writeln!(answers, "forked")?;
            // One after the other: they boot from the same root filesystem's image.
            wait(source)?;
            wait(run(test_setup, &root, "fork", &fork, None)?)
        }
    }

    impl Test for TestFxfsGuestLiveFork {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            90
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            if let Err(err) = self.run(&test_setup) {
                eprintln!("{err:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output == "OK source\nOK fork\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected \"OK source\\nOK fork\\n\", got {output:?}"))
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
    use std::io::{BufRead, BufReader, Write};
    use std::os::fd::AsRawFd;

    const MNT: &str = "/mnt/fxfs";
    /// `FIFREEZE` and `FITHAW`: `_IOWR('X', 119, int)` and `_IOWR('X', 120, int)`.
    const FIFREEZE: libc::c_ulong = 0xc004_5877;
    const FITHAW: libc::c_ulong = 0xc004_5878;

    fn mount() {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    fn remount() {
        unmount();
        mount();
    }

    fn freeze(request: libc::c_ulong) {
        let dir = fs::File::open(MNT).unwrap();
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
        assert_eq!(ret, 0, "ioctl {request:#x}: {}", std::io::Error::last_os_error());
    }

    /// A connection to the host on `PORT`.
    fn connect() -> fs::File {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::{FromRawFd as _, IntoRawFd as _};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        // SAFETY: the socket's descriptor, which the file owns from here on.
        unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) }
    }

    impl Test for TestFxfsGuestLiveFork {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            match std::env::var("FXDAX_LIVE_FORK").unwrap().as_str() {
                "source" => {
                    fs::write(format!("{MNT}/before"), BEFORE).unwrap();
                    freeze(FIFREEZE);
                    let mut host = connect();
                    host.write_all(b"frozen\n").unwrap();
                    let mut said = String::new();
                    BufReader::new(host).read_line(&mut said).unwrap();
                    assert_eq!(said, "forked\n");
                    freeze(FITHAW);
                    fs::write(format!("{MNT}/after"), AFTER).unwrap();
                    remount();
                    assert_eq!(fs::read(format!("{MNT}/before")).unwrap(), BEFORE);
                    assert_eq!(fs::read(format!("{MNT}/after")).unwrap(), AFTER);
                    unmount();
                    println!("OK source");
                }
                "fork" => {
                    assert_eq!(fs::read(format!("{MNT}/before")).unwrap(), BEFORE);
                    let after = fs::read(format!("{MNT}/after")).unwrap_err();
                    assert_eq!(after.kind(), std::io::ErrorKind::NotFound);
                    fs::write(format!("{MNT}/fork"), b"the fork's own").unwrap();
                    remount();
                    assert_eq!(fs::read(format!("{MNT}/fork")).unwrap(), b"the fork's own");
                    unmount();
                    println!("OK fork");
                }
                role => panic!("no role {role}"),
            }
        }
    }
}
