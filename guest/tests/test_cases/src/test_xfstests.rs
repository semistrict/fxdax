// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! xfstests (GPL-2.0) on fxdax's guest kernel's Fxfs, with a DAX window or without (TASK-44.15): the
//! kernel's own filesystem suite, its `generic/quick` group unless FXDAX_XFSTESTS names others.
//! Fxfs has no block device for it: its devices are extent stores, named by tags, as virtiofs's
//! are, which xfstests knows, so its image takes fxfs for virtiofs, and a test's scratch filesystem
//! is emptied rather than made anew. fxdax: but one of a size (`_scratch_mkfs_sized`, the enospc
//! group's), which is a device of its own, its store as large ([`SIZED`]), empty till its first
//! mount formats it, as the image's common/rc has it (TASK-149.25).
//!
//! Some tests run and fail for what Fxfs here doesn't do ([`KNOWN_FAILURES`]), each said why; one
//! of them that runs and passes fails the case, so that the list keeps no test it no longer needs
//! (TASK-149.20).

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
        "iproute libuuid-devel liburing-devel make openssl-devel perl procps-ng psmisc python3 ",
        "sed shadow-utils tar util-linux which xfsdump xfsprogs xfsprogs-devel quota ",
        "fio-engine-libaio && dnf clean all\n",
        "RUN git init /xfstests \\\n",
        " && git -C /xfstests fetch --depth 1 ",
        "https://git.kernel.org/pub/scm/fs/xfs/xfstests-dev.git refs/tags/v2026.10.15 \\\n",
        " && git -C /xfstests checkout FETCH_HEAD\n",
        "WORKDIR /xfstests\n",
        "RUN sed -E -i '/^[[:space:]]*[^#]*virtiofs[^#]*\\)/ s/virtiofs/virtiofs|fxfs/' ",
        "common/rc common/config\n",
        "RUN make -j8 && useradd -m fsgqa && useradd -m 123456-fsgqa && useradd -m fsgqa2\n",
    );

    /// fxdax: what the image's common/rc adds for Fxfs (TASK-149.25), after xfstests' own, which
    /// it overrides: a scratch filesystem of a size (`_try_scratch_mkfs_sized`) is the device of
    /// that size ([`SIZED`]), the test's from then on (SCRATCH_DEV), emptied as virtiofs's is; one
    /// of no device's size isn't run. Its first mount formats it. A scratch filesystem is
    /// unmounted by its mount point, as check unmounts whichever device a test left mounted.
    pub(super) const COMMON_RC: &str = r#"
