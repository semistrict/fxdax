// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A follower that can follow no more recovers by remounting (doc-8; TASK-79.23, 79.24, 79.42).
//!
//! A batch that fails, as the kernel's `fxfs.follow_fail_batch` has it, ends the follower: what
//! reads the trees fails with ESTALE. It unmounts and mounts again, and sees all its writer
//! committed. Then its writer dies with writes uncommitted, which cuts the follower off: an extent
//! it never read reads ESTALE, and no new writer opens the store while the follower's connection
//! does. It unmounts, which closes the connection: a new writer opens the store, as of its last
//! commit, and writes on; and the follower mounts again, on a connection of its own, and follows
//! that writer. Last, the host kills the daemon: the follower's VM, on that new connection, exits
//! with status 158 (`ExtentdLost`), as the VM of one that never reopened does.

use macros::{guest, host};

pub struct TestFxfsGuestFollowRemount {
    /// Whether the devices have DAX windows.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

/// The vsock port each guest says where it is on, and the host answers.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5053;

#[cfg_attr(feature = "host", allow(dead_code))]
const ONE: &[u8] = b"the first file\n";
#[cfg_attr(feature = "host", allow(dead_code))]
const TWO: &[u8] = b"committed as the follower's batch fails\n";
#[cfg_attr(feature = "host", allow(dead_code))]
const THREE: &[u8] = b"never committed\n";
#[cfg_attr(feature = "host", allow(dead_code))]
const FOUR: &[u8] = b"written by the next writer\n";
/// How many files the follower never reads until it looks for the cut: each read caches its file.
#[cfg_attr(feature = "host", allow(dead_code))]
const COLD: usize = 64;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const CAPACITY: u64 = 256 << 20;
    /// What fxdax's Firecracker exits with when its daemon's connection ends (`ExtentdLost`).
    const EXTENTD_LOST: i32 = 158;
    /// How long a VM takes at most to exit once its daemon is gone.
    const BOUND: Duration = Duration::from_secs(10);
    /// How long the daemon takes at most to let go of a store its last connection closed.
    const LETS_GO_WITHIN: Duration = Duration::from_secs(30);

