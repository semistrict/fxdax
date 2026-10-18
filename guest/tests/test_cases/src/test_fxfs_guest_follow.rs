// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Live read-only followers (fxdax's backlog doc-8, TASK-42.6): a VM follows a store another VM
//! writes, both running. The writer writes files and syncs; the follower mounts its store,
//! read-only, and finds them, and keeps one open and another mapped. The writer then writes a new
//! file, renames one, deletes the one the follower keeps open, and overwrites the one it maps, and
//! syncs; the follower sees all of it, without remounting: the new file, the rename, the deletion
//! (the open file still reading as it was, as POSIX has it), and the overwrite, through read(2) and
//! through its mapping. Each guest tells the host over vsock where it is, and the host tells the
//! other.

use macros::{guest, host};

pub struct TestFxfsGuestFollow {
    /// Whether the devices have DAX windows, which the follower maps the writer's files from.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

/// The vsock port each guest says where it is on, and the host answers.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5053;

/// What the files hold.
#[cfg_attr(feature = "host", allow(dead_code))]
const ONE: &[u8] = b"the first file\n";
#[cfg_attr(feature = "host", allow(dead_code))]
const TWO: &[u8] = b"the file the follower keeps open\n";
#[cfg_attr(feature = "host", allow(dead_code))]
const THREE: &[u8] = b"written while the follower follows\n";
/// The mapped file's size, and the bytes of its first page before and after.
#[cfg_attr(feature = "host", allow(dead_code))]
const BIG: usize = 3 << 20;
#[cfg_attr(feature = "host", allow(dead_code))]
const PAGE: usize = 4096;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::CString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    const CAPACITY: u64 = 256 << 20;

