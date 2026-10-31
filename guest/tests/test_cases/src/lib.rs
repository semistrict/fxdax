// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

//! fxdax's guest tests: VMs booted with fxdax's Firecracker and guest kernel, each a test case run
//! by the runner (`runner test --test-case NAME`), its host half here under the `host` feature and
//! its guest half, in the guest agent, under `guest`. Moved from libkrun's tests (TASK-40), whose
//! framework this is.

mod test_pjdfstest;
use test_pjdfstest::TestPjdfstest;

mod test_fxfs_guest;
use test_fxfs_guest::TestFxfsGuest;
mod test_fxfs_guest_resize;
use test_fxfs_guest_resize::TestFxfsGuestResize;
mod test_fxfs_guest_live_fork;
use test_fxfs_guest_live_fork::TestFxfsGuestLiveFork;
mod test_fxfs_guest_pgbench;
use test_fxfs_guest_pgbench::{Probe, Storage as PgbenchStorage, TestFxfsGuestPgbench};
mod test_fxfs_guest_daemon_lost;
use test_fxfs_guest_daemon_lost::TestFxfsGuestDaemonLost;
mod test_fxfs_guest_follow;
use test_fxfs_guest_follow::TestFxfsGuestFollow;
mod test_fxfs_guest_follow_remount;
use test_fxfs_guest_follow_remount::TestFxfsGuestFollowRemount;
mod test_fxfs_guest_follow_torture;
use test_fxfs_guest_follow_torture::{Mutant, TestFxfsGuestFollowTorture};
mod test_fxfs_guest_import;
use test_fxfs_guest_import::TestFxfsGuestImport;
mod test_fxfs_guest_docker;
use test_fxfs_guest_docker::TestFxfsGuestDocker;
mod test_fxfs_guest_snapshot;
use test_fxfs_guest_snapshot::{Mode as SnapshotMode, TestFxfsGuestSnapshot};
mod test_extent_guest;
use test_extent_guest::TestExtentGuest;
mod test_runner_deadlock;
use test_runner_deadlock::TestRunnerDeadlock;

#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_checks;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_checks::TestFxfsGuestChecks;

// What the guest's checks print as they fail.
#[cfg(feature = "guest")]
mod kernel_log;

// What the guest's tests unbind the extent driver with.
#[cfg(feature = "guest")]
mod extent_driver;

#[cfg(any(feature = "guest", test))]
mod sqlite;
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fs_bench;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fs_bench::{Backend as BenchBackend, TestFsBench};

