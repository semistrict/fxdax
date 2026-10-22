// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Snapshots of a VM with an extent-store device (fxdax's TASK-39.1): the guest's Fxfs, written
//! to and frozen, is snapshotted, the VM goes, and the snapshot is restored (`Mode`):
//!
//! - **Zygote:** twice, as a zygote's children are, each into a store of its own, forked from the
//!   snapshot's point; the host then checks each store holds what its guest wrote, and not the
//!   other's.
//! - **Suspend:** once, into the store the VM had, which nothing changed meanwhile.
//! - **Migrate:** once, as on another host: its extent daemon keeps stores in a bucket, and the
//!   restore names a point and a store it doesn't have here, so it takes the point's branch from
//!   the bucket, and forks the store from it. The bucket is FXDAX_EXTENTD_REMOTE's, if set, or one
//!   in the daemon's memory. With FXDAX_MIGRATE_PHASE, it runs on two hosts (TASK-28.11,
//!   scripts/migrate-gce.sh): `source` snapshots the VM, its store's branch in the bucket, and
//!   leaves the snapshot, its memory and the VM's root filesystem in FXDAX_MIGRATE_BUNDLE; then
//!   `target`, on another host, which has nothing of the store, restores it from there, at the
//!   same paths, the root filesystem's too, which the restored VM goes on reading.
//!
//! Each restored guest thaws, reads what was written before the snapshot (with a DAX window,
//! through a mapping made before it, which the restored window maps again), writes its own file,
//! which mints extents of its store's branch, and remounts.
//!
//! The VMs run in Firecracker driven through its API. Their root filesystem's disk is read-only,
//! as the restored VMs share it. The guest tells the host it is frozen over vsock, and each
//! restored guest, whose connection the restore resets, asks the host its name on a new one.

use macros::{guest, host};

pub struct TestFxfsGuestSnapshot {
    /// Whether the device has a DAX window.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub mode: Mode,
}

/// How the snapshot is restored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Zygote,
    Suspend,
    Migrate,
}

impl Mode {
    /// The restored VMs' names, which each writes to its store.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    fn restores(self) -> &'static [&'static str] {
        match self {
            Self::Zygote => &["a", "b"],
            Self::Suspend => &["resumed"],
            Self::Migrate => &["moved"],
        }
    }
}

/// Which part of a migration between hosts runs here (FXDAX_MIGRATE_PHASE), if one does.
#[cfg_attr(feature = "guest", allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The VM, snapshotted, left in the bundle.
    Source,
    /// The snapshot in the bundle, restored.
    Target,
}

#[cfg_attr(feature = "guest", allow(dead_code))]
impl Phase {
    fn of_env() -> Option<Self> {
        match std::env::var("FXDAX_MIGRATE_PHASE").ok()?.as_str() {
            "source" => Some(Self::Source),
            "target" => Some(Self::Target),
            other => panic!("FXDAX_MIGRATE_PHASE is source or target, not {other:?}"),
        }
    }
}

/// The vsock port the guest says it is frozen on, and asks its name on once restored.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5051;
/// What the guest writes before the snapshot.
const BEFORE: &[u8] = b"written before the snapshot\n";
/// The file the guest maps, with a DAX window, before the snapshot, and reads after: a pattern.
#[cfg_attr(feature = "host", allow(dead_code))]
const MAPPED_LEN: usize = 64 << 10;

