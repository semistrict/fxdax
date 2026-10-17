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
mod test_extent_guest;
use test_extent_guest::TestExtentGuest;

#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_checks;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_checks::TestFxfsGuestChecks;

#[cfg(target_os = "linux")]
mod kernel_log;

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
        if cfg!(target_os = "macos") {
            ShouldRun::No(reason)
        } else {
            ShouldRun::Yes
        }
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
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("fxfs-guest-checks", Box::new(TestFxfsGuestChecks { dax: false })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("fxfs-guest-checks-dax", Box::new(TestFxfsGuestChecks { dax: true })),
        TestCase::new("fxfs-guest-dax", Box::new(TestFxfsGuest::host_formats().with_dax())),
        TestCase::new("fxfs-guest-crash-dax", Box::new(TestFxfsGuest::crash().with_dax())),
        TestCase::new("extent-guest", Box::new(TestExtentGuest)),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fs-ext4-guest-kernel", Box::new(TestFsBench { backend: BenchBackend::Ext4GuestKernel })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fs-ext4-dax-pmem", Box::new(TestFsBench { backend: BenchBackend::Ext4DaxPmem })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fs-fxfs-guest", Box::new(TestFsBench { backend: BenchBackend::FxfsGuest })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fs-fxfs-guest-dax", Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDax })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fork-fxfs-guest-dax-warm", Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkWarm })),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("bench-fork-fxfs-guest-dax-cold", Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkCold })),
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
/// standard error goes there too, between these bytes (shift out, shift in), which the host
/// splits off its standard output.
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
