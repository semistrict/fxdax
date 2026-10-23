// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of memory that is never out (`OomBehavior::Evict`, TASK-50.3), which Zircon lacks: an
//! allocation that finds no free page at all, with no relief, makes room by evicting clean
//! pager-backed pages on demand, whatever their age, as nothing else frees memory for it; or
//! fails at once, with none to evict, or eviction off (TASK-79.70).
//!
//! Each test configures a small kernel before it first runs, which works as nextest runs each
//! test in its own process. Time ages no page: the scanner, which drives aging, is disabled, and
//! with it eviction, which the tests that evict turn on again; and the pages supplied are the
//! newest, too new for eviction but on demand.

use super::test_helper::make_committed_pager_vmo;
use crate::boot_options::{BootOptions, OomBehavior};
use crate::kernel::deadline::Deadline;
use crate::vm::evictor::{EvictionLevel, Output, TriggerReason};
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::physmap::CHUNK_SIZE;
use crate::vm::pmm::{self, ALLOC_FLAG_CAN_WAIT};
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_object_paged::VmObjectPaged;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: u64 = page::SIZE as u64;
const CHUNK_PAGES: usize = CHUNK_SIZE / page::SIZE;

/// Boots a kernel of 16 chunks of memory, which is never out, as by default.
fn boot() {
    let options = BootOptions { memory_bytes: (16 * CHUNK_SIZE) as u64, ..Default::default() };
    assert_eq!(options.oom_behavior, OomBehavior::Evict);
    BootOptions::set(options).expect("the kernel has not started");
}

/// Takes every free page with a pager-backed VMO's clean pages, the newest, and returns the VMO.
fn fill_with_clean_pages() -> Arc<VmObjectPaged> {
    let node = pmm::node();
    let free = node.count_free_pages() as usize;
    let (vmo, _) = make_committed_pager_vmo(free, false, false).unwrap();
    assert_eq!(node.count_free_pages(), 0);
    vmo
}

/// Takes every free page with an anonymous VMO's pages, which eviction cannot free, and returns
/// the VMO.
fn fill_with_anonymous_pages() -> Arc<VmObjectPaged> {
    let node = pmm::node();
    let len = node.count_free_pages() * PAGE_SIZE;
    let vmo = VmObjectPaged::create(0, 0, len).unwrap();
    vmo.commit_range(0, len).unwrap();
    assert_eq!(node.count_free_pages(), 0);
    vmo
}

/// Waits for a page of the default arena, as an anonymous page request does, trying again as
/// told to.
fn wait_for_page() -> Result<VmPagePtr, Status> {
    let deadline = Deadline::after_mono(10_000_000_000);
    loop {
        match pmm::node().wait_for_single_page_allocation(deadline) {
            Err(Status::SHOULD_WAIT) => continue,
            result => break result,
        }
    }
}

/// Eviction on demand takes the pages too new for the other levels, the oldest first, as many as
/// it is asked for; the other levels take none of them.
#[test]
fn eviction_on_demand_takes_pages_too_new_for_other_levels() {
    boot();
    let _scanner = AutoVmScannerDisable::new();
    let (vmo, _) = make_committed_pager_vmo(2 * CHUNK_PAGES, false, false).unwrap();
    let evictor = pmm::node().evictor();
    evictor.enable_eviction(false);

    let evict = |level| {
        let result = evictor.evict_synchronous(
            CHUNK_SIZE as u64,
            0,
            level,
            Output::NoPrint,
            TriggerReason::Other,
        );
        result.counts.pager_backed
    };
    assert_eq!(evict(EvictionLevel::OnlyOldest), 0);
    assert_eq!(evict(EvictionLevel::IncludeNewest), 0);
    assert_eq!(evict(EvictionLevel::OnDemand), CHUNK_PAGES as u64);
    let resident: Vec<bool> =
        (0..2 * CHUNK_PAGES as u64).map(|i| vmo.debug_get_page(i * PAGE_SIZE).is_some()).collect();
    assert_eq!(resident, [[false; CHUNK_PAGES], [true; CHUNK_PAGES]].concat());
}