    /// A VM of the guest's `role` on the store at `store`, the writer's or, if `read_only`, a
    /// follower's, run in Firecracker; its vsock's socket `uds`.
    fn run(
        test: &TestFxfsGuestFollow,
        test_setup: &TestSetup,
        root: &Path,
        role: &str,
        store: &Path,
        read_only: bool,
        uds: &Path,
    ) -> anyhow::Result<Firecracker> {
        let mut vm = Vm::new(2, 512)?;
        vm.guest_kernel()?;
        // Both run at once: each boots from an image of its own.
        vm.root_image(&format!("{role}.rootfs.ext4"));
        let window = if test.dax { 1 << 28 } else { 0 };
        if read_only {
            vm.extent_store_following("extents", store, window)?;
        } else {
            vm.extent_store("extents", store, CAPACITY, window)?;
        }
        vm.env(&CString::new(format!("FXDAX_FOLLOW={role}"))?);
        vm.vsock(uds);
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

    /// A guest's connection to the host: what it says, a line at a time, and the host's answers.
    struct Guest {
        said: BufReader<UnixStream>,
        answers: UnixStream,
    }

    impl Guest {
        fn accept(listener: &UnixListener) -> anyhow::Result<Self> {
            let (stream, _) = listener.accept()?;
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

    impl TestFxfsGuestFollow {
        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let root = setup_rootfs(test_setup)?;
            let store = dir.join("store");
            // Short, as Unix sockets' paths must be.
            let uds = |role: &str| -> PathBuf {
                std::env::temp_dir()
                    .join(format!("fxdax-follow-{}-{role}.vsock", std::process::id()))
            };
            let listen = |role: &str| UnixListener::bind(format!("{}_{PORT}", uds(role).display()));
            let (writers, followers) = (listen("writer")?, listen("follower")?);

            let writer = run(self, test_setup, &root, "writer", &store, false, &uds("writer"))?;
            let mut w = Guest::accept(&writers)?;
            w.expect("ready")?;
            let follower =
                run(self, test_setup, &root, "follower", &store, true, &uds("follower"))?;
            let mut f = Guest::accept(&followers)?;
            f.expect("following")?;
            w.tell("change")?;
            w.expect("changed")?;
            f.tell("check")?;
            f.expect("seen")?;
            w.tell("done")?;
            wait(writer)?;
            wait(follower)
        }
    }

    impl Test for TestFxfsGuestFollow {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            120
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
            if output == "OK writer\nOK follower\n" {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(format!(
                    "expected \"OK writer\\nOK follower\\n\", got {output:?}"
                ))
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
    use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    const MNT: &str = "/mnt/fxfs";
    /// How long a follower takes at most to see its writer's sync.
    const FOLLOWS_WITHIN: Duration = Duration::from_secs(30);

    fn path(name: &str) -> String {
        format!("{MNT}/{name}")
    }

    fn mount() {
        mount_with(0);
    }

    fn mount_with(flags: libc::c_ulong) {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), flags, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// Makes what the filesystem holds durable: a commit the follower is told of.
    fn sync() {
        let dir = fs::File::open(MNT).unwrap();
        assert_eq!(unsafe { libc::syncfs(dir.as_raw_fd()) }, 0, "syncfs");
    }

    /// `BIG` bytes, each page's of `seed`.
    fn big(seed: u8) -> Vec<u8> {
        (0..BIG).map(|i| seed ^ (i / PAGE) as u8).collect()
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

    fn writer() {
        let (mut host, mut tell) = connect();
        fs::write(path("one"), ONE).unwrap();
        fs::create_dir(path("dir")).unwrap();
        fs::write(path("dir/two"), TWO).unwrap();
        fs::write(path("big"), big(1)).unwrap();
        sync();
        tell.write_all(b"ready\n").unwrap();

        expect(&mut host, "change");
        fs::write(path("three"), THREE).unwrap();
        fs::rename(path("one"), path("renamed")).unwrap();
        fs::remove_file(path("dir/two")).unwrap();
        let mut file = fs::OpenOptions::new().write(true).open(path("big")).unwrap();
        file.write_all(&big(2)[..PAGE]).unwrap();
        drop(file);
        sync();
        tell.write_all(b"changed\n").unwrap();

        expect(&mut host, "done");
        unmount();
        // Mounted read-only (-o ro), the writer's own filesystem reads, and writes nothing.
        mount_with(libc::MS_RDONLY);
        assert_eq!(read("three").unwrap(), THREE);
        let refused = fs::write(path("four"), b"a read-only mount writes nothing").unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::EROFS), "{refused}");
        unmount();
        println!("OK writer");
    }

    /// The bytes of `name`, or the error reading it.
    fn read(name: &str) -> std::io::Result<Vec<u8>> {
        fs::read(path(name))
    }

    /// What the follower sees once it followed the writer's changes: none of them, or all.
    fn followed(mapped: &[u8]) -> bool {
        let gone = |name| read(name).is_err_and(|error| error.kind() == ErrorKind::NotFound);
        read("three").is_ok_and(|three| three == THREE)
            && gone("one")
            && read("renamed").is_ok_and(|renamed| renamed == ONE)
            && gone("dir/two")
            && read("big").is_ok_and(|data| data[..PAGE] == big(2)[..PAGE])
            && mapped[..PAGE] == big(2)[..PAGE]
    }

    fn follower() {
        let (mut host, mut tell) = connect();
        assert_eq!(read("one").unwrap(), ONE);
        assert_eq!(read("dir/two").unwrap(), TWO);
        assert_eq!(read("big").unwrap(), big(1));
        let refused = fs::write(path("four"), b"a follower writes nothing").unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::EROFS), "{refused}");
        let mut held = fs::File::open(path("dir/two")).unwrap();
        let big_file = fs::File::open(path("big")).unwrap();
        // SAFETY: A shared read-only mapping of the file, which the test reads alone.
        let mapped = unsafe {
            let addr = libc::mmap(
                std::ptr::null_mut(),
                BIG,
                libc::PROT_READ,
                libc::MAP_SHARED,
                big_file.as_raw_fd(),
                0,
            );
            assert_ne!(addr, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            std::slice::from_raw_parts(addr as *const u8, BIG)
        };
        assert!(mapped[..PAGE] == big(1)[..PAGE], "the mapping maps other data");
        tell.write_all(b"following\n").unwrap();

        expect(&mut host, "check");
        let deadline = Instant::now() + FOLLOWS_WITHIN;
        while !followed(mapped) {
            assert!(Instant::now() < deadline, "the follower didn't see its writer's changes");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Deleted while open: it reads as it was until closed.
        let mut kept = Vec::new();
        held.seek(SeekFrom::Start(0)).unwrap();
        held.read_to_end(&mut kept).unwrap();
        assert_eq!(kept, TWO);
        tell.write_all(b"seen\n").unwrap();

        // SAFETY: The mapping made above, which nothing uses any more.
        unsafe { libc::munmap(mapped.as_ptr() as *mut _, BIG) };
        drop((held, big_file));
        unmount();
        println!("OK follower");
    }

    impl Test for TestFxfsGuestFollow {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            match std::env::var("FXDAX_FOLLOW").unwrap().as_str() {
                "writer" => writer(),
                "follower" => follower(),
                role => panic!("no role {role}"),
            }
        }
    }
}