pub enum TestOutcome {
    Pass,
    Fail(String),
    Timeout,
    Skip(&'static str),
    Report(Box<dyn ReportImpl>),
}

pub enum ShouldRun {
    Yes,
    No(&'static str),
}

impl ShouldRun {
    /// Returns Yes unless on macOS, in which case returns No with the given reason.
    pub fn yes_unless_macos(reason: &'static str) -> Self {
        if cfg!(target_os = "macos") { ShouldRun::No(reason) } else { ShouldRun::Yes }
    }
}

pub fn test_cases() -> Vec<TestCase> {
    // Register your test here:
    vec![
        TestCase::new("pjdfstest-fxfs-guest", Box::new(TestPjdfstest { dax: false })),
        TestCase::new("pjdfstest-fxfs-guest-dax", Box::new(TestPjdfstest { dax: true })),
        TestCase::new("fxfs-guest", Box::new(TestFxfsGuest::host_formats())),
        TestCase::new("fxfs-guest-format", Box::new(TestFxfsGuest::guest_formats())),
        TestCase::new("fxfs-guest-crash", Box::new(TestFxfsGuest::crash())),
        TestCase::new("fxfs-guest-resize", Box::new(TestFxfsGuestResize)),
        TestCase::new("fxfs-guest-live-fork", Box::new(TestFxfsGuestLiveFork)),
        TestCase::new(
            "fxfs-guest-pgbench",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::Fxfs,
                probe: Probe::None,
                fsync: true,
            }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::FxfsDax,
                probe: Probe::None,
                fsync: true,
            }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-profile",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::Fxfs,
                probe: Probe::Profile,
                fsync: true,
            }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax-profile",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::FxfsDax,
                probe: Probe::Profile,
                fsync: true,
            }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-locks",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::Fxfs,
                probe: Probe::Locks,
                fsync: true,
            }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-nofsync",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::Fxfs,
                probe: Probe::None,
                fsync: false,
            }),
        ),
        TestCase::new(
            "pgbench-ext4-dax-pmem",
            Box::new(TestFxfsGuestPgbench {
                storage: PgbenchStorage::Ext4DaxPmem,
                probe: Probe::None,
                fsync: true,
            }),
        ),
        TestCase::new("fxfs-guest-daemon-lost", Box::new(TestFxfsGuestDaemonLost)),
        TestCase::new("runner-deadlock", Box::new(TestRunnerDeadlock)),
        TestCase::new("fxfs-guest-follow", Box::new(TestFxfsGuestFollow { dax: false })),
        TestCase::new("fxfs-guest-follow-dax", Box::new(TestFxfsGuestFollow { dax: true })),
        TestCase::new(
            "fxfs-guest-follow-remount",
            Box::new(TestFxfsGuestFollowRemount { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-follow-remount-dax",
            Box::new(TestFxfsGuestFollowRemount { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture",
            Box::new(TestFxfsGuestFollowTorture { dax: false, mutant: None }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-dax",
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: None }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant",
            Box::new(TestFxfsGuestFollowTorture { dax: false, mutant: Some(Mutant::Invalidation) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-dax",
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Invalidation) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-faults-dax",
            Box::new(TestFxfsGuestFollowTorture {
                dax: true,
                mutant: Some(Mutant::InvalidationFaults),
            }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-barrier-dax",
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Barrier) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-nested-dax",
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Nested) }),
        ),
        TestCase::new("fxfs-guest-import", Box::new(TestFxfsGuestImport { dax: false })),
        TestCase::new("fxfs-guest-import-dax", Box::new(TestFxfsGuestImport { dax: true })),
        TestCase::new(
            "fxfs-guest-snapshot",
            Box::new(TestFxfsGuestSnapshot { dax: false, mode: SnapshotMode::Zygote }),
        ),
        TestCase::new(
            "fxfs-guest-snapshot-dax",
            Box::new(TestFxfsGuestSnapshot { dax: true, mode: SnapshotMode::Zygote }),
        ),
        TestCase::new(
            "fxfs-guest-suspend-dax",
            Box::new(TestFxfsGuestSnapshot { dax: true, mode: SnapshotMode::Suspend }),
        ),
        TestCase::new(
            "fxfs-guest-migrate-dax",
            Box::new(TestFxfsGuestSnapshot { dax: true, mode: SnapshotMode::Migrate }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks",
            Box::new(TestFxfsGuestChecks { dax: false, delay: false, small_window: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax",
            Box::new(TestFxfsGuestChecks { dax: true, delay: false, small_window: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax-delay",
            Box::new(TestFxfsGuestChecks { dax: true, delay: true, small_window: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax-small",
            Box::new(TestFxfsGuestChecks { dax: true, delay: false, small_window: true }),
        ),
        TestCase::new("fxfs-guest-dax", Box::new(TestFxfsGuest::host_formats().with_dax())),
        TestCase::new("fxfs-guest-crash-dax", Box::new(TestFxfsGuest::crash().with_dax())),
        // fxdax: the VM stopped with an evacuation held at each of its steps, the pause points
        // GATHERED, EVACUEES_MAPPED and PLACED of guest/linux/fs/fxfs/dax.c (TASK-119.16).
        TestCase::new("fxfs-guest-crash-dax-gathered", Box::new(TestFxfsGuest::crash_at(4))),
        TestCase::new("fxfs-guest-crash-dax-mapped", Box::new(TestFxfsGuest::crash_at(6))),
        TestCase::new("fxfs-guest-crash-dax-placed", Box::new(TestFxfsGuest::crash_at(7))),
        TestCase::new("fxfs-guest-crash-dax-killed", Box::new(TestFxfsGuest::crash_killed())),
        TestCase::new("fxfs-guest-faults-dax", Box::new(TestFxfsGuest::faults())),
        TestCase::new("fxfs-guest-docker-dax", Box::new(TestFxfsGuestDocker)),
        TestCase::new("extent-guest", Box::new(TestExtentGuest)),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-guest-kernel",
            Box::new(TestFsBench { backend: BenchBackend::Ext4GuestKernel }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-dax-pmem",
            Box::new(TestFsBench { backend: BenchBackend::Ext4DaxPmem }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-pmem",
            Box::new(TestFsBench { backend: BenchBackend::Ext4Pmem }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuest }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest-dax",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDax }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest-dax-followed",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxFollowed }),
        ),
        TestCase::new(
            "bench-fs-fxfs-guest-dax-followed-idle",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxFollowedIdle }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-dax-warm",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkWarm }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-dax-cold",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkCold }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-warm",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestForkWarm }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-ext4-dax-pmem",
            Box::new(TestFsBench { backend: BenchBackend::Ext4DaxPmemFork }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-ext4-pmem",
            Box::new(TestFsBench { backend: BenchBackend::Ext4PmemFork }),
        ),
        TestCase::new(
            "bench-fork-fxfs-guest-dax-profiled",
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkProfiled }),
        ),
    ]
}

////////////////////
// Implementation details:
//////////////////

pub trait ReportImpl {
    fn fmt_text(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    fn fmt_gh_markdown(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

pub trait Report: ReportImpl {
    fn text(&self) -> ReportText<'_, Self> {
        ReportText(self)
    }

    fn gh_markdown(&self) -> ReportGhMarkdown<'_, Self> {
        ReportGhMarkdown(self)
    }
}

impl<T: ReportImpl + ?Sized> Report for T {}

pub struct ReportText<'a, T: ReportImpl + ?Sized>(pub &'a T);

impl<T: ReportImpl + ?Sized> std::fmt::Display for ReportText<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_text(f)
    }
}

pub struct ReportGhMarkdown<'a, T: ReportImpl + ?Sized>(pub &'a T);