# fxdax: Fxfs: a scratch filesystem of a size, a device as large (TASK-149.25).
eval "_upstream$(declare -f _try_scratch_mkfs_sized)"
eval "_upstream$(declare -f _scratch_unmount)"
_try_scratch_mkfs_sized()
{
	[ "$FSTYP" = fxfs ] || { _upstream_try_scratch_mkfs_sized "$@"; return; }
	_scratch_unmount 2>/dev/null
	export SCRATCH_DEV=scratch-$1
	_try_scratch_mount || _notrun "no Fxfs scratch device of $1 bytes"
	_scratch_unmount
	_scratch_cleanup_files
}
_scratch_unmount()
{
	[ "$FSTYP" = fxfs ] || { _upstream_scratch_unmount "$@"; return; }
	_unmount $SCRATCH_MNT
}
"#;

    /// The image: [`CONTAINERFILE`], and [`COMMON_RC`] added to common/rc as it is, each byte
    /// printf's octal escape.
    static IMAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let escaped: String = COMMON_RC.bytes().map(|byte| format!("\\{byte:03o}")).collect();
        format!("{CONTAINERFILE}RUN printf '{escaped}' >> common/rc\n")
    });

    /// Tests that fail for what Fxfs here doesn't do, which any other failure isn't.
    /// fxdax: no longer generic/225 (DAX appends' reserved blocks, in FIEMAP), 564 (past a DAX
    /// mount's s_maxbytes) and 798 (cachestat, DAX's no page cache), which passed with DAX and
    /// without once DAX copied on write, the files a mount makes in the page cache till read in
    /// again (GCE r695, r696, TASK-44.15); nor generic/303 and 525 (offsets past a DAX mount's
    /// s_maxbytes, EFBIG), 372 (one of FIEMAP's shared extents, TASK-143) and 401 (readdir's
    /// d_type of a device, fifo or socket, TASK-144), which passed with DAX and without on main
    /// d7a36088b, the first run of the nightly's (GCE r960, TASK-149.20.1).
    const KNOWN_FAILURES: [&str; 14] = [
        // An extent store reserves nothing, so fallocate leaves no unwritten extents, which they
        // look for with FIEMAP (`fxfs_vfs_fallocate`).
        "generic/009",
        "generic/092",
        "generic/094",
        "generic/255",
        "generic/568",
        "generic/796",
        "generic/797",
        // A heuristic of how few extents a reflinked file's data takes, which the allocator
        // makes it pass or not (r348, r352); its data is right either way.
        "generic/301",
        "generic/302",
        // FIEMAP doesn't say which extents a reflink shares (TASK-143).
        "generic/414",
        "generic/578",
        "generic/702",
        // Fxfs has no parent pointers, so a directory's handle can't be reconnected (TASK-145).
        "generic/467",
        "generic/477",
    ];

    /// Of [`KNOWN_FAILURES`], those that pass or not as the allocator places a reflinked file's
    /// data: none fails the case by passing.
    const SOMETIMES: [&str; 2] = ["generic/301", "generic/302"];

    /// The bytes of each filesystem, and of its DAX window if it has one.
    const SIZE: u64 = 4 << 30;

    /// fxdax: the sizes of scratch filesystem the tests ask `_scratch_mkfs_sized` for, a device of
    /// each, tagged `scratch-<bytes>`, its DAX window as large: generic/416's, 619's, 015's (and
    /// 027, 077, 083, 226, 449), 371's, 269's (and 546), 558's, 274's (and 275) and 312's.
    pub(super) const SIZED: [u64; 8] =
        [128 << 20, 240 << 20, 256 << 20, 330 << 20, 512 << 20, 1 << 30, 2 << 30, 5 << 30];
    const WINDOW: crate::common::Window = crate::common::Window::Large;

    impl Test for TestXfstests {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(IMAGE.as_str())
        }

        fn timeout_secs(&self) -> u64 {
            4 * 60 * 60
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let mut vm = Vm::new(4, 4096)?;
            vm.guest_kernel()?;
            let window = if self.dax { crate::common::dax_window(WINDOW)? } else { 0 };
            vm.extent_store("extents", &test_setup.tmp_dir.join("extents"), SIZE, window)?;
            vm.extent_store("scratch", &test_setup.tmp_dir.join("scratch"), SIZE, window)?;
            for size in SIZED {
                let window = if self.dax { size } else { 0 };
                let (tag, path) =
                    (format!("scratch-{size}"), test_setup.tmp_dir.join(format!("scratch-{size}")));
                vm.extent_store(&tag, &path, size, window)?;
            }
            // Check's arguments, separated by commas: a kernel's command line carries them.
            let tests = std::env::var("FXDAX_XFSTESTS").unwrap_or("-g,generic/quick".to_owned());
            vm.env(&CString::new(format!("XFSTESTS={tests}"))?);
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
            let stdout = String::from_utf8_lossy(&stdout);
            match verdict(&stdout, &KNOWN_FAILURES, &SOMETIMES) {
                Ok(()) => TestOutcome::Pass,
                Err(why) => TestOutcome::Fail(format!("{why}\n{stdout}")),
            }
        }
    }

    /// Whether xfstests' `check` says, in `stdout`, that no test failed but those of `known`, and
    /// that each of `known` it ran failed, but those of `sometimes`: one that passed is to go from
    /// the list (TASK-149.20). Or why not.
    pub(super) fn verdict(stdout: &str, known: &[&str], sometimes: &[&str]) -> Result<(), String> {
        let listed = |prefix: &str| -> Option<Vec<&str>> {
            let line = stdout.lines().find_map(|line| line.strip_prefix(prefix))?;
            Some(line.split_whitespace().collect())
        };
        let failures = match listed("Failures: ") {
            Some(failures) => failures,
            None if stdout.lines().any(|line| line.starts_with("Passed all ")) => Vec::new(),
            None => {
                return Err("xfstests' check said neither what failed nor that all passed".into());
            }
        };
        let unknown: Vec<_> = failures.iter().filter(|test| !known.contains(test)).collect();
        if !unknown.is_empty() {
            return Err(format!("failed, not known to: {unknown:?}"));
        }
        let ran = listed("Ran: ").unwrap_or_default();
        let passed: Vec<_> = (known.iter())
            .filter(|test| {
                ran.contains(test) && !failures.contains(test) && !sometimes.contains(test)
            })
            .collect();
        if !passed.is_empty() {
            return Err(format!(
                "known to fail, and passed: take them from KNOWN_FAILURES: {passed:?}"
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{COMMON_RC, IMAGE, SIZED, verdict};
        use std::process::Command;

        /// What `script` prints, run by bash after xfstests' functions as stubs that say what's
        /// called, and the image's common/rc.
        fn bash(script: &str) -> String {
            let stubs = r#"
_try_scratch_mkfs_sized() { echo "upstream mkfs $*"; }
_scratch_unmount() { echo "upstream unmount $SCRATCH_DEV"; }
_try_scratch_mount() { echo "mount $SCRATCH_DEV on $SCRATCH_MNT"; [ "$SCRATCH_DEV" != scratch-1 ]; }
_scratch_cleanup_files() { echo "emptied $SCRATCH_DEV"; }
_unmount() { echo "unmount $*"; }
_notrun() { echo "notrun: $*"; exit 0; }
SCRATCH_DEV=scratch SCRATCH_MNT=/mnt/scratch
"#;
            let output = Command::new("bash")
                .args(["-c", &format!("{stubs}{COMMON_RC}{script}")])
                .output()
                .expect("run bash");
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap()
        }

        /// The image's common/rc is [`COMMON_RC`], byte for byte, as printf writes it.
        #[test]
        fn the_image_adds_common_rc_as_it_is() {
            let line = IMAGE.lines().last().unwrap();
            let escaped = line.strip_prefix("RUN printf '").unwrap();
            let escaped = escaped.strip_suffix("' >> common/rc").unwrap();
            let printed = Command::new("sh").args(["-c", "printf \"$1\"", "sh", escaped]).output();
            assert_eq!(String::from_utf8(printed.unwrap().stdout).unwrap(), COMMON_RC);
        }

        /// Of Fxfs, a scratch filesystem of a size is the device of that size, emptied, the test's
        /// from then on, and one of no device's size isn't run; any other filesystem's is
        /// xfstests' own.
        #[test]
        fn a_scratch_filesystem_of_a_size_is_its_device() {
            let size = SIZED[2];
            assert_eq!(
                bash(&format!(
                    "FSTYP=fxfs\n_try_scratch_mkfs_sized {size}\n_scratch_unmount\necho $SCRATCH_DEV"
                )),
                format!(
                    "unmount /mnt/scratch\nmount scratch-{size} on /mnt/scratch\nunmount \
                     /mnt/scratch\nemptied scratch-{size}\nunmount /mnt/scratch\nscratch-{size}\n"
                )
            );
            assert_eq!(
                bash("FSTYP=fxfs\n_try_scratch_mkfs_sized 1\necho ran on"),
                "unmount /mnt/scratch\nmount scratch-1 on /mnt/scratch\nnotrun: no Fxfs scratch \
                 device of 1 bytes\n"
            );
            assert_eq!(
                bash("FSTYP=xfs\n_try_scratch_mkfs_sized 4096\n_scratch_unmount"),
                "upstream mkfs 4096\nupstream unmount scratch\n"
            );
        }

        const KNOWN: [&str; 3] = ["generic/009", "generic/301", "generic/401"];
        const SOMETIMES: [&str; 1] = ["generic/301"];

        #[test]
        fn known_failures_pass() {
            let stdout = "Ran: generic/001 generic/009 generic/401\nFailures: generic/009 generic/401\n\
                          Failed 2 of 3 tests\n";
            assert_eq!(verdict(stdout, &KNOWN, &SOMETIMES), Ok(()));
        }

        #[test]
        fn known_failures_not_run_pass() {
            let stdout =
                "Ran: generic/001 generic/009\nFailures: generic/009\nFailed 1 of 2 tests\n";
            assert_eq!(verdict(stdout, &KNOWN, &SOMETIMES), Ok(()));
            assert_eq!(
                verdict("Ran: generic/001\nPassed all 1 tests\n", &KNOWN, &SOMETIMES),
                Ok(())
            );
        }

        #[test]
        fn other_failures_fail() {
            let stdout = "Ran: generic/001 generic/009\nFailures: generic/001 generic/009\n";
            assert_eq!(
                verdict(stdout, &KNOWN, &SOMETIMES),
                Err(r#"failed, not known to: ["generic/001"]"#.into())
            );
        }

        #[test]
        fn known_failures_that_pass_fail() {
            let stdout =
                "Ran: generic/009 generic/401\nFailures: generic/009\nFailed 1 of 2 tests\n";
            assert_eq!(
                verdict(stdout, &KNOWN, &SOMETIMES),
                Err(r#"known to fail, and passed: take them from KNOWN_FAILURES: ["generic/401"]"#
                    .into())
            );
            let stdout = "Ran: generic/009 generic/401\nPassed all 2 tests\n";
            assert_eq!(
                verdict(stdout, &KNOWN, &SOMETIMES),
                Err(r#"known to fail, and passed: take them from KNOWN_FAILURES: ["generic/009", "generic/401"]"#.into())
            );
        }

        #[test]
        fn known_failures_that_sometimes_pass_pass_either_way() {
            let stdout = "Ran: generic/301\nPassed all 1 tests\n";
            assert_eq!(verdict(stdout, &KNOWN, &SOMETIMES), Ok(()));
            let stdout = "Ran: generic/301\nFailures: generic/301\nFailed 1 of 1 tests\n";
            assert_eq!(verdict(stdout, &KNOWN, &SOMETIMES), Ok(()));
        }

        #[test]
        fn a_check_that_said_nothing_fails() {
            let said = verdict("check: exit status: 1\n", &KNOWN, &SOMETIMES);
            assert_eq!(
                said,
                Err("xfstests' check said neither what failed nor that all passed".into())
            );
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use std::process::Command;
    use std::time::Duration;

    const WATCH_EVERY: Duration = Duration::from_secs(300);

    impl Test for TestXfstests {
        fn in_guest(self: Box<Self>) {
            for dir in ["/mnt/test", "/mnt/scratch"] {
                std::fs::create_dir_all(dir).unwrap();
            }
            // What a shell's process substitution and scripts look for, which a devtmpfs lacks.
            for (link, target) in [
                ("/dev/fd", "/proc/self/fd"),
                ("/dev/stdin", "/proc/self/fd/0"),
                ("/dev/stdout", "/proc/self/fd/1"),
                ("/dev/stderr", "/proc/self/fd/2"),
            ] {
                std::os::unix::fs::symlink(target, link).unwrap();
            }
            // Locks' tests talk over it, generic/131's.
            let status = Command::new("ip").args(["link", "set", "lo", "up"]).status();
            assert!(status.expect("run ip").success());
            // The rootfs's files are all the runner's (`--no-same-owner`), but a login needs its home.
            for user in ["fsgqa", "fsgqa2", "123456-fsgqa"] {
                let owner = format!("{user}:{user}");
                let home = format!("/home/{user}");
                let status = Command::new("chown").args(["-R", &owner, &home]).status();
                assert!(status.expect("run chown").success());
            }
            // A test that spins, each spin yielding, trips no lockup detector, and says nothing
            // for ever: what each CPU is doing goes to the console now and then (GCE r346, r348).
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(WATCH_EVERY);
                    std::fs::write("/proc/sysrq-trigger", "l").unwrap();
                }
            });
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
            print_failed_logs();
        }
    }

    /// The end of each failed test's diff and its own log, which the console is all that leaves
    /// the guest.
    fn print_failed_logs() {
        const LOG_TAIL: usize = 4096;
        let Ok(results) = std::fs::read_dir("/xfstests/results/generic") else { return };
        let paths: Vec<_> = results.map(|entry| entry.unwrap().path()).collect();
        for bad in paths.iter().filter(|path| path.to_string_lossy().ends_with(".out.bad")) {
            for path in [bad.clone(), bad.with_extension("").with_extension("full")] {
                let Ok(log) = std::fs::read(&path) else { continue };
                let tail = String::from_utf8_lossy(&log[log.len().saturating_sub(LOG_TAIL)..]);
                println!("==> {}\n{tail}", path.display());
            }
        }
    }
}
