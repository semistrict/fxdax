// Copyright 2019 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The page scanner, which harvests accessed information and drives aging and eviction.
//!
//! Ported from `zircon/kernel/vm/scanner.cc` and `zircon/kernel/vm/include/vm/scanner.h`.
//!
//! Deviations:
//! - Its thread starts through `crate::sync::spawn`, which with the `shuttle` feature starts
//!   none, as time doesn't pass under Shuttle.
//! - No zero page scanning, as de-duplicating zero pages is not ported.
//! - No page table reclamation, as the host's page tables are not the kernel's to reclaim, so the
//!   page table eviction policy and its operations are not ported.
//! - No compression or discardable VMOs.
//! - No console commands, so no requests to dump state or reclaim everything.
//! - The scanner thread is a host thread.

use crate::boot_options::BootOptions;
use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::{AutounsignalEvent, Event};
use crate::vm::arch_vm_aspace::{NonTerminalAction, TerminalAction};
use crate::vm::page_queues::DEFAULT_MIN_MRU_ROTATE_TIME;
use crate::vm::pmm::{self, PmmNode};
use crate::vm::vm_aspace::VmAspace;
use parking_lot::Mutex;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::thread::JoinHandle;
use zx_types::{ZX_TIME_INFINITE_PAST, zx_duration_mono_t, zx_instant_mono_t};

const SCANNER_OP_DISABLE: u32 = 1 << 1;
const SCANNER_OP_ENABLE: u32 = 1 << 2;
const SCANNER_OP_UPDATE_HARVEST_TIME: u32 = 1 << 6;

struct ScannerDisabled {
    disable_count: u32,
    /// The scanner thread, once started.
    thread: Option<JoinHandle<()>>,
}

struct Scanner {
    /// Tracks what the scanner should do when it is next woken up.
    operation: AtomicU32,
    /// Event to signal the scanner thread to wake up and perform work.
    request_event: AutounsignalEvent,
    /// Event that is signaled whenever the scanner is disabled. This is used to synchronize
    /// disable requests with the scanner thread.
    disabled_event: Event,
    disabled_lock: Mutex<ScannerDisabled>,
    /// Mutex used to ensure only a single access scan is happening at once.
    accessed_scanner_lock: Mutex<()>,
    last_accessed_scan_complete: AtomicI64,
    /// The accessed scan rate starts matched to the minimum aging period, since scanning more
    /// frequently than that does not produce any fidelity of information.
    accessed_scan_period: AtomicI64,
    reclaim_pt_next_accessed_scan: AtomicBool,
}

static SCANNER: LazyLock<Scanner> = LazyLock::new(|| Scanner {
    operation: AtomicU32::new(0),
    request_event: AutounsignalEvent::new(false),
    disabled_event: Event::new(false),
    disabled_lock: Mutex::new(ScannerDisabled { disable_count: 0, thread: None }),
    accessed_scanner_lock: Mutex::new(()),
    last_accessed_scan_complete: AtomicI64::new(ZX_TIME_INFINITE_PAST),
    accessed_scan_period: AtomicI64::new(DEFAULT_MIN_MRU_ROTATE_TIME),
    reclaim_pt_next_accessed_scan: AtomicBool::new(false),
});

fn zx_time_add_duration(
    time: zx_instant_mono_t,
    duration: zx_duration_mono_t,
) -> zx_instant_mono_t {
    time.saturating_add(duration)
}

fn scanner_request_thread() {
    let scanner = &*SCANNER;
    let node = pmm::node();
    let mut disabled = false;
    let mut next_harvest_deadline = zx_time_add_duration(
        current_mono_time(),
        scanner.accessed_scan_period.load(Ordering::Relaxed),
    );
    loop {
        let deadline =
            if disabled { Deadline::infinite() } else { Deadline::no_slack(next_harvest_deadline) };
        // Timing out is how the scanner wakes up to harvest.
        let _ = scanner.request_event.wait(&deadline);
        let mut op = scanner.operation.swap(0, Ordering::AcqRel);
        // It is possible for enable and disable to happen at the same time. This indicates the
        // disabled count went from 1->0->1 and so we want to remain disabled. We do this by
        // performing the enable step first. We know that the scenario of 0->1->0 is not possible
        // as the 0->1 part of that holds the mutex until complete.
        if op & SCANNER_OP_ENABLE != 0 {
            op &= !SCANNER_OP_ENABLE;
            node.page_queues().enable_aging();
            // Re-enable eviction if it was originally enabled.
            if BootOptions::get().page_scanner_enable_eviction {
                node.evictor().enable_eviction(false);
            }
            disabled = false;
        }
        if op & SCANNER_OP_DISABLE != 0 {
            op &= !SCANNER_OP_DISABLE;
            // Make sure no eviction is happening either.
            node.evictor().disable_eviction();
            disabled = true;
            node.page_queues().disable_aging();
            // Grab the harvester lock to wait for any in progress scans to complete.
            drop(scanner.accessed_scanner_lock.lock());
            scanner.disabled_event.signal();
        }
        if disabled {
            // put the remaining ops back and resume waiting.
            scanner.operation.fetch_or(op, Ordering::AcqRel);
            continue;
        }

        let current = current_mono_time();

        // Recalculate next_harvest_deadline, as scanning may have happened via a different means
        // while we were sleeping and we do not want to re-scan unless *no* scan happens for the
        // period.
        next_harvest_deadline = zx_time_add_duration(
            scanner.last_accessed_scan_complete.load(Ordering::Acquire),
            scanner.accessed_scan_period.load(Ordering::Relaxed),
        );
        if current >= next_harvest_deadline {
            scanner_wait_for_accessed_scan(next_harvest_deadline);
            op |= SCANNER_OP_UPDATE_HARVEST_TIME;
        }

        if op & SCANNER_OP_UPDATE_HARVEST_TIME != 0 {
            op &= !SCANNER_OP_UPDATE_HARVEST_TIME;
            next_harvest_deadline = zx_time_add_duration(
                scanner.last_accessed_scan_complete.load(Ordering::Acquire),
                scanner.accessed_scan_period.load(Ordering::Relaxed),
            );
        }
        debug_assert_eq!(op, 0);
    }
}