/// The pattern the mapped file holds.
#[cfg_attr(feature = "host", allow(dead_code))]
fn mapped_pattern() -> Vec<u8> {
    (0..MAPPED_LEN).map(|i| (i % 251) as u8).collect()
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::{Firecracker, Vm};
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::Context;
    use futures::executor::block_on;
    use fxdax::{Filesystem, Storage};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    const CAPACITY: u64 = 512 << 20;

    /// A short path for a vsock's socket, which Unix sockets' 107 bytes hold: the test's
    /// directory may be deep.
    fn uds(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fxdax-snap-{}-{name}.vsock", std::process::id()))
    }

    /// Reads a line from the guest on the vsock port the VM at `uds` forwards to the host.
    fn listen(uds: &Path) -> anyhow::Result<UnixListener> {
        let path = format!("{}_{PORT}", uds.display());
        let _ = std::fs::remove_file(&path);
        UnixListener::bind(&path).with_context(|| format!("binding {path}"))
    }

    /// The store the test's restore `name` opens.
    fn store(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("store-{name}"))
    }

    fn read_file(fs: &Filesystem, name: &str) -> Vec<u8> {
        let (file, attr) = block_on(fs.root().lookup(name.as_bytes()))
            .unwrap_or_else(|err| panic!("host: lookup {name}: {err}"));
        let mut buf = vec![0; attr.size as usize];
        let count = block_on(file.read(0, &mut buf)).unwrap();
        buf.truncate(count);
        buf
    }

    /// The files a migration between hosts moves: the snapshot, its memory, and the VM's root
    /// filesystem, which the restored VM goes on reading.
    const BUNDLED: [&str; 3] = ["snapshot", "memory", "rootfs.ext4"];

    /// The bundle a migration between hosts leaves the VM in, and restores it from.
    fn bundle() -> anyhow::Result<PathBuf> {
        Ok(std::env::var_os("FXDAX_MIGRATE_BUNDLE").context("FXDAX_MIGRATE_BUNDLE not set")?.into())
    }

    impl TestFxfsGuestSnapshot {
        /// The VMs restored here, as `phase` says.
        fn restores(&self, phase: Option<Phase>) -> &'static [&'static str] {
            match phase {
                Some(Phase::Source) => &[],
                _ => self.mode.restores(),
            }
        }

        fn run(&self, test_setup: &TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let phase = Phase::of_env().filter(|_| self.mode == Mode::Migrate);
            let root = setup_rootfs(test_setup)?;
            std::fs::create_dir_all(root.join("mnt/fxfs"))?;
            // Migrating, a daemon of the test's own, which keeps stores in a bucket, as the hosts
            // a VM moves between do: the environment's, or one in its memory.
            let extentd = match self.mode {
                Mode::Migrate => {
                    // In the temporary directory, as a socket's path is short (SUN_LEN), and a
                    // test's directory, a round's of a soak, may not be.
                    let socket = std::env::temp_dir()
                        .join(format!("fxdax-{}-extentd.sock", std::process::id()));
                    let named = std::env::var_os("FXDAX_EXTENTD_REMOTE")
                        .is_some_and(|remote| !remote.is_empty());
                    let bucket = std::ffi::OsStr::new("memory:///migrate");
                    let env: &[(&str, &std::ffi::OsStr)] =
                        if named { &[] } else { &[("FXDAX_EXTENTD_REMOTE", bucket)] };
                    let daemon = crate::common::spawn_daemon(test_setup, &socket, "extentd", env)?;
                    std::mem::forget(daemon);
                    socket
                }
                _ => std::env::var_os("FXDAX_EXTENTD").context("FXDAX_EXTENTD not set")?.into(),
            };
            let (snapshot, memory) = (dir.join("snapshot"), dir.join("memory"));
            if phase == Some(Phase::Target) {
                // The VM as the source host left it, where it was there.
                let bundle = bundle()?;
                for name in BUNDLED {
                    std::fs::copy(bundle.join(name), dir.join(name))
                        .with_context(|| format!("taking {name} from the bundle"))?;
                }
            } else {
                let mut vm = Vm::new(2, 512)?;
                vm.guest_kernel()?;
                let window = if self.dax { 1 << 28 } else { 0 };
                vm.extent_store_served(
                    "extents",
                    &store(dir, "source"),
                    CAPACITY,
                    window,
                    &extentd,
                );
                vm.read_only_root();
                let source_uds = uds("source");
                vm.vsock(&source_uds);
                let frozen = listen(&source_uds)?;
                let api = Vm::api_socket("source");
                let source = vm.spawn_api(&root, &test_setup.test_case, "source", &api)?;

                // Once the guest's filesystem is frozen, the snapshot, and the VM goes.
                let (stream, _) = frozen.accept()?;
                let mut said = String::new();
                BufReader::new(stream).read_line(&mut said)?;
                anyhow::ensure!(said == "frozen\n", "the guest said {said:?}");
                source.request("PATCH", "/vm", &serde_json::json!({ "state": "Paused" }))?;
                source.request(
                    "PUT",
                    "/snapshot/create",
                    &serde_json::json!({
                        "snapshot_type": "Full",
                        "snapshot_path": snapshot,
                        "mem_file_path": memory,
                    }),
                )?;
                source.kill()?;
            }
            if phase == Some(Phase::Source) {
                // For another host to restore: the point's branch is in the bucket.
                let bundle = bundle()?;
                std::fs::create_dir_all(&bundle)?;
                for name in BUNDLED {
                    std::fs::copy(dir.join(name), bundle.join(name))
                        .with_context(|| format!("leaving {name} in the bundle"))?;
                }
            }

            // Restored, told its name: each into its own store, but resuming.
            let mut restored = Vec::new();
            for &name in self.restores(phase) {
                let elsewhere = dir.join("elsewhere");
                let overrides = match self.mode {
                    Mode::Zygote => serde_json::json!([
                        { "id": "extents", "path_on_host": store(dir, name) },
                    ]),
                    Mode::Suspend => serde_json::json!([]),
                    Mode::Migrate => {
                        std::fs::create_dir_all(&elsewhere)?;
                        serde_json::json!([{
                            "id": "extents",
                            "path_on_host": store(&elsewhere, name),
                            "point_on_host": elsewhere.join("point"),
                            "extentd_socket": extentd,
                        }])
                    }
                };
                let uds = uds(name);
                let listener = listen(&uds)?;
                let firecracker = Firecracker::spawn(dir, name, &Vm::api_socket(name), None, &[])?;
                firecracker.request(
                    "PUT",
                    "/snapshot/load",
                    &serde_json::json!({
                        "snapshot_path": snapshot,
                        "mem_backend": { "backend_type": "File", "backend_path": memory },
                        "resume_vm": true,
                        "vsock_override": { "uds_path": uds },
                        "extent_overrides": overrides,
                    }),
                )?;
                // Each connection the guest makes, as one reset by the restore is made again.
                std::thread::spawn(move || -> std::io::Result<()> {
                    for stream in listener.incoming() {
                        writeln!(stream?, "{name}")?;
                    }
                    Ok(())
                });
                restored.push(firecracker);
            }
            for firecracker in restored {
                let (status, console) = firecracker.wait()?;
                crate::vm::split_console(std::fs::File::open(&console)?)?;
                anyhow::ensure!(status.success(), "a restored VM exited {status}");
            }
            Ok(())
        }
    }

    impl Test for TestFxfsGuestSnapshot {
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

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            let restores = self.restores(Phase::of_env().filter(|_| self.mode == Mode::Migrate));
            let expected: String = restores.iter().map(|name| format!("OK {name}\n")).collect();
            if output != expected {
                return TestOutcome::Fail(format!("expected {expected:?}, got {output:?}"));
            }
            // Each store holds what its guest wrote, checked, and not another's: but a migrated
            // one, whose data the daemon that served it took from its bucket, gone with it.
            let stores: Vec<(&str, PathBuf)> = match self.mode {
                Mode::Zygote => {
                    restores.iter().map(|name| (*name, store(&test_setup.tmp_dir, name))).collect()
                }
                Mode::Suspend => vec![(restores[0], store(&test_setup.tmp_dir, "source"))],
                Mode::Migrate => vec![],
            };
            for (name, path) in stores {
                let storage = Storage::open(path).expect("open store");
                let fs = block_on(async { Filesystem::mount(storage.check().await?).await })
                    .expect("mount store");
                assert_eq!(read_file(&fs, "before"), BEFORE, "store {name}: before");
                assert_eq!(read_file(&fs, "who"), name.as_bytes(), "store {name}: who");
                assert_eq!(read_file(&fs, "after").len(), 4 << 20, "store {name}: after");
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
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
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

    /// Freezes, or thaws, the filesystem.
    fn freeze(request: libc::c_ulong) {
        let dir = fs::File::open(MNT).unwrap();
        let ret = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, 0) };
        assert_eq!(ret, 0, "ioctl {request:#x}: {}", std::io::Error::last_os_error());
    }

    /// A connection to the host on `PORT`.
    fn connect() -> fs::File {
        try_connect().expect("connecting to the host")
    }

    /// A connection to the host on `PORT`, or why not.
    fn try_connect() -> std::io::Result<fs::File> {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
        use std::os::fd::{FromRawFd as _, IntoRawFd as _};
        let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
            .expect("a vsock socket");
        connect(fd.as_raw_fd(), &VsockAddr::new(2, PORT))?;
        // SAFETY: the socket's descriptor, which the file owns from here on.
        Ok(unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) })
    }

    /// The name the host gives this restored VM, on a new connection. The restore resets the
    /// guest's vsock connections, which its kernel learns of from an event it may take only after
    /// the old connection has ended, and a new one is made: a new connection reset so is made
    /// again (TASK-50.1.2).
    fn name_from_host() -> String {
        for _ in 0..100 {
            let mut name = String::new();
            let read = try_connect().and_then(|host| BufReader::new(host).read_line(&mut name));
            match read {
                Ok(_) => return name.trim_end().to_owned(),
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::NotConnected | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    // The reset is the kernel's to take: a moment for it.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(err) => panic!("reading the name from the host: {err}"),
            }
        }
        panic!("every connection to the host was reset");
    }

    /// Maps the `MAPPED_LEN` bytes of `path`, shared and read-only, which stays mapped.
    fn map(path: &str) -> &'static [u8] {
        let file = fs::File::open(path).unwrap();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MAPPED_LEN,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        // SAFETY: the mapping, never unmapped.
        unsafe { std::slice::from_raw_parts(map as *const u8, MAPPED_LEN) }
    }

    impl Test for TestFxfsGuestSnapshot {
        fn in_guest(self: Box<Self>) {
            mount();
            fs::write(format!("{MNT}/before"), BEFORE).unwrap();
            fs::write(format!("{MNT}/mapped"), mapped_pattern()).unwrap();
            let mapped = map(&format!("{MNT}/mapped"));
            // Read through the mapping, which with a DAX window maps the window's pages.
            assert!(mapped == &mapped_pattern()[..], "the mapping before the snapshot");
            freeze(FIFREEZE);
            {
                let mut host = connect();
                host.write_all(b"frozen\n").unwrap();
                // The snapshot is taken, and the VM goes; in each VM restored from it, the
                // connection is reset.
                let mut rest = Vec::new();
                let _ = host.read_to_end(&mut rest);
            }
            eprintln!("restored");
            let name = name_from_host();
            eprintln!("{name}: thawing");
            freeze(FITHAW);
            eprintln!("{name}: thawed");

            assert_eq!(fs::read(format!("{MNT}/before")).unwrap(), BEFORE);
            assert!(mapped == &mapped_pattern()[..], "the mapping after the restore");
            // SAFETY: the mapping `map` made, which nothing uses any more.
            assert_eq!(unsafe { libc::munmap(mapped.as_ptr() as *mut _, MAPPED_LEN) }, 0);
            fs::write(format!("{MNT}/who"), &name).unwrap();
            eprintln!("{name}: wrote");
            // New extents, of the restored store's branch.
            let after: Vec<u8> = (0..4 << 20).map(|i: usize| (i % 241) as u8).collect();
            let mut file = fs::File::create(format!("{MNT}/after")).unwrap();
            file.write_all(&after).unwrap();
            file.sync_all().unwrap();
            drop(file);
            unmount();
            mount();
            assert_eq!(fs::read(format!("{MNT}/who")).unwrap(), name.as_bytes());
            assert_eq!(fs::read(format!("{MNT}/after")).unwrap(), after);
            unmount();
            println!("OK {name}");
        }
    }
}
