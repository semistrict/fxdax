// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The host's extent daemon killed under load (TASK-50.2, doc-4 "When the host's daemon goes"):
//! two VMs, on stores a daemon of the test's own serves, write files and fault their pages in
//! through the DAX window; each tells the host once it is at it, and the host kills the daemon.
//! Each VM's Firecracker exits, within `BOUND`, with status 158 (`ExtentdLost`), which its host
//! process sees: no vCPU waits for ever on a DAX fault the daemon would have served.

use macros::{guest, host};

pub struct TestFxfsGuestDaemonLost;

/// The vsock port the guest says it is loading on.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5053;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const CAPACITY: u64 = 256 << 20;
    /// What fxdax's Firecracker exits with when its daemon's connection ends (`ExtentdLost`).
    const EXTENTD_LOST: i32 = 158;
    /// How long a VM takes at most to exit once its daemon is gone.
    const BOUND: Duration = Duration::from_secs(10);
    const VMS: [&str; 2] = ["first", "second"];

    /// In the temporary directory, as a socket's path is short (SUN_LEN), and a test's directory,
    /// a round's of a soak, may not be.
    fn socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-lost-{}-{name}", std::process::id()))
    }

    /// A VM, `name`, on its store, which `extentd` serves, with a DAX window, and its vsock at
    /// `uds`.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        name: &str,
        extentd: &Path,
        uds: &Path,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(2, 512)?;
        vm.guest_kernel()?;
        // Both run at once: each boots from an image of its own, which making it remakes.
        vm.root_image(&format!("{name}.rootfs.ext4"));
        let store = test_setup.tmp_dir.join(format!("store-{name}"));
        vm.extent_store_served("extents", &store, CAPACITY, 1 << 28, extentd);
        vm.vsock(uds);
        vm.spawn_api(root, &test_setup.test_case, name, &Vm::api_socket(name))
    }

    impl TestFxfsGuestDaemonLost {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(test_setup)?;
            let extentd = socket("extentd.sock");
            let mut daemon = spawn_daemon(test_setup, &extentd, "extentd", &[], None)?;
            let mut vms = Vec::new();
            for name in VMS {
                let uds = socket(&format!("{name}.vsock"));
                let listener = UnixListener::bind(format!("{}_{PORT}", uds.display()))?;
                vms.push((name, boot(test_setup, &root, name, &extentd, &uds)?, listener));
            }
            for (name, firecracker, listener) in &mut vms {
                let stream = firecracker.accept(listener)?;
                let mut said = String::new();
                BufReader::new(stream).read_line(&mut said)?;
                anyhow::ensure!(said == "loading\n", "{name}'s guest said {said:?}");
            }
            daemon.kill()?;
            let killed = Instant::now();
            daemon.wait()?;
            for (name, mut firecracker, _) in vms {
                while firecracker.running()? {
                    if killed.elapsed() > BOUND {
                        let console = firecracker.kill()?;
                        anyhow::bail!(
                            "{name}'s VM runs still {BOUND:?} after its daemon was killed: see {}",
                            console.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let (status, console) = firecracker.wait()?;
                anyhow::ensure!(
                    status.code() == Some(EXTENTD_LOST),
                    "{name}'s VM exited {status} (signal {:?}), not {EXTENTD_LOST}: see {}",
                    status.signal(),
                    console.display()
                );
            }
            Ok(())
        }
    }

    impl Test for TestFxfsGuestDaemonLost {
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
            println!("OK");
            std::process::exit(0);
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output == "OK\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected \"OK\\n\", got {output:?}"))
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
    use std::os::fd::AsRawFd;

    const MNT: &str = "/mnt/fxfs";
    /// Each file's size: pages the DAX window maps as they fault.
    const SIZE: usize = 4 << 20;
    const PAGE: usize = 4096;

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

    /// Writes the file `round`, through its mapping, a page at a time, each a DAX fault, syncs it,
    /// and removes the round before's.
    fn load(round: usize) {
        let path = format!("{MNT}/load-{round}");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(SIZE as u64).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        for page in (0..SIZE).step_by(PAGE) {
            unsafe { (map.cast::<u8>().add(page)).write_volatile(round as u8) };
        }
        assert_eq!(unsafe { libc::munmap(map, SIZE) }, 0);
        file.sync_all().unwrap();
        if round > 0 {
            fs::remove_file(format!("{MNT}/load-{}", round - 1)).unwrap();
        }
    }

    impl Test for TestFxfsGuestDaemonLost {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            load(0);
            connect().write_all(b"loading\n").unwrap();
            // Until the host's daemon goes, and the VM with it.
            for round in 1.. {
                load(round);
            }
        }
    }
}
