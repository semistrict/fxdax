//! Scanning of page state across the system.
//!
//! Ported from `zircon/kernel/vm/scanner.cc`: the accessed scan, which harvests the accessed bits
//! of mapped pages into the page queues' age information before they age.
//!
//! Deviations:
//! - Address spaces are not yet ported, so an accessed scan has no mappings to harvest and only
//!   records that it happened.
//! - There is no scanner thread: the zero page scanner, page table reclamation and periodic
//!   accessed scans are not ported, so `scanner_init` only starts the page queues.

use crate::boot_options::BootOptions;
use crate::kernel::deadline::current_mono_time;
use crate::vm::pmm::PmmNode;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use zx_types::{ZX_TIME_INFINITE_PAST, zx_instant_mono_t};

static LAST_ACCESSED_SCAN_COMPLETE: AtomicI64 = AtomicI64::new(ZX_TIME_INFINITE_PAST);
static ACCESSED_SCANNER_LOCK: Mutex<()> = Mutex::new(());

/// Returns whether an accessed scan is needed for accessed information to be as recent as
/// `update_time`.
pub fn needs_accessed_scan(update_time: zx_instant_mono_t) -> bool {
    update_time > LAST_ACCESSED_SCAN_COMPLETE.load(Ordering::Acquire)
}

/// Ensures an accessed scan has completed since `update_time`, performing one if needed.
///
/// Currently accessed scanning happens completely inline, and so this does one of three things
/// 1. Returns immediately if a sufficiently recent scan already happened
/// 2. Waits for an in progress scan to finish, and then most likely returns unless update_time is
///    ZX_TIME_INFINITE
/// 3. Performs an entire scan and then returns.
pub fn wait_for_accessed_scan(update_time: zx_instant_mono_t) {
    if !needs_accessed_scan(update_time) {
        // scanning is sufficiently up to date.
        return;
    }
    let _guard = ACCESSED_SCANNER_LOCK.lock();
    // Re-check now that we hold the lock in case a scan just finished and we were blocked on it.
    if !needs_accessed_scan(update_time) {
        return;
    }
    LAST_ACCESSED_SCAN_COMPLETE.store(current_mono_time(), Ordering::Release);
}

/// Starts the page queues aging with the configured intervals.
pub fn scanner_init(pmm: &'static PmmNode) {
    let options = BootOptions::get();
    let page_queues = pmm.page_queues();
    page_queues.set_active_ratio_multiplier(options.page_scanner_active_ratio_multiplier);
    page_queues.start_threads(
        i64::from(options.page_scanner_min_aging_interval_ms) * 1_000_000,
        i64::from(options.page_scanner_max_aging_interval_ms) * 1_000_000,
    );
}