impl<T: ReportImpl + ?Sized> std::fmt::Display for ReportGhMarkdown<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_gh_markdown(f)
    }
}

use macros::{guest, host};
#[host]
use std::path::PathBuf;

#[cfg(all(feature = "guest", feature = "host"))]
compile_error!("Cannot enable both guest and host in the same binary!");

#[cfg(feature = "host")]
mod common;

#[cfg(feature = "host")]
pub mod rootfs;

#[cfg(feature = "host")]
pub mod vm;

#[cfg(all(feature = "guest", target_os = "linux"))]
pub mod guest_init;

/// Under a VMM whose guest has a single console (Firecracker's serial port), the guest agent's
/// standard output goes there between these bytes (start and end of text), and its standard error
/// between the next two (shift out, shift in), which the host splits apart, and from what the
/// kernel says there.
pub const STDOUT_START: u8 = 0x02;
pub const STDOUT_END: u8 = 0x03;
pub const STDERR_START: u8 = 0x0e;
pub const STDERR_END: u8 = 0x0f;

#[host]
#[derive(Clone, Debug)]
pub struct TestSetup {
    pub test_case: String,
    // A tmp directory for misc. artifacts used be the test (e.g. sockets)
    pub tmp_dir: PathBuf,
}

#[host]
impl TestSetup {
    /// Register a PID to be killed after the test finishes.
    ///
    /// The runner will SIGKILL these PIDs after check() returns, even if the
    /// test crashed. Use this for background processes (e.g. gvproxy) that
    /// outlive the VM.
    pub fn register_cleanup_pid(&self, pid: u32) {
        use std::io::Write;
        let path = self.tmp_dir.join("cleanup.pids");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("Failed to open cleanup.pids");
        writeln!(file, "{}", pid).expect("Failed to write cleanup PID");
    }
}

#[host]
pub trait Test {
    /// Start the VM
    fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()>;

    /// Checks the output of the (host) process which started the VM
    fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
        let output = String::from_utf8(stdout).unwrap();
        if output == "OK\n" {
            TestOutcome::Pass
        } else {
            TestOutcome::Fail(format!("expected exactly {:?}, got {:?}", "OK\n", output))
        }
    }

    /// Check if this test should run on this platform.
    fn should_run(&self) -> ShouldRun {
        ShouldRun::Yes
    }

    /// Return Containerfile content if this test needs a custom rootfs image.
    /// The runner will build the image via podman and extract it before launching the VM.
    /// If podman is unavailable, the test is skipped.
    fn rootfs_image(&self) -> Option<&'static str> {
        None
    }

    /// Per-test timeout in seconds. The runner kills the test if it exceeds this.
    fn timeout_secs(&self) -> u64 {
        15
    }

    /// fxdax: whether the runner fails the test, once it passed, on its VMs' kernels complaining
    /// on their consoles (`vm::complaints`, TASK-44.8): unless it means them to.
    fn checks_console(&self) -> bool {
        true
    }
}

#[guest]
pub trait Test {
    /// This will be executed in the guest, you can panic! if the test failed!
    fn in_guest(self: Box<Self>) {}
}

pub struct TestCase {
    pub name: &'static str,
    pub test: Box<dyn Test>,
}

impl TestCase {
    // Your test can be parametrized, so you can add the same test multiple times constructed with
    // different parameters with and specify a different name here.
    pub fn new(name: &'static str, test: Box<dyn Test>) -> Self {
        Self { name, test }
    }

    /// Check if this test should run on this platform.
    #[host]
    pub fn should_run(&self) -> ShouldRun {
        self.test.should_run()
    }

    #[host]
    pub fn rootfs_image(&self) -> Option<&'static str> {
        self.test.rootfs_image()
    }

    #[host]
    pub fn timeout_secs(&self) -> u64 {
        self.test.timeout_secs()
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_testcases_have_unique_names() {
        let test_cases = test_cases();
        let mut names: HashSet<&str> = HashSet::new();

        for test_case in test_cases {
            let name = test_case.name();
            let was_inserted = names.insert(name);
            if !was_inserted {
                panic!("test_cases() contains multiple items named `{name}`")
            }

            if name == "all" {
                panic!("test_cases() contains test named {name}, but the name is reseved")
            }
        }
    }
}