    /// In the temporary directory, as a socket's path is short (SUN_LEN).
    fn socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-remount-{}-{name}", std::process::id()))
    }

    /// A VM's place: what the test needs of it wherever it boots one.
    struct Place<'a> {
        test: &'a TestFxfsGuestFollowRemount,
        setup: &'a TestSetup,
        root: &'a Path,
        store: &'a Path,
        extentd: &'a Path,
    }

    impl Place<'_> {
        /// A VM named `name`, of the guest's `role`, and what it connects to.
        fn boot(
            &self,
            name: &str,
            role: &str,
            read_only: bool,
        ) -> anyhow::Result<(Firecracker, UnixListener)> {
            let uds = socket(&format!("{name}.vsock"));
            let listener = UnixListener::bind(format!("{}_{PORT}", uds.display()))?;
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.root_image(&format!("{name}.rootfs.ext4"));
            let window = if self.test.dax { 1 << 28 } else { 0 };
            if read_only {
                vm.extent_store_following_served("extents", self.store, window, self.extentd);
            } else {
                vm.extent_store_served("extents", self.store, CAPACITY, window, self.extentd);
            }
            vm.env(&CString::new(format!("FXDAX_FOLLOW={role}"))?);
            vm.vsock(&uds);
            let api = Vm::api_socket(name);
            Ok((vm.spawn_api(self.root, &self.setup.test_case, name, &api)?, listener))
        }
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

        fn expect(&mut self, line: &str) -> anyhow::Result<()> {
            let mut said = String::new();
            self.said.read_line(&mut said)?;
            anyhow::ensure!(said == format!("{line}\n"), "a guest said {said:?}, not {line:?}");
            Ok(())
        }

        fn tell(&mut self, line: &str) -> anyhow::Result<()> {
            Ok(writeln!(self.answers, "{line}")?)
        }
    }

    /// Waits for `firecracker`'s VM, `name`, to exit `EXTENTD_LOST`, its daemon `killed`.
    fn lost(mut firecracker: Firecracker, name: &str, killed: Instant) -> anyhow::Result<()> {
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
        Ok(())
    }

    impl TestFxfsGuestFollowRemount {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let root = setup_rootfs(test_setup)?;
            let store = test_setup.tmp_dir.join("store");
            let extentd = socket("extentd.sock");
            let mut daemon = spawn_daemon(test_setup, &extentd, "extentd", &[], None)?;
            let place = Place {
                test: self,
                setup: test_setup,
                root: &root,
                store: &store,
                extentd: &extentd,
            };

            let (mut writer, listener) = place.boot("writer", "writer", false)?;
            let mut w = Guest::accept(&mut writer, &listener)?;
            w.expect("ready")?;
            let (mut follower, listener) = place.boot("follower", "follower", true)?;
            let mut f = Guest::accept(&mut follower, &listener)?;
            f.expect("following")?;

            // A batch that fails.
            f.tell("fail batches")?;
            f.expect("failing")?;
            w.tell("change")?;
            w.expect("changed")?;
            f.tell("changed")?;
            f.expect("remounted")?;

            // A writer that dies with writes uncommitted, cutting the follower off.
            w.tell("leave writes uncommitted")?;
            w.expect("uncommitted")?;
            writer.kill()?;
            f.tell("cut off")?;
            f.expect("cut off")?;

            // No writer opens the store while the follower's connection does.
            let (mut refused, listener) = place.boot("refused", "next-writer", false)?;
            anyhow::ensure!(
                Guest::accept(&mut refused, &listener).is_err(),
                "a writer opened a store its follower's connection kept cut off"
            );
            refused.wait()?;

            // Then it does, once the follower's unmount closed that connection.
            f.tell("unmount")?;
            f.expect("unmounted")?;
            let tried = Instant::now();
            let (next, mut w) = loop {
                let name = format!("next-{}", tried.elapsed().as_millis());
                let (mut next, listener) = place.boot(&name, "next-writer", false)?;
                match Guest::accept(&mut next, &listener) {
                    Ok(w) => break (next, w),
                    Err(error) if tried.elapsed() > LETS_GO_WITHIN => return Err(error),
                    Err(_) => {
                        next.wait()?;
                    }
                }
            };
            w.expect("written")?;
            f.tell("mount")?;
            f.expect("remounted")?;

            daemon.kill()?;
            let killed = Instant::now();
            daemon.wait()?;
            lost(next, "the next writer", killed)?;
            lost(follower, "the follower", killed)
        }
    }

    impl Test for TestFxfsGuestFollowRemount {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            180
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
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";
    const FAIL_BATCH: &str = "/sys/module/fxfs/parameters/follow_fail_batch";
    /// How long a follower takes at most to reach its writer's sync, or a cut.
    const WITHIN: Duration = Duration::from_secs(30);

    fn path(name: &str) -> String {
        format!("{MNT}/{name}")
    }

    fn cold(index: usize) -> String {
        path(&format!("cold-{index}"))
    }

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
        let ret = unsafe { libc::umount(target.as_ptr()) };
        assert_eq!(ret, 0, "umount: {}", std::io::Error::last_os_error());
    }

    fn sync() {
        let dir = fs::File::open(MNT).unwrap();
        assert_eq!(unsafe { libc::syncfs(dir.as_raw_fd()) }, 0, "syncfs");
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

    /// Waits for the host, which ends the VM.
    fn wait_to_end(host: &mut BufReader<fs::File>) {
        let mut said = String::new();
        host.read_line(&mut said).unwrap();
        panic!("the host said {said:?}, not ending the VM");
    }

    fn writer() {
        let (mut host, mut tell) = connect();
        fs::write(path("one"), ONE).unwrap();
        for index in 0..COLD {
            fs::write(cold(index), vec![index as u8; 8192]).unwrap();
        }
        sync();
        writeln!(tell, "ready").unwrap();

        expect(&mut host, "change");
        fs::write(path("two"), TWO).unwrap();
        sync();
        writeln!(tell, "changed").unwrap();

        expect(&mut host, "leave writes uncommitted");
        let file = fs::File::create(path("three")).unwrap();
        (&file).write_all(THREE).unwrap();
        // SAFETY: On the file, whose data it writes back, committing nothing.
        let flags = libc::SYNC_FILE_RANGE_WRITE | libc::SYNC_FILE_RANGE_WAIT_AFTER;
        let ret = unsafe { libc::sync_file_range(file.as_raw_fd(), 0, 0, flags) };
        assert_eq!(ret, 0, "sync_file_range: {}", std::io::Error::last_os_error());
        writeln!(tell, "uncommitted").unwrap();
        wait_to_end(&mut host);
    }

    fn next_writer() {
        let (mut host, mut tell) = connect();
        assert_eq!(fs::read(path("one")).unwrap(), ONE);
        assert_eq!(fs::read(path("two")).unwrap(), TWO);
        let gone = fs::read(path("three")).unwrap_err();
        assert_eq!(gone.kind(), ErrorKind::NotFound, "what no commit held: {gone}");
        fs::write(path("four"), FOUR).unwrap();
        sync();
        writeln!(tell, "written").unwrap();
        wait_to_end(&mut host);
    }

    /// Waits for a read of what `file` names to fail ESTALE: until then, it reads, or doesn't
    /// exist yet.
    fn stale(file: impl Fn(usize) -> String) {
        let start = Instant::now();
        for attempt in 0.. {
            assert!(start.elapsed() < WITHIN, "no ESTALE after {attempt} reads");
            match fs::read(file(attempt)) {
                Ok(_) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESTALE) => return,
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => panic!("the read failed with {error}, not ESTALE"),
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn follower() {
        let (mut host, mut tell) = connect();
        assert_eq!(fs::read(path("one")).unwrap(), ONE);
        writeln!(tell, "following").unwrap();

        // A batch that fails. Its readers read no more, until it is remounted.
        expect(&mut host, "fail batches");
        fs::write(FAIL_BATCH, "1").unwrap();
        writeln!(tell, "failing").unwrap();
        expect(&mut host, "changed");
        stale(|_| path("two"));
        let listed = fs::read_dir(MNT).and_then(|mut entries| entries.next().transpose());
        let listed = listed.map(|_| ()).unwrap_err();
        assert_eq!(listed.raw_os_error(), Some(libc::ESTALE), "{listed}");
        unmount();
        fs::write(FAIL_BATCH, "0").unwrap();
        mount();
        assert_eq!(fs::read(path("two")).unwrap(), TWO);
        writeln!(tell, "remounted").unwrap();

        // Cut off: what it has to read from its store, it can't.
        expect(&mut host, "cut off");
        stale(|attempt| cold(attempt % COLD));
        writeln!(tell, "cut off").unwrap();

        expect(&mut host, "unmount");
        unmount();
        writeln!(tell, "unmounted").unwrap();

        expect(&mut host, "mount");
        mount();
        let start = Instant::now();
        while !fs::exists(path("four")).unwrap() {
            assert!(start.elapsed() < WITHIN, "no next writer's file");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read(path("four")).unwrap(), FOUR);
        assert_eq!(fs::read(cold(0)).unwrap(), vec![0u8; 8192]);

        // Unmounting closed the device: unbinding it has no queues to let go of (TASK-79.48).
        unmount();
        let device = crate::extent_driver::device();
        crate::extent_driver::unbind(&device);
        crate::extent_driver::bind(&device);
        mount();
        assert_eq!(fs::read(path("four")).unwrap(), FOUR);
        writeln!(tell, "remounted").unwrap();
        wait_to_end(&mut host);
    }

    impl Test for TestFxfsGuestFollowRemount {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            match std::env::var("FXDAX_FOLLOW").unwrap().as_str() {
                "writer" => writer(),
                "next-writer" => next_writer(),
                "follower" => follower(),
                role => panic!("no role {role}"),
            }
        }
    }
}
