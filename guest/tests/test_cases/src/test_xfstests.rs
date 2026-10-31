// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! xfstests (GPL-2.0) on fxdax's guest kernel's Fxfs, with a DAX window or without (TASK-44.15): the
//! kernel's own filesystem suite, its `generic/quick` group unless FXDAX_XFSTESTS names others.
//! Fxfs has no block device for it: its devices are extent stores, named by tags, as virtiofs's
//! are, which xfstests knows, so its image takes fxfs for virtiofs, and a test's scratch filesystem
//! is emptied rather than made anew.

use macros::{guest, host};

pub struct TestXfstests {
    /// xfstests-fxfs-guest-dax: whether the devices have DAX windows.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use std::ffi::CString;

    const CONTAINERFILE: &str = concat!(
        "FROM fedora:43\n",
        "RUN dnf install -y acl attr autoconf automake bc diffutils e2fsprogs findutils fio gawk ",
        "gcc git gzip hostname indent kmod libacl-devel libaio-devel libattr-devel libtool ",
        "libuuid-devel liburing-devel make openssl-devel perl procps-ng psmisc python3 sed ",
        "shadow-utils tar util-linux which xfsdump xfsprogs xfsprogs-devel && dnf clean all\n",
        "RUN git init /xfstests \\\n",
        " && git -C /xfstests fetch --depth 1 ",
        "https://git.kernel.org/pub/scm/fs/xfs/xfstests-dev.git refs/tags/v2026.10.15 \\\n",
        " && git -C /xfstests checkout FETCH_HEAD\n",
        "WORKDIR /xfstests\n",
        "RUN sed -E -i '/^[[:space:]]*[^#]*virtiofs[^#]*\\)/ s/virtiofs/virtiofs|fxfs/' ",
        "common/rc common/config\n",
        "RUN make -j8 && useradd -m fsgqa && useradd -m 123456-fsgqa && useradd -m fsgqa2\n",
    );

    /// The bytes of each filesystem, and of its DAX window if it has one.
    const SIZE: u64 = 4 << 30;
    const WINDOW: u64 = 1 << 30;

    impl Test for TestXfstests {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        fn timeout_secs(&self) -> u64 {
            4 * 60 * 60
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let mut vm = Vm::new(4, 4096)?;
            vm.guest_kernel()?;
            let window = if self.dax { WINDOW } else { 0 };
            vm.extent_store("extents", &test_setup.tmp_dir.join("extents"), SIZE, window)?;
            vm.extent_store("scratch", &test_setup.tmp_dir.join("scratch"), SIZE, window)?;
            // Check's arguments, separated by commas: a kernel's command line carries them.
            let tests = std::env::var("FXDAX_XFSTESTS").unwrap_or("-g,generic/quick".to_owned());
            vm.env(&CString::new(format!("XFSTESTS={tests}"))?);
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let stdout = String::from_utf8_lossy(&stdout);
            if stdout.lines().any(|line| line.starts_with("Passed all ")) {
                TestOutcome::Pass
            } else {
                TestOutcome::Fail(stdout.to_string())
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use std::process::Command;

    impl Test for TestXfstests {
        fn in_guest(self: Box<Self>) {
            for dir in ["/mnt/test", "/mnt/scratch"] {
                std::fs::create_dir_all(dir).unwrap();
            }
            let tests = std::env::var("XFSTESTS").unwrap();
            let status = Command::new("./check")
                .args(tests.split(','))
                .current_dir("/xfstests")
                .envs([
                    ("FSTYP", "fxfs"),
                    ("TEST_DEV", "extents"),
                    ("TEST_DIR", "/mnt/test"),
                    ("SCRATCH_DEV", "scratch"),
                    ("SCRATCH_MNT", "/mnt/scratch"),
                ])
                .status()
                .expect("run xfstests' check");
            println!("check: {status}");
        }
    }
}
