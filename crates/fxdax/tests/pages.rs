// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of physical memory's configuration, which works as nextest runs each test in its own
//! process, whose physical memory is not yet in use.

use fxdax::{Errno, pages};

const MIB: u64 = 1024 * 1024;

#[test]
fn configure_before_physical_memory_is_used() {
    pages::configure(pages::Config::with_memory(256 * MIB)).expect("configure failed");
    assert_eq!(physmem::memory_bytes(), 256 * MIB);
    drop(pages::Arena::new());
}

#[test]
fn configure_once_physical_memory_is_used_fails() {
    drop(pages::Arena::new());
    let error = pages::configure(pages::Config::default()).expect_err("configured");
    assert_eq!(error.to_string(), "physical memory has already been configured or used");
    assert_eq!(error.errno(), Some(Errno::EBUSY));
}

#[test]
fn configure_once() {
    assert_eq!(pages::Config::default(), pages::Config::with_memory(1024 * MIB));
    pages::configure(pages::Config::with_memory(4096 * MIB)).expect("configure failed");
    assert_eq!(physmem::memory_bytes(), 4096 * MIB);
    let error = pages::configure(pages::Config::default()).expect_err("configured");
    assert_eq!(error.errno(), Some(Errno::EBUSY));
}

#[test]
fn memory_of_part_of_a_chunk_fails() {
    let error = pages::configure(pages::Config::with_memory(3 * MIB)).expect_err("configured");
    assert_eq!(error.to_string(), "invalid physical memory size: 3145728 bytes, not whole chunks of 2097152");
    assert_eq!(error.errno(), Some(Errno::EINVAL));
}

/// TASK-79.37: memory of more chunks (2 MiB) than the host maps fails, a mapping each at most, as
/// the process may make `vm.max_map_count` of them, 4096 kept for its others.
#[cfg(target_os = "linux")]
#[test]
fn more_memory_than_the_host_maps_fails() {
    let max_map_count = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap();
    let chunks = max_map_count.trim().parse::<u64>().unwrap() - 4096;
    let (memory, max) = ((chunks + 1) * 2 * MIB, chunks * 2 * MIB);
    let error =
        pages::configure(pages::Config::with_memory(memory)).expect_err("configured");
    assert_eq!(
        error.to_string(),
        format!(
            "invalid physical memory size: {memory} bytes, more than the host maps, {max}: raise vm.max_map_count"
        )
    );
    assert_eq!(error.errno(), Some(Errno::EINVAL));
    pages::configure(pages::Config::with_memory(max)).expect("configure failed");
}
