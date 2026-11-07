// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Test the extent-store device and the guest kernel's driver of it (fxdax's
// TASK-26.4 and TASK-26.5).
//
// The host boots the kernel fxdax's scripts/build-guest-kernel.sh builds, which
// FXDAX_GUEST_KERNEL names, with an extent-store device on a new store with a
// DAX window. The guest runs the driver's self-test, which issues every request
// of the protocol, many writes and reads at once, and reads an extent mapped
// into the DAX window in place. After the VM exits the host opens the store and
// checks what the self-test committed.

use macros::{guest, host};

pub struct TestExtentGuest;

/// fxdax TASK-141.3: the driver refuses a device of another protocol version at probe, naming both:
/// here the device is of this build's, and the driver is told it speaks the next
/// (`virtio_extent.version`), as a guest of another build would. No device is bound, and the
/// kernel's log says why. Before, the device's configuration said no version, and a driver of any
/// build served it.
pub struct TestExtentGuestOtherVersion;

/// The version the driver is told it speaks: the device's, this build's, and one more.
const OTHER_VERSION: (u32, u32) = (1, 2);

/// The root the driver's self-test commits.
#[cfg_attr(feature = "guest", allow(dead_code))]
const ROOT: &[u8] = b"fxdax extent store self-test";

#[host]
mod host {
    use super::*;

    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};

    impl Test for TestExtentGuest {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let store = test_setup.tmp_dir.join("extents");
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", &store, 64 << 20, 64 << 20)?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
            }
            // What the self-test committed is in the store.
            let store = fxdax::ExtentStore::open(test_setup.tmp_dir.join("extents"))
                .expect("open the extent store");
            assert_eq!(store.root().unwrap(), ROOT);
            let info = store.info().unwrap();
            // 34 extents of two blocks written, sequences 0 to 33, of which two were freed: the
            // last, a write whose commit set the root (WRITE_ROOT), by the self-test, its root put
            // back.
            assert_eq!(info.used, 32 * 2 * 4096);
            // fxdax: a store opened says one past the highest of its extents it holds, 32, where
            // fxfs looks for orphans below (TASK-165.9's one recovery; before, its footers'
            // stamp, one past the highest written). The freed 33 is minted no more: its writer
            // mints from its floor, past every number an earlier writer minted.
            assert_eq!(info.next_sequence, 33);
            assert!(info.sequence_floor > 33, "the floor, {}, is past 33", info.sequence_floor);
            TestOutcome::Pass
        }
    }

    impl Test for TestExtentGuestOtherVersion {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let store = test_setup.tmp_dir.join("extents");
            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", &store, 64 << 20, 64 << 20)?;
            let param = format!("virtio_extent.version={}", OTHER_VERSION.1);
            vm.module_param(&std::ffi::CString::new(param)?);
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
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

    use std::fs;

    const DRIVER: &str = "/sys/bus/virtio/drivers/virtio_extent";

    impl Test for TestExtentGuest {
        fn in_guest(self: Box<Self>) {
            let devices: Vec<_> = fs::read_dir(DRIVER)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.join("selftest").exists())
                .collect();
            assert_eq!(devices.len(), 1, "{devices:?}");
            let selftest = devices[0].join("selftest");
            assert_eq!(fs::read_to_string(&selftest).unwrap(), "not run\n");
            fs::write(&selftest, "1").unwrap();
            assert_eq!(fs::read_to_string(&selftest).unwrap(), "ok\n");
            let _ = ROOT;
            println!("OK");
        }
    }

    impl Test for TestExtentGuestOtherVersion {
        fn in_guest(self: Box<Self>) {
            let bound: Vec<_> = fs::read_dir(DRIVER)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.starts_with("virtio"))
                .collect();
            assert_eq!(bound, Vec::<String>::new(), "the driver took a device of another version");
            let (device, driver) = OTHER_VERSION;
            let refused = format!(
                "the device is of protocol version {device}, this driver of {driver}: refused, as \
                 a driver of another build is"
            );
            let records = crate::kernel_log::records();
            let said: Vec<_> = records.iter().filter(|record| record.contains(&refused)).collect();
            assert_eq!(said.len(), 1, "the kernel's log: {records:#?}");
            println!("OK");
        }
    }
}
