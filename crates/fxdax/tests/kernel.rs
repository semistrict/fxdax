//! Tests of the kernel's configuration, which works as nextest runs each test in its own process,
//! whose kernel has not started.

use futures::executor::block_on;
use fxdax::{Filesystem, Storage, kernel};
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
    assert_eq!(error.to_string(), "the kernel has already started");
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