/// Returns whether an accessed scan is needed for accessed information to be as recent as
/// `update_time`.
pub fn scanner_needs_accessed_scan(update_time: zx_instant_mono_t) -> bool {
    update_time > SCANNER.last_accessed_scan_complete.load(Ordering::Acquire)
}

/// Ensures an accessed scan has completed since `update_time`, performing one if needed.
///
/// Currently accessed scanning happens completely inline, and so this does one of three things
/// 1. Returns immediately if a sufficiently recent scan already happened
/// 2. Waits for an in progress scan to finish, and then most likely returns unless update_time is
///    ZX_TIME_INFINITE
/// 3. Performs an entire scan and then returns.
///
/// The public definition of this method is abstract to allow for this to, in the future, not
/// necessarily perform a scan itself, but sync up with the scanner thread that might be slowly
/// scanning in the background.
pub fn scanner_wait_for_accessed_scan(update_time: zx_instant_mono_t) {
    let scanner = &*SCANNER;
    if !scanner_needs_accessed_scan(update_time) {
        // scanning is sufficiently up to date.
        return;
    }
    let _guard = scanner.accessed_scanner_lock.lock();
    // Re-check now that we hold the lock in case a scan just finished and we were blocked on it.
    if !scanner_needs_accessed_scan(update_time) {
        return;
    }
    let reclaim_pt = scanner.reclaim_pt_next_accessed_scan.swap(false, Ordering::AcqRel);
    // Perform a scan.
    // If we neither have page eviction or page table eviction then we can skip harvesting
    // accessed bits.
    if reclaim_pt || pmm::node().evictor().is_eviction_enabled() {
        let action =
            if reclaim_pt { NonTerminalAction::FreeUnaccessed } else { NonTerminalAction::Retain };
        VmAspace::harvest_all_user_accessed_bits(action, TerminalAction::UpdateAgeAndHarvest);
    }
    scanner.last_accessed_scan_complete.store(current_mono_time(), Ordering::Release);
}

/// Disables the scanner, waiting until it is disabled. Each call must be matched by a call to
/// `scanner_pop_disable_count`.
pub fn scanner_push_disable_count() {
    let scanner = &*SCANNER;
    let mut disabled = scanner.disabled_lock.lock();
    // Make sure we have a scanner thread, otherwise the wait may block forever.
    debug_assert!(disabled.thread.is_some());
    if disabled.disable_count == 0 {
        scanner.operation.fetch_or(SCANNER_OP_DISABLE, Ordering::AcqRel);
        scanner.request_event.signal();
    }
    disabled.disable_count += 1;
    let status = scanner.disabled_event.wait(&Deadline::infinite());
    debug_assert!(status.is_ok());
}

pub fn scanner_pop_disable_count() {
    let scanner = &*SCANNER;
    let mut disabled = scanner.disabled_lock.lock();
    debug_assert!(disabled.thread.is_some());
    debug_assert!(disabled.disable_count > 0);
    disabled.disable_count -= 1;
    if disabled.disable_count == 0 {
        scanner.operation.fetch_or(SCANNER_OP_ENABLE, Ordering::AcqRel);
        scanner.request_event.signal();
        scanner.disabled_event.unsignal();
    }
}

/// Disables the scanner for as long as it is alive.
pub struct AutoVmScannerDisable(());

impl AutoVmScannerDisable {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // The scanner is started with the PMM.
        pmm::node();
        scanner_push_disable_count();
        Self(())
    }
}

impl Drop for AutoVmScannerDisable {
    fn drop(&mut self) {
        scanner_pop_disable_count();
    }
}

/// Starts the scanner, and the page queues aging and eviction it drives.
pub fn scanner_init(pmm: &'static PmmNode) {
    let scanner = &*SCANNER;
    let options = BootOptions::get();
    let thread = crate::sync::spawn("scanner-request-thread", scanner_request_thread);
    if !options.page_scanner_start_at_boot {
        let mut disabled = scanner.disabled_lock.lock();
        disabled.disable_count += 1;
        scanner.operation.fetch_or(SCANNER_OP_DISABLE, Ordering::AcqRel);
        scanner.request_event.signal();
    }

    if options.page_scanner_enable_eviction {
        pmm.evictor().enable_eviction(false);
    }

    let page_queues = pmm.page_queues();
    page_queues.set_active_ratio_multiplier(options.page_scanner_active_ratio_multiplier);
    page_queues.start_threads(
        i64::from(options.page_scanner_min_aging_interval_ms) * 1_000_000,
        i64::from(options.page_scanner_max_aging_interval_ms) * 1_000_000,
    );
    // Set the access scan to at least 1 second over the min page scanning interval. This both
    // ensures that the access scan period is never 0 and that redundant scanning before the page
    // queues can age does not occur.
    let accessed_scan_period_ms = (options.page_scanner_min_aging_interval_ms + 1000)
        .max(options.page_scanner_accessed_scan_interval_ms);
    scanner
        .accessed_scan_period
        .store(i64::from(accessed_scan_period_ms) * 1_000_000, Ordering::Relaxed);

    page_queues.set_lru_action(options.lru_action);

    let mut disabled = scanner.disabled_lock.lock();
    debug_assert!(disabled.thread.is_none());
    disabled.thread = thread;
}
