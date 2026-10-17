// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that the kernel evicts pager-backed pages under memory pressure, so that more
//! pager-backed memory than the PMM has can be used.
//!
//! Each test configures a small PMM with `BootOptions` before the kernel first runs, which
//! works as nextest runs each test in its own process.

use zircon::boot_options::{BootOptions, OomBehavior};
use zircon::vm::evictor::Evictor;
use zircon_core_tests::userpager::*;

const MB: u64 = 1024 * 1024;

/// Configures a kernel with `memory_mb` of memory, whose pressure levels are scaled down to
/// match.
fn boot(memory_mb: u64) {
    let options = BootOptions {
        memory_bytes: memory_mb * MB,
        oom_out_of_memory_threshold_mb: 8,
        oom_imminent_oom_delta_mb: 2,
        oom_critical_threshold_mb: 16,
        oom_warning_threshold_mb: 24,
        oom_debounce_mb: 1,
        oom_eviction_delay_ms: 0,
        oom_hysteresis_seconds: 1,
        // Running out of memory fails the test rather than aborting the test process.
        oom_behavior: OomBehavior::JobKill,
        ..BootOptions::default()
    };
    assert!(BootOptions::set(options).is_ok(), "the kernel has already started");
}

fn pages_evicted() -> u64 {
    let stats = Evictor::get_global_stats();
    stats.pager_backed_oom + stats.pager_backed_other
}

// Streams through a pager-backed VMO twice the size of memory, twice, checking its contents and
// hinting that each chunk is no longer needed once read, as a streaming reader would. Pages read
// earlier must be evicted to make room for later ones, and supplied again when read again.
#[test]
fn stream_more_than_memory() {
    const MEMORY_MB: u64 = 64;
    boot(MEMORY_MB);

    let mut pager = UserPager::initialized();

    let num_pages = 2 * MEMORY_MB * MB / page_size();
    let vmo = pager.create_vmo(num_pages).unwrap();
    assert!(pager.start_tagged_page_fault_handler());

    let chunk_pages = MB / page_size();
    for pass in 0..2 {
        for offset in (0..num_pages).step_by(chunk_pages as usize) {
            assert!(vmo.check_vmo(offset, chunk_pages, None), "pass {pass} page {offset}");
            let range = (offset * page_size(), chunk_pages * page_size());
            vmo.vmo().op_range(zx::VmoOp::DONT_NEED, range.0, range.1).unwrap();
        }
    }

    // At least the pages beyond the memory the PMM has must have been evicted, on each pass.
    assert!(pages_evicted() >= num_pages, "evicted {} of {num_pages} pages", pages_evicted());
}

/// The memory pressure event of `kind`, which the kernel signals while memory is at its level.
fn pressure_event(kind: zx::SystemEventKind) -> zx::Event {
    // There are no jobs: the kernel doesn't check the root job.
    let root_job = zx::Job::from(zx::NullableHandle::invalid());
    zx::system_get_event(&root_job, kind).unwrap()
}

fn is_signaled(event: &zx::Event, deadline: zx::MonotonicInstant) -> bool {
    event.wait_one(zx::Signals::EVENT_SIGNALED, deadline).to_result().is_ok()
}

// Commits memory until the kernel signals warning pressure, then frees it and waits for normal
// pressure again, through the events zx_system_get_event gives.
#[test]
fn pressure_events_follow_free_memory() {
    const MEMORY_MB: u64 = 64;
    boot(MEMORY_MB);

    let normal = pressure_event(zx::SystemEventKind::MemoryPressureNormal);
    let warning = pressure_event(zx::SystemEventKind::MemoryPressureWarning);
    let info = normal.basic_info().unwrap();
    assert_eq!(
        info.rights,
        zx::Rights::DUPLICATE | zx::Rights::TRANSFER | zx::Rights::WAIT,
        "events are given without the right to signal them"
    );
    // The watchdog's worker signals the level memory starts at.
    assert!(is_signaled(
        &normal,
        zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(10))
    ));
    assert!(!is_signaled(&warning, zx::MonotonicInstant::INFINITE_PAST));

    let mut vmos = Vec::new();
    while !is_signaled(
        &warning,
        zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(20)),
    ) {
        assert!((vmos.len() as u64) < MEMORY_MB, "no warning after committing all of memory");
        let vmo = zx::Vmo::create(MB).unwrap();
        vmo.op_range(zx::VmoOp::COMMIT, 0, MB).unwrap();
        vmos.push(vmo);
    }
    assert!(!is_signaled(&normal, zx::MonotonicInstant::INFINITE_PAST));

    vmos.clear();
    assert!(is_signaled(
        &normal,
        zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(10))
    ));
    assert!(!is_signaled(&warning, zx::MonotonicInstant::INFINITE_PAST));
}

#[test]
fn system_get_event_rejects_unknown_kinds() {
    let mut event = zx::sys::ZX_HANDLE_INVALID;
    // SAFETY: `event` is a valid out-pointer.
    let status = unsafe { zx::sys::zx_system_get_event(zx::sys::ZX_HANDLE_INVALID, 0, &mut event) };
    assert_eq!(zx::Status::ok(status), Err(zx::Status::INVALID_ARGS));
}
