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

    let mut pager = UserPager::new();
    assert!(pager.init());

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