/// With every free page taken by a pager-backed VMO's clean pages, an allocation that can wait
/// waits while eviction frees some of them, those supplied first, and then takes one. It failed
/// at once, asking for no eviction, and Fxfs's writes, which ask for pages to dirty again as the
/// kernel fails them, never finished.
#[test]
fn allocations_that_find_memory_out_evict_clean_pages_of_any_age() {
    boot();
    let node = pmm::node();
    let _scanner = AutoVmScannerDisable::new();
    let vmo = fill_with_clean_pages();
    node.evictor().enable_eviction(false);

    assert_eq!(node.alloc_page(ALLOC_FLAG_CAN_WAIT).err(), Some(Status::SHOULD_WAIT));
    let page = wait_for_page().expect("a page eviction freed");
    assert!(vmo.debug_get_page(0).is_none(), "the oldest page was not evicted first");
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
}

/// With relief, as the extent daemon's, whose evictor frees only what is its own, such as cached
/// extents, an allocation that finds every free page taken by the kernel's clean pages is relieved
/// by evicting those too, on demand. A pass that freed none of its evictor's own exhausted the
/// arena, as the kernel's pages stayed, and the import's writes asked again for ever.
#[test]
fn relief_evicts_the_kernels_clean_pages_on_demand_too() {
    boot();
    let node = pmm::node();
    let _scanner = AutoVmScannerDisable::new();
    let vmo = fill_with_clean_pages();
    node.evictor().enable_eviction(false);
    // An evictor with nothing of its own to free.
    pmm::relieve_with(|_, _| 0).unwrap();

    assert_eq!(node.alloc_page(ALLOC_FLAG_CAN_WAIT).err(), Some(Status::SHOULD_WAIT));
    let page = wait_for_page().expect("a page relief freed");
    assert!(vmo.debug_get_page(0).is_none(), "the oldest page was not evicted first");
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
}

/// With every free page taken by an anonymous VMO's pages, which eviction cannot free, an
/// allocation fails at once, whether or not it can wait, rather than wait for room nothing makes.
#[test]
fn allocations_that_find_memory_out_fail_with_nothing_to_evict() {
    boot();
    let node = pmm::node();
    let _vmo = fill_with_anonymous_pages();

    assert_eq!(node.alloc_page(0).err(), Some(Status::NO_MEMORY));
    assert_eq!(node.alloc_page(ALLOC_FLAG_CAN_WAIT).err(), Some(Status::NO_MEMORY));
}

/// With eviction off, as the scanner disabled turns it off, or `page_scanner_enable_eviction`
/// does, nothing evicts the clean pages that take every free page: an allocation fails at once,
/// whether or not it can wait, rather than wait for room nothing makes. One that could wait was
/// told to, and an anonymous page request waiting for it panicked once it had waited 30 s.
#[test]
fn allocations_that_find_memory_out_fail_with_eviction_off() {
    boot();
    let node = pmm::node();
    let _scanner = AutoVmScannerDisable::new();
    let _vmo = fill_with_clean_pages();
    assert!(!node.evictor().is_eviction_enabled());

    assert_eq!(node.alloc_page(0).err(), Some(Status::NO_MEMORY));
    assert_eq!(node.alloc_page(ALLOC_FLAG_CAN_WAIT).err(), Some(Status::NO_MEMORY));
}

/// An allocation of many pages that can wait, finding too few free, is told to wait at once,
/// taking none; and a VMO's commit, told so, takes its pages a page at a time, each waiting, as
/// one page's allocation does, while eviction frees clean pages for it.
#[test]
fn commits_that_find_memory_out_wait_for_each_page() {
    boot();
    let node = pmm::node();
    let _scanner = AutoVmScannerDisable::new();
    let vmo = fill_with_clean_pages();
    node.evictor().enable_eviction(false);

    let mut pages = VmPageDoublyLinkedList::new();
    assert_eq!(node.alloc_pages(2, ALLOC_FLAG_CAN_WAIT, &mut pages), Err(Status::SHOULD_WAIT));
    assert!(pages.is_empty());
    let len = 2 * CHUNK_SIZE as u64;
    let committed = VmObjectPaged::create(ALLOC_FLAG_CAN_WAIT, 0, len).unwrap();
    committed.commit_range(0, len).unwrap();
    assert!(vmo.debug_get_page(0).is_none(), "the oldest page was not evicted first");
}
