// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax TASK-111.4: `fxdaxctl stats --json` takes one snapshot of what the extent daemon, a live
//! VM's process and its guest counted, each with the commit it was built of.
//!
//! The host boots the guest kernel with an extent-store device with a DAX window, on a new store,
//! which the guest formats as it mounts it. The guest writes a file, durably, and reads it through
//! the window; then, twice, it reads what its Fxfs counted (/proc/fs/fxfs/extents), asks the host
//! over vsock for a snapshot, and reads it again. The host runs fxdaxctl (FXDAXCTL_BIN) against the
//! daemon's control socket, keeps its JSON in the test's directory (`stats-N.json`), and checks it:
//! the daemon's build and the VM process's, one commit; the VM's store, answered, its window's
//! mappings and its host mappings within their budget; its guest's build, a commit, and its
//! counters, each between what the guest read before asking and after, which the host's answer
//! tells it. The second snapshot is later than the first, and counts the guest's writes since.

use macros::{guest, host};

pub struct TestFxfsGuestStats;

const MIB: u64 = 1 << 20;
/// The store's capacity.
#[cfg_attr(feature = "guest", allow(dead_code))]
const CAPACITY: u64 = 256 * MIB;
/// The DAX window, as large as the data ([`crate::common::dax_window`]).
#[cfg(feature = "host")]
const WINDOW: crate::common::Window = crate::common::Window::Large;
/// The vsock port the guest asks for each snapshot on: a line of `stats`, and the counters it read,
/// `NAME=VALUE` each; the host answers with `ok` and what the snapshot says of them, or the
/// failure.
#[cfg_attr(feature = "guest", allow(dead_code))]
const PORT: u32 = 5051;
/// The guest's counters compared: its device's writes and the window's map requests.
#[cfg_attr(not(feature = "guest"), allow(dead_code))]
const COUNTERS: [&str; 2] = ["device_writes", "window_map_requests"];

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use anyhow::{Context, bail, ensure};
    use serde_json::Value;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    /// The daemon's snapshot, as `fxdaxctl stats --json` prints it through the control socket of
    /// the daemon at `extentd`.
    fn stats(extentd: &Path) -> anyhow::Result<(String, Value)> {
        let fxdaxctl = std::env::var_os("FXDAXCTL_BIN").context("FXDAXCTL_BIN")?;
        let output = std::process::Command::new(fxdaxctl)
            .args(["stats", "--json"])
            .env("FXDAX_EXTENTD_GRPC", extentd.with_extension("control"))
            .output()?;
        let stdout = String::from_utf8(output.stdout)?;
        ensure!(
            output.status.success(),
            "fxdaxctl stats --json: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json = serde_json::from_str(&stdout).with_context(|| format!("{stdout:?}"))?;
        Ok((stdout, json))
    }

    /// Whether `build` is a commit, `-dirty` if a tracked file differed from it, as
    /// scripts/build-commit.sh says one.
    fn is_commit(build: &str) -> bool {
        let commit = build.strip_suffix("-dirty").unwrap_or(build);
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    /// Checks the snapshot `json` holds the daemon's and the VM process's builds, one commit, and
    /// the VM's store, at `store`, its process's and its guest's counters; and returns those of its
    /// guest's `asked` names, and when it was taken.
    fn check(json: &Value, store: &Path, asked: &[&str]) -> anyhow::Result<(Vec<u64>, u64)> {
        let build = json["extentd"]["build"].as_str().context("the daemon's build")?;
        ensure!(is_commit(build), "the daemon's build: {build:?}");
        ensure!(json["extentd"]["counters"]["maps"].as_u64().is_some(), "the daemon's counters");
        let vms = json["vms"].as_array().context("vms")?;
        let vm = (vms.iter())
            .find(|vm| vm["path"].as_str().map(PathBuf::from).as_deref() == Some(store))
            .with_context(|| format!("no VM of {}", store.display()))?;
        ensure!(vm["follower"] == false, "a writer: {vm}");
        ensure!(vm["answered"] == true, "it answered: {vm}");
        ensure!(vm["vmm"]["build"].as_str() == Some(build), "the VM process's build: {vm}");
        ensure!(vm["vmm"]["counters"]["stage_ops"].as_u64() > Some(0), "stage operations: {vm}");
        let gauges = &vm["vmm"]["gauges"];
        let installed = gauges["host_mappings"].as_u64().context("its host mappings")?;
        let budget = gauges["host_mapping_budget"].as_u64().context("their budget")?;
        ensure!(installed <= budget && budget > 0, "its mappings: {vm}");
        ensure!(vm["window"]["mappings"].as_u64() > Some(0), "its window's mappings: {vm}");
        let guest = &vm["guest"];
        let guest_build = guest["build"].as_str().context("its guest's build")?;
        ensure!(is_commit(guest_build), "its guest's build: {guest_build:?}");
        ensure!(guest["histograms"]["window_map_requests"].as_array().is_some(), "{guest}");
        let counters = (asked.iter())
            .map(|name| {
                guest["counters"][name].as_u64().with_context(|| format!("{name}: {guest}"))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok((counters, json["taken_unix_ns"].as_u64().context("taken_unix_ns")?))
    }

    /// Takes a snapshot each time the guest asks, on the connection `listener` takes, keeping its
    /// JSON in `dir`, and answers with what it says of the guest's counters, or why it failed.
    fn snapshots(
        listener: UnixListener,
        extentd: &Path,
        store: &Path,
        dir: &Path,
    ) -> anyhow::Result<()> {
        let (stream, _) = listener.accept()?;
        let mut answers = stream.try_clone()?;
        let store = store.canonicalize()?;
        let mut taken_before = 0;
        for (round, line) in BufReader::new(stream).lines().enumerate() {
            let line = line?;
            let read: Vec<(&str, u64)> = (line.strip_prefix("stats").context("an ask")?)
                .split_whitespace()
                .map(|pair| {
                    let (name, value) = pair.split_once('=').context("NAME=VALUE")?;
                    Ok((name, value.parse()?))
                })
                .collect::<anyhow::Result<_>>()?;
            let checked = stats(extentd).and_then(|(text, json)| {
                std::fs::write(dir.join(format!("stats-{}.json", round + 1)), &text)?;
                eprintln!("fxdaxctl stats --json, snapshot {}:\n{text}", round + 1);
                let names: Vec<&str> = read.iter().map(|(name, _)| *name).collect();
                let (counters, taken) = check(&json, &store, &names)?;
                ensure!(taken > taken_before, "taken at {taken}, before {taken_before}");
                taken_before = taken;
                for ((name, before), counted) in read.iter().zip(&counters) {
                    ensure!(counted >= before, "{name}: {counted}, the guest read {before} before");
                }
                Ok(names
                    .iter()
                    .zip(counters)
                    .map(|(name, value)| format!(" {name}={value}"))
                    .collect::<String>())
            });
            match checked {
                Ok(said) => writeln!(answers, "ok{said}")?,
                Err(err) => {
                    let err = format!("{err:#}").replace('\n', " ");
                    writeln!(answers, "{err}")?;
                    bail!(err);
                }
            }
        }
        Ok(())
    }

    impl Test for TestFxfsGuestStats {
        fn should_run(&self) -> ShouldRun {
            if std::env::var_os("FXDAXCTL_BIN").is_none() {
                return ShouldRun::No("FXDAXCTL_BIN does not name fxdaxctl");
            }
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            60
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let store = test_setup.tmp_dir.join("extents");
            let root = setup_rootfs(&test_setup)?;
            let extentd = PathBuf::from(
                std::env::var_os("FXDAX_EXTENTD").expect("FXDAX_EXTENTD names the daemon"),
            );
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", &store, CAPACITY, crate::common::dax_window(WINDOW)?)?;
            // Short, as Unix sockets' paths must be: the test's directory may be deep.
            let uds =
                std::env::temp_dir().join(format!("fxdax-stats-{}.vsock", std::process::id()));
            let listener = UnixListener::bind(format!("{}_{PORT}", uds.display()))?;
            vm.vsock(&uds);
            let dir = test_setup.tmp_dir.clone();
            std::thread::spawn(move || {
                // Created as the guest mounts it, before it asks.
                if let Err(err) = snapshots(listener, &extentd, &store, &dir) {
                    eprintln!("stats: {err:#}");
                }
            });
            vm.enter(&root, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
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
    use std::io::{BufRead, BufReader, Write};

    const MNT: &str = "/mnt/fxfs";

    fn mount() {
        let source = CString::new("extents").unwrap();
        let target = CString::new(MNT).unwrap();
        let fstype = CString::new("fxfs").unwrap();
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    /// Writes `len` bytes to a new file at `path`, durably, and reads them back through the DAX
    /// window, evicted first.
    fn write_and_read(path: &str, len: u64) {
        let chunk: Vec<u8> = (0..MIB).map(|i| (i % 251) as u8).collect();
        let mut file = fs::File::create(path).unwrap();
        for _ in 0..len / MIB {
            file.write_all(&chunk).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);
        crate::guest_fxfs::evict(path);
        assert!(crate::guest_fxfs::is_dax(path), "{path} isn't DAX");
        let read = fs::read(path).unwrap();
        assert!(read.chunks(MIB as usize).all(|read| read == chunk), "{path} reads otherwise");
    }

    /// What the guest's Fxfs counted of [`COUNTERS`], as it says now.
    fn counted() -> Vec<u64> {
        let stats = crate::guest_fxfs::stats("extents");
        COUNTERS.iter().map(|name| stats[*name]).collect()
    }

    /// The host, over vsock: where the guest asks for snapshots.
    struct Host {
        asks: fs::File,
        answers: BufReader<fs::File>,
    }

    impl Host {
        fn connect() -> Self {
            use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
            use std::os::fd::{FromRawFd as _, IntoRawFd as _};
            let fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None)
                .expect("a vsock socket");
            connect(std::os::fd::AsRawFd::as_raw_fd(&fd), &VsockAddr::new(2, PORT))
                .expect("connecting to the host");
            // SAFETY: the socket's descriptor, which the file owns from here on.
            let asks = unsafe { fs::File::from_raw_fd(fd.into_raw_fd()) };
            let answers = BufReader::new(asks.try_clone().unwrap());
            Self { asks, answers }
        }

        /// Asks for a snapshot, having read `before` of [`COUNTERS`], and returns what it says
        /// of them.
        fn snapshot(&mut self, before: &[u64]) -> Vec<u64> {
            let read: String = COUNTERS
                .iter()
                .zip(before)
                .map(|(name, value)| format!(" {name}={value}"))
                .collect();
            writeln!(self.asks, "stats{read}").unwrap();
            let mut answer = String::new();
            self.answers.read_line(&mut answer).unwrap();
            let said = answer.trim_end().strip_prefix("ok");
            let said = said.unwrap_or_else(|| panic!("the host's snapshot: {answer}"));
            (said.split_whitespace())
                .zip(COUNTERS)
                .map(|(pair, name)| {
                    let value = pair.strip_prefix(&format!("{name}=")).unwrap();
                    value.parse().unwrap()
                })
                .collect()
        }
    }

    impl Test for TestFxfsGuestStats {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            write_and_read(&format!("{MNT}/first"), 8 * MIB);
            let mut host = Host::connect();
            let mut snapshots = Vec::new();
            for round in 0..2 {
                if round == 1 {
                    write_and_read(&format!("{MNT}/second"), 8 * MIB);
                }
                let before = counted();
                let snapshot = host.snapshot(&before);
                let after = counted();
                for ((name, snapshot), after) in COUNTERS.iter().zip(&snapshot).zip(&after) {
                    assert!(snapshot <= after, "{name}: the snapshot's {snapshot}, after {after}");
                }
                snapshots.push(snapshot);
            }
            // The guest's writes since the first, in the second.
            assert!(snapshots[1][0] > snapshots[0][0], "device_writes: {snapshots:?}");
            unmount();
            println!("OK");
        }
    }
}
