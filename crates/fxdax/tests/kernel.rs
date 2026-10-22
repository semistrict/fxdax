// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of the kernel's configuration, which works as nextest runs each test in its own process,
//! whose kernel has not started.

use futures::executor::block_on;
use fxdax::{Errno, Filesystem, Storage, kernel};
use zircon::boot_options::BootOptions;

const MIB: u64 = 1024 * 1024;

#[test]
fn configure_before_the_kernel_starts() {
    let config = kernel::Config {
        memory_bytes: 256 * MIB,
        warning_threshold_bytes: 64 * MIB,
        critical_threshold_bytes: 32 * MIB + 1,
        out_of_memory_threshold_bytes: 8 * MIB,
    };
    kernel::configure(config).expect("configure failed");
    let options = BootOptions::get();
    assert_eq!(options.memory_bytes, 256 * MIB);
    assert_eq!(
        (
            options.oom_warning_threshold_mb,
            options.oom_critical_threshold_mb,
            options.oom_out_of_memory_threshold_mb
        ),
        (64, 32, 8)
    );
    block_on(async { Filesystem::format(Storage::memory(8 * MIB)).await?.close().await }).unwrap();
}

#[test]
fn configure_after_the_kernel_starts_fails() {
    block_on(async { Filesystem::format(Storage::memory(8 * MIB)).await?.close().await }).unwrap();
    let error = kernel::configure(kernel::Config::default()).expect_err("configured");
    assert_eq!(error.to_string(), "the kernel has already been configured or started");
    assert_eq!(error.errno(), Some(Errno::EBUSY));
}

#[test]
fn memory_scales_the_default_thresholds() {
    assert_eq!(kernel::Config::with_memory(1024 * MIB), kernel::Config::default());
    assert_eq!(
        kernel::Config::with_memory(4096 * MIB),
        kernel::Config {
            memory_bytes: 4096 * MIB,
            warning_threshold_bytes: 1200 * MIB,
            critical_threshold_bytes: 600 * MIB,
            out_of_memory_threshold_bytes: 200 * MIB,
        }
    );
    kernel::configure(kernel::Config::with_memory(4096 * MIB)).expect("configure failed");
    assert_eq!(BootOptions::get().memory_bytes, 4096 * MIB);
    // Once.
    let error = kernel::configure(kernel::Config::default()).expect_err("configured");
    assert_eq!(error.errno(), Some(Errno::EBUSY));
}

#[test]
fn too_little_memory_for_distinct_thresholds_fails() {
    // 300, 150 and 50 MiB a GiB of 4 MiB are all 0 MiB.
    let error = kernel::configure(kernel::Config::with_memory(4 * MIB)).expect_err("configured");
    assert_eq!(error.errno(), Some(Errno::EINVAL));
}

#[test]
fn thresholds_must_decrease_below_the_memory() {
    let default = kernel::Config::default();
    assert_eq!(default.memory_bytes, 1024 * MIB);
    for config in [
        kernel::Config { critical_threshold_bytes: default.warning_threshold_bytes, ..default },
        kernel::Config { out_of_memory_threshold_bytes: 200 * MIB, ..default },
        kernel::Config { memory_bytes: default.warning_threshold_bytes, ..default },
    ] {
        let error = kernel::configure(config).expect_err("configured");
        assert_eq!(error.to_string(), "memory thresholds must decrease, and be below the memory");
    }
    kernel::configure(default).expect("configure failed");
}

/// TASK-79.37: memory of more chunks (2 MiB) than the host maps fails, a mapping each at most, as
/// the process may make `vm.max_map_count` of them, 4096 kept for its others.
#[cfg(target_os = "linux")]
#[test]
fn more_memory_than_the_host_maps_fails() {
    let max_map_count = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap();
    let chunks = max_map_count.trim().parse::<u64>().unwrap() - 4096;
    let error = kernel::configure(kernel::Config::with_memory((chunks + 1) * 2 * MIB))
        .expect_err("configured");
    assert_eq!(error.to_string(), "the memory is more than the host maps: raise vm.max_map_count");
    assert_eq!(error.errno(), Some(Errno::EINVAL));
    kernel::configure(kernel::Config::with_memory(chunks * 2 * MIB)).expect("configure failed");
}
