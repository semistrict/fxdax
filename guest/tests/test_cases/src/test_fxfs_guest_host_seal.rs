// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Two VMs on one extent daemon share its budget of open pages, which only a guest's seal lets go
//! of (TASK-153.15). The idle one writes through its DAX window until the daemon has room for less
//! than one more write of it, syncing nothing, and stays so. The busy one then writes: its MAPs,
//! refused, would find no room its own seals make, and it failed ENOMEM; the daemon asks the idle
//! one to seal, which it does, as `seals_asked` in its stats says, and the busy one's writes go on.
//! Both read back all they wrote.

use macros::{guest, host};

pub struct TestFxfsGuestHostSeal;

/// The vsock port each guest says where it is on, and the host answers.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5054;
/// The daemon's memory.
#[cfg_attr(feature = "guest", allow(dead_code))]
const MEMORY: u64 = 128 << 20;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    const CAPACITY: u64 = 1 << 30;
    const WINDOW: u64 = 1 << 30;

    /// In the temporary directory, as a socket's path is short (SUN_LEN).
    fn socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-seal-{}-{name}", std::process::id()))
    }

    /// A VM of the guest's `role`, on a store of its own, which `extentd` serves; its vsock's
    /// socket `uds`.
    fn boot(
        test_setup: &TestSetup,
        root: &Path,
        role: &str,
        extentd: &Path,
        uds: &Path,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(2, 1024)?;
        vm.guest_kernel()?;
        // Both run at once: each boots from an image of its own.
        vm.root_image(&format!("{role}.rootfs.ext4"));
        let store = test_setup.tmp_dir.join(format!("store-{role}"));
        vm.extent_store_served("extents", &store, CAPACITY, WINDOW, extentd);
        vm.env(&CString::new(format!("FXDAX_SEAL_ROLE={role}"))?);
        vm.vsock(uds);
        vm.spawn_api(root, &test_setup.test_case, role, &Vm::api_socket(role))
    }

    /// Waits for `firecracker`'s VM to end, and passes on what its guest said.
    fn wait(firecracker: Firecracker) -> anyhow::Result<()> {
        let (status, console) = firecracker.wait()?;
        crate::vm::split_console(std::fs::File::open(&console)?)?;
        anyhow::ensure!(status.success(), "a VM exited {status}: see {}", console.display());
        Ok(())
    }

    /// A guest's connection to the host: what it says, a line at a time, and the host's answers.
    struct Guest {
        said: BufReader<UnixStream>,
        answers: UnixStream,
    }

    impl Guest {
        fn accept(firecracker: &mut Firecracker, listener: &UnixListener) -> anyhow::Result<Self> {
            let stream = firecracker.accept(listener)?;
            Ok(Self { answers: stream.try_clone()?, said: BufReader::new(stream) })
        }

        fn said(&mut self) -> anyhow::Result<String> {
            let mut said = String::new();
            self.said.read_line(&mut said)?;
            Ok(said.trim_end().to_owned())
        }

        fn expect(&mut self, line: &str) -> anyhow::Result<()> {
            let said = self.said()?;
            anyhow::ensure!(said == line, "a guest said {said:?}, not {line:?}");
            Ok(())
        }

        fn tell(&mut self, line: &str) -> anyhow::Result<()> {
            Ok(writeln!(self.answers, "{line}")?)
        }
    }

    impl TestFxfsGuestHostSeal {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(test_setup)?;
            let extentd = socket("extentd.sock");
            spawn_daemon(test_setup, &extentd, "extentd", &[], Some(MEMORY))?;
            let uds = |role: &str| socket(&format!("{role}.vsock"));
            let listen = |role: &str| UnixListener::bind(format!("{}_{PORT}", uds(role).display()));
            let (idles, busies) = (listen("idle")?, listen("busy")?);

            let mut idle = boot(test_setup, &root, "idle", &extentd, &uds("idle"))?;
            let mut i = Guest::accept(&mut idle, &idles)?;
            let holding = i.said()?;
            anyhow::ensure!(holding.starts_with("holding "), "the idle guest said {holding:?}");
            eprintln!("the idle guest is {holding}");
            let mut busy = boot(test_setup, &root, "busy", &extentd, &uds("busy"))?;
            let mut b = Guest::accept(&mut busy, &busies)?;
            b.expect("ready")?;
            b.tell("write")?;
            b.expect("written")?;
            i.tell("check")?;
            wait(idle)?;
            wait(busy)
        }
    }

    impl Test for TestFxfsGuestHostSeal {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            300
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
            if output == "OK idle\nOK busy\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!("expected \"OK idle\\nOK busy\\n\", got {output:?}"))
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
    use std::os::unix::fs::FileExt;

    const MNT: &str = "/mnt/fxfs";
    /// Each write's bytes.
    const WRITE: usize = 1 << 20;
    /// What the busy guest writes: more than the room the idle one leaves, less than the budget.
    const BUSY: usize = 64 << 20;

    fn mount() {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        // SAFETY: Each is a string, as the call asks.
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        // SAFETY: A string, as the call asks.
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// The stat `name` of the filesystem (/proc/fs/fxfs/<tag>).
    fn stat(name: &str) -> u64 {
        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
        let prefix = format!("{name} ");
        (stats.lines())
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("no {name}"))
            .parse()
            .unwrap()
    }

    /// A connection to the host on `PORT`: what it says, and where this guest says it is.
    fn connect() -> (BufReader<fs::File>, fs::File) {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::{FromRawFd as _, IntoRawFd as _};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT)).expect("connecting to the host");
        // SAFETY: the socket's descriptor, which the file owns from here on.
        let file = unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) };
        (BufReader::new(file.try_clone().unwrap()), file)
    }

    fn expect(host: &mut BufReader<fs::File>, line: &str) {
        let mut said = String::new();
        host.read_line(&mut said).unwrap();
        assert_eq!(said, format!("{line}\n"));
    }

    fn fill(buf: &mut [u8], round: usize) {
        for (at, word) in buf.chunks_mut(8).enumerate() {
            word.copy_from_slice(&((round << 32 | at) as u64).to_le_bytes()[..word.len()]);
        }
    }

    fn open(name: &str) -> fs::File {
        (fs::OpenOptions::new().create(true).truncate(true).read(true).write(true))
            .open(format!("{MNT}/{name}"))
            .unwrap()
    }

    /// Writes the write `round` of `file`, its blocks new.
    fn write(file: &fs::File, round: usize) {
        let mut bytes = vec![0; WRITE];
        fill(&mut bytes, round);
        let at = (round * WRITE) as u64;
        file.write_all_at(&bytes, at).unwrap_or_else(|err| panic!("write {round}: {err}"));
    }

    /// Writes `count` writes of `file` from its start: how many went in before the daemon refused
    /// a MAP for want of room, if it did, which a seal of the guest's let in.
    fn write_until_refused(file: &fs::File, count: usize) -> Option<usize> {
        let refused = stat("dax_maps_refused");
        (0..count).find(|round| {
            write(file, *round);
            stat("dax_maps_refused") != refused
        })
    }

    /// Reads back the `count` writes of `file`.
    fn check(file: &fs::File, count: usize) {
        let (mut bytes, mut read) = (vec![0; WRITE], vec![0; WRITE]);
        for round in 0..count {
            fill(&mut bytes, round);
            file.read_exact_at(&mut read, (round * WRITE) as u64).unwrap();
            assert!(read == bytes, "write {round} reads otherwise");
        }
    }

    /// Writes as much as the daemon holds open without refusing a MAP, sealed nothing of: the
    /// writes the first refusal came after, sealed and written anew, fewer each time one is
    /// refused. Then a write more finds less room than it takes.
    fn hold(file: &fs::File) -> usize {
        let mut count = write_until_refused(file, usize::MAX).expect("the daemon refused no MAP");
        loop {
            file.sync_all().unwrap();
            file.set_len(0).unwrap();
            match write_until_refused(file, count) {
                None => return count,
                Some(fewer) => count = fewer,
            }
        }
    }

    fn idle() {
        let (mut host, mut tell) = connect();
        let file = open("held");
        let held = hold(&file);
        writeln!(tell, "holding {held} MiB").unwrap();
        expect(&mut host, "check");
        assert!(stat("seals_asked") > 0, "the host asked the idle guest to seal nothing");
        check(&file, held);
        drop(file);
        unmount();
        println!("OK idle");
    }

    fn busy() {
        let (mut host, mut tell) = connect();
        tell.write_all(b"ready\n").unwrap();
        expect(&mut host, "write");
        let file = open("written");
        (0..BUSY / WRITE).for_each(|round| write(&file, round));
        check(&file, BUSY / WRITE);
        tell.write_all(b"written\n").unwrap();
        drop(file);
        unmount();
        println!("OK busy");
    }

    impl Test for TestFxfsGuestHostSeal {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            match std::env::var("FXDAX_SEAL_ROLE").unwrap().as_str() {
                "idle" => idle(),
                "busy" => busy(),
                role => panic!("no role {role}"),
            }
        }
    }
}
