// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of physical memory's size: configured before its first use, or the default.

use crate::{
    ArenaId, CHUNK_SIZE, ConfigError, DEFAULT_MEMORY_BYTES, Wait, alloc, configure,
    count_free_pages, count_total_bytes, memory_bytes, page_size,
};

const CHUNK: u64 = CHUNK_SIZE as u64;

/// Unconfigured, physical memory is a GiB, which reading it fixes.
#[test]
fn memory_is_a_gib_unless_configured() {
    assert_eq!(memory_bytes(), DEFAULT_MEMORY_BYTES);
    assert_eq!(DEFAULT_MEMORY_BYTES, 1 << 30);
    assert_eq!(configure(16 * CHUNK), Err(ConfigError::InUse));
    assert_eq!(count_total_bytes(), DEFAULT_MEMORY_BYTES);
    assert_eq!(count_free_pages(), DEFAULT_MEMORY_BYTES / page_size());
}

/// Configured, physical memory is the size given, which is configured once.
#[test]
fn configured_memory_is_its_size() {
    configure(16 * CHUNK).unwrap();
    assert_eq!(configure(32 * CHUNK), Err(ConfigError::InUse));
    assert_eq!(memory_bytes(), 16 * CHUNK);
    assert_eq!(count_total_bytes(), 16 * CHUNK);
    assert_eq!(count_free_pages(), 16 * CHUNK / page_size());
}

/// Physical memory in use is configured no more.
#[test]
fn memory_in_use_is_not_configured() {
    let page = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    assert_eq!(configure(16 * CHUNK), Err(ConfigError::InUse));
    drop(page);
}

/// A size of no memory, or not of whole chunks, is refused, and leaves physical memory to be
/// configured.
#[test]
fn sizes_not_of_whole_chunks_are_invalid() {
    assert_eq!(configure(0), Err(ConfigError::Invalid("no memory".into())));
    let size = CHUNK + page_size();
    let invalid = configure(size).unwrap_err();
    let why = format!("{size} bytes, not whole chunks of {CHUNK}");
    assert_eq!(invalid, ConfigError::Invalid(why.clone()));
    assert_eq!(invalid.to_string(), format!("invalid physical memory size: {why}"));
    configure(2 * CHUNK).unwrap();
    assert_eq!(count_total_bytes(), 2 * CHUNK);
}

/// TASK-79.37: Linux maps no more chunks than `vm.max_map_count` lets a process: physical memory
/// is no larger.
#[cfg(target_os = "linux")]
#[test]
fn sizes_past_what_the_host_maps_are_invalid() {
    let max = crate::max_size().expect("Linux limits a process's mappings");
    let max_map_count = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap();
    let max_map_count: u64 = max_map_count.trim().parse().unwrap();
    assert_eq!(max, (max_map_count - 4096) * CHUNK);
    let why =
        format!("{} bytes, more than the host maps, {max}: raise vm.max_map_count", max + CHUNK);
    assert_eq!(configure(max + CHUNK), Err(ConfigError::Invalid(why)));
    configure(16 * CHUNK).unwrap();
}

/// macOS limits no process's mappings.
#[cfg(target_os = "macos")]
#[test]
fn macos_maps_any_size() {
    assert_eq!(crate::max_size(), None);
}

/// Pages are the host's.
#[test]
fn pages_are_the_hosts() {
    // SAFETY: sysconf has no preconditions.
    let host = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    assert_eq!(page_size(), host as u64);
    assert!(CHUNK.is_multiple_of(page_size()));
}
