// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/vm/unittests/evictor_unittest.cc`.
//!
//! Not ported: `evictor_discardable_test`, as there are no discardable VMOs.

use crate::vm::evictor::{EvictedPageCounts, EvictionLevel, EvictionTarget, Evictor};
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_cow_pages::{ReclaimType, VmCowReclaimSuccess};
use parking_lot::Mutex;
use std::sync::Arc;

struct TestState {
    free_pages: u64,
    total_evictions: u64,
    max_evictions: u64,
}

/// Custom pmm node to link with the evictor under test. Facilitates verifying the free count
/// which is not possible with the global pmm node.
struct TestPmmNode {
    state: Arc<Mutex<TestState>>,
    evictor: Arc<Evictor>,
}

impl TestPmmNode {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(TestState {
            free_pages: 0,
            total_evictions: 0,
            max_evictions: u64::MAX,
        }));
        let reclaim_state = state.clone();
        let free_pages_state = state.clone();
        let evictor = Arc::new(Evictor::with_functions(
            Some(Box::new(move |_eviction_level| {
                let mut state = reclaim_state.lock();
                if state.total_evictions >= state.max_evictions {
                    return None;
                }
                state.free_pages += 1;
                state.total_evictions += 1;
                Some(Ok(VmCowReclaimSuccess { r#type: ReclaimType::Evict, num_pages: 1 }))
            })),
            Some(Box::new(move || free_pages_state.lock().free_pages)),
        ));
        evictor.enable_eviction(true);
        Self { state, evictor }
    }

    fn get_eviction_target(&self) -> EvictionTarget {
        self.evictor.debug_get_eviction_target()
    }

    fn combine_eviction_target(&self, target: EvictionTarget) {
        self.evictor.combine_eviction_target(target);
    }

    fn evict_from_preloaded_target(&self) -> EvictedPageCounts {
        self.evictor.evict_from_preloaded_target()
    }

    fn free_pages(&self) -> u64 {
        self.state.lock().free_pages
    }

    fn evictor(&self) -> &Evictor {
        &self.evictor
    }

    fn cap_evictions(&self, max: u64) {
        self.state.lock().max_evictions = max;
    }

    fn uncap_evictions(&self) {
        self.state.lock().max_evictions = u64::MAX;
    }
}

impl Drop for TestPmmNode {
    fn drop(&mut self) {
        self.evictor.disable_eviction();
    }
}

fn rand() -> u64 {
    // SAFETY: Always safe to call.
    unsafe { libc::rand() as u64 }
}

fn rand_level() -> EvictionLevel {
    if rand() % 2 != 0 { EvictionLevel::IncludeNewest } else { EvictionLevel::OnlyOldest }
}

// Test that a one shot eviction target can be set as expected.
#[test]
fn evictor_set_target_test() {
    let _scanner_disable = AutoVmScannerDisable::new();
    let node = TestPmmNode::new();

    let expected = EvictionTarget {
        pending: rand() % 2 != 0,
        free_pages_target: rand(),
        min_pages_to_free: rand(),
        level: rand_level(),
        ..EvictionTarget::default()
    };

    node.combine_eviction_target(expected);

    let actual = node.get_eviction_target();

    assert_eq!(actual.pending, expected.pending);
    assert_eq!(actual.free_pages_target, expected.free_pages_target);
    assert_eq!(actual.min_pages_to_free, expected.min_pages_to_free);
    assert_eq!(actual.level, expected.level);
}

// Test that multiple one shot eviction targets can be combined as expected.
#[test]
fn evictor_combine_targets_test() {
    let _scanner_disable = AutoVmScannerDisable::new();
    let node = TestPmmNode::new();

    const NUM_TARGETS: usize = 5;
    let targets: [EvictionTarget; NUM_TARGETS] = std::array::from_fn(|_| EvictionTarget {
        pending: true,
        free_pages_target: rand() % 1000,
        min_pages_to_free: rand() % 1000,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    });
    for target in targets {
        node.combine_eviction_target(target);
    }

    let mut expected = EvictionTarget::default();
    for target in targets {
        expected.pending = expected.pending || target.pending;
        expected.level = expected.level.max(target.level);
        expected.min_pages_to_free += target.min_pages_to_free;
        expected.free_pages_target = expected.free_pages_target.max(target.free_pages_target);
    }

    let actual = node.get_eviction_target();

    assert_eq!(actual.pending, expected.pending);
    assert_eq!(actual.free_pages_target, expected.free_pages_target);
    assert_eq!(actual.min_pages_to_free, expected.min_pages_to_free);
    assert_eq!(actual.level, expected.level);
}

// Test that the evictor can evict from pager backed vmos as expected.
#[test]
fn evictor_pager_backed_test() {
    let _scanner_disable = AutoVmScannerDisable::new();
    let node = TestPmmNode::new();

    let mut target = EvictionTarget {
        pending: true,
        free_pages_target: 20,
        min_pages_to_free: 10,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    };

    // The node starts off with zero pages.
    let mut free_count = node.free_pages();
    assert_eq!(free_count, 0);

    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Free pages target was greater than min pages target. So precisely free pages target must
    // have been evicted.
    assert_eq!(counts.pager_backed, target.free_pages_target);
    assert!(counts.pager_backed >= target.min_pages_to_free);
    // The node has the desired number of free pages now, and a minimum of min pages have been
    // freed.
    free_count = node.free_pages();
    assert_eq!(free_count, target.free_pages_target);
    assert!(free_count >= target.min_pages_to_free);

    target = EvictionTarget {
        pending: true,
        free_pages_target: 10,
        min_pages_to_free: 20,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    };

    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Min pages target was greater than free pages target. So precisely min pages target must
    // have been evicted.
    assert_eq!(counts.pager_backed, target.min_pages_to_free);
    // The node has the desired number of free pages now, and a minimum of min pages have been
    // freed.
    assert!(node.free_pages() >= target.free_pages_target);
    assert_eq!(node.free_pages(), free_count + target.min_pages_to_free);
}

// Test that eviction meets the required free and min target as expected.
#[test]
fn evictor_free_target_test() {
    let _scanner_disable = AutoVmScannerDisable::new();

    // Only evict from pager backed vmos.
    let node = TestPmmNode::new();

    let mut target = EvictionTarget {
        pending: true,
        free_pages_target: 20,
        min_pages_to_free: 0,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    };

    // The node starts off with zero pages.
    let mut free_count = node.free_pages();
    assert_eq!(free_count, 0);

    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Free pages target was greater than min pages target. So precisely free pages target must
    // have been evicted.
    assert_eq!(counts.pager_backed, target.free_pages_target);
    // The node has the desired number of free pages now, and a minimum of min pages have been
    // freed.
    free_count = node.free_pages();
    assert_eq!(free_count, target.free_pages_target);
    assert!(free_count >= target.min_pages_to_free);

    // Evict again with the same target.
    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // No new pages should have been evicted, as the free target was already met with the previous
    // round of eviction, and no minimum pages were requested to be evicted.
    assert_eq!(counts.pager_backed, 0);
    assert_eq!(node.free_pages(), free_count);

    // Evict again with a higher free memory target. No min pages target.
    let delta_pages = 10;
    target.free_pages_target += delta_pages;
    target.min_pages_to_free = 0;
    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Exactly delta_pages evicted.
    assert_eq!(counts.pager_backed, delta_pages);
    assert!(counts.pager_backed >= target.min_pages_to_free);
    // Free count increased by delta_pages.
    free_count = node.free_pages();
    assert_eq!(free_count, target.free_pages_target);

    // Evict again with a higher free memory target and also a min pages target.
    target.free_pages_target += delta_pages;
    target.min_pages_to_free = delta_pages;
    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Exactly delta_pages evicted.
    assert_eq!(counts.pager_backed, delta_pages);
    assert!(counts.pager_backed >= target.min_pages_to_free);
    // Free count increased by delta_pages.
    free_count = node.free_pages();
    assert_eq!(free_count, target.free_pages_target);

    // Evict again with the same free target, but request a min number of pages to be freed.
    target.min_pages_to_free = 2;
    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Exactly min pages evicted.
    assert_eq!(counts.pager_backed, target.min_pages_to_free);
    // Free count increased by min pages.
    assert_eq!(node.free_pages(), free_count + target.min_pages_to_free);
}

// Test that eviction using an external target does not alter a previously set eviction target.
#[test]
fn evictor_external_target_test() {
    let _scanner_disable = AutoVmScannerDisable::new();
    let node = TestPmmNode::new();

    let expected = EvictionTarget {
        pending: rand() % 2 != 0,
        free_pages_target: 111,
        min_pages_to_free: 33,
        level: rand_level(),
        ..EvictionTarget::default()
    };

    node.combine_eviction_target(expected);

    let external = EvictionTarget {
        pending: !expected.pending,
        free_pages_target: 99,
        min_pages_to_free: 22,
        level: if expected.level == EvictionLevel::OnlyOldest {
            EvictionLevel::IncludeNewest
        } else {
            EvictionLevel::OnlyOldest
        },
        ..EvictionTarget::default()
    };
    node.evictor().evict_from_external_target(external);

    let actual = node.get_eviction_target();

    assert_eq!(actual.pending, expected.pending);
    assert_eq!(actual.free_pages_target, expected.free_pages_target);
    assert_eq!(actual.min_pages_to_free, expected.min_pages_to_free);
    assert_eq!(actual.level, expected.level);
}

// Test that a one shot eviction target can be set as expected.
#[test]
fn evictor_min_target_carried_over_test() {
    let _scanner_disable = AutoVmScannerDisable::new();
    let node = TestPmmNode::new();

    let target = EvictionTarget {
        pending: true,
        free_pages_target: 10,
        min_pages_to_free: 15,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    };

    // The node starts off with zero pages.
    let free_count = node.free_pages();
    assert_eq!(free_count, 0);

    // Cap the number of evictions to 5.
    node.cap_evictions(5);

    node.combine_eviction_target(target);
    let counts = node.evict_from_preloaded_target();

    // Exactly 5 pages evicted.
    assert_eq!(counts.pager_backed, 5);

    // Uncap evictions.
    node.uncap_evictions();

    // Combine target with zero min pages requested.
    node.combine_eviction_target(EvictionTarget {
        pending: true,
        free_pages_target: 0,
        min_pages_to_free: 0,
        level: EvictionLevel::IncludeNewest,
        ..EvictionTarget::default()
    });
    let counts = node.evict_from_preloaded_target();

    // Remaining pages should have been evicted.
    assert_eq!(counts.pager_backed, target.min_pages_to_free - 5);

    let free_count = node.free_pages();
    assert_eq!(free_count, target.min_pages_to_free);
}
