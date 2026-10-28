// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ported from `zircon/kernel/vm/unittests/aspace_unittest.cc`.
//!
//! Only the accessed harvesting tests are ported so far. The tagged variant is not ported, as
//! pointer tags are not.

use super::test_helper::make_committed_pager_vmo;
use crate::unittest::user_memory::UserMemory;
use crate::vm::arch_vm_aspace::{NonTerminalAction, TerminalAction};
use crate::vm::pmm;
use crate::vm::scanner::AutoVmScannerDisable;
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vm_object_paged::{VmObjectPaged, VmObjectReadWriteOptions};
use std::sync::atomic::Ordering;

const PAGE_SIZE: usize = page::SIZE;

fn harvest_access_bits(non_terminal_action: NonTerminalAction, terminal_action: TerminalAction) {
    let _scanner_disable = AutoVmScannerDisable::new();
    VmAspace::harvest_all_user_accessed_bits(non_terminal_action, terminal_action);
}

/// Consume the (scalar) value, ensuring that the operation to calculate the value can not be
/// optimized out/ deemed as unused by the compiler.
fn consume_value<T>(value: T) {
    std::hint::black_box(value);
}

// Touch mappings in an aspace and ensure we can correctly harvest the accessed bits.
#[test]
fn vmaspace_accessed_test_untagged() {
    let _scanner_disable = AutoVmScannerDisable::new();

    // Create some memory we can map touch to test accessed tracking on. Needs to be created from
    // user pager backed memory as harvesting is allowed to be limited to just that.
    let (vmo, pages) = make_committed_pager_vmo(1, false, false).unwrap();
    let page = pages[0];
    let mem = UserMemory::create_from_vmo(vmo, 0).unwrap();

    mem.commit_and_map(0..PAGE_SIZE).unwrap();

    let page_queues = pmm::node().page_queues();
    let queue = || page.page_queue().load(Ordering::Relaxed);

    // Initial accessed state is undefined, so harvest it away.
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);

    // Grab the current queue for the page and then rotate the page queues. This means any future,
    // correct, access harvesting should result in a new page queue.
    let mut current_queue = queue();
    page_queues.rotate_reclaim_queues();

    // Read from the mapping to (hopefully) set the accessed bit.
    consume_value(mem.user_in::<i32>().read().unwrap());
    // Harvest it to move it in the page queue.
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);

    assert_ne!(current_queue, queue());
    current_queue = queue();

    // Rotating and harvesting again should not make the queue change since we have not accessed
    // it.
    page_queues.rotate_reclaim_queues();
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
    assert_eq!(current_queue, queue());

    // Set the accessed bit again, and make sure it does now harvest.
    page_queues.rotate_reclaim_queues();
    consume_value(mem.user_in::<i32>().read().unwrap());
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
    assert_ne!(current_queue, queue());

    // Set the accessed bit and update age without harvesting.
    consume_value(mem.user_in::<i32>().read().unwrap());
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAge);
    current_queue = queue();

    // Now if we rotate and update again, we should re-age the page.
    page_queues.rotate_reclaim_queues();
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAge);
    assert_ne!(current_queue, queue());
    current_queue = queue();
    page_queues.rotate_reclaim_queues();
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAge);
    assert_ne!(current_queue, queue());
}

// Ensure that if a user requested VMO read/write operation would hit a page that has had its
// accessed bits harvested that any resulting fault can be handled.
#[test]
fn vmaspace_usercopy_accessed_fault_test() {
    let _scanner_disable = AutoVmScannerDisable::new();

    // Create some memory we can map touch to test accessed tracking on. Needs to be created from
    // user pager backed memory as harvesting is allowed to be limited to just that.
    let (mapping_vmo, _) = make_committed_pager_vmo(1, false, false).unwrap();
    let mem = UserMemory::create_from_vmo(mapping_vmo, 0).unwrap();

    mem.commit_and_map(0..PAGE_SIZE).unwrap();

    // Need a separate VMO to read/write from.
    let vmo = VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE as u64).unwrap();

    // Touch the mapping to make sure it is committed and mapped.
    mem.user_out::<u8>().copy_to_user(&42).unwrap();

    // Harvest any accessed bits.
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);

    // Read from the VMO into the mapping that has been harvested.
    let (read_status, read_actual) =
        vmo.read_user(mem.user_out::<u8>(), 0, 1, VmObjectReadWriteOptions::None);
    assert_eq!(read_status, Ok(()));
    assert_eq!(read_actual, 1);
}

/// fxdax (TASK-111.1): a page a guest physical address space maps, as a DAX window maps an extent's
/// pages into a hypervisor's second stage, is aged as accessed at every harvest for as long as it
/// is mapped, though nothing the host sees accesses it, and is never made inaccessible there:
/// eviction then takes the pages no guest maps before those a guest may be using. Unmapped, it
/// ages as any page not accessed does.
#[test]
fn guest_physical_mappings_are_aged_as_accessed_without_being_harvested() {
    use crate::arch::host::mmu::recording::RecordingSecondStage;
    use crate::hypervisor::aspace::GuestPhysicalAspace;
    use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE};
    use crate::vm::vm_address_region::{
        RangeOpType, VMAR_FLAG_SPECIFIC, VmAddressRegionOpChildren,
    };
    use std::sync::Arc;

    let _scanner_disable = AutoVmScannerDisable::new();
    let (vmo, pages) = make_committed_pager_vmo(1, false, false).unwrap();
    let page = pages[0];
    let second_stage = Arc::new(RecordingSecondStage::default());
    let gpa = GuestPhysicalAspace::create_with_second_stage(PAGE_SIZE, second_stage.clone())
        .expect("Failed to create aspace");
    let vmar = gpa.root_vmar();
    vmar.create_vm_mapping(
        0,
        PAGE_SIZE,
        0,
        VMAR_FLAG_SPECIFIC,
        vmo,
        0,
        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
        "window",
    )
    .expect("Failed to map");
    vmar.range_op(RangeOpType::MapRange, 0, PAGE_SIZE, VmAddressRegionOpChildren::Yes)
        .expect("Failed to populate");
    assert!(gpa.arch_aspace().query(0).is_ok(), "the page isn't mapped");
    second_stage.take();

    let page_queues = pmm::node().page_queues();
    let queue = || page.page_queue().load(Ordering::Relaxed);
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
    for _ in 0..3 {
        let aged = queue();
        page_queues.rotate_reclaim_queues();
        harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
        assert_ne!(aged, queue(), "a mapped page wasn't aged as accessed");
    }
    assert_eq!(second_stage.take(), vec![], "harvesting changed the second stage");

    vmar.unmap(0, PAGE_SIZE, VmAddressRegionOpChildren::No).expect("Failed to unmap");
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
    let unmapped = queue();
    page_queues.rotate_reclaim_queues();
    harvest_access_bits(NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest);
    assert_eq!(unmapped, queue(), "an unmapped page was aged as accessed");
}

/// fxdax (TASK-111.1): evicting a page leaves the mappings of the pages beside it that it doesn't
/// evict. Upstream unmaps the whole batch of pages an eviction looks at, and keeps those too young
/// to reclaim unmapped, which a guest's DAX window, whose second stage is another process's, only
/// maps again through a fault the extent daemon resolves.
#[test]
fn evicting_a_page_leaves_the_mappings_of_those_it_keeps() {
    use super::test_helper::make_reclaimable;
    use crate::arch::host::mmu::recording::RecordingSecondStage;
    use crate::hypervisor::aspace::GuestPhysicalAspace;
    use crate::vm::arch_vm_aspace::ARCH_MMU_FLAG_PERM_READ;
    use crate::vm::vm_address_region::{
        RangeOpType, VMAR_FLAG_SPECIFIC, VmAddressRegionOpChildren,
    };
    use crate::vm::vm_cow_pages::EvictionAction;
    use std::sync::Arc;

    const PAGES: usize = 16;
    let _scanner_disable = AutoVmScannerDisable::new();
    let (vmo, pages) = make_committed_pager_vmo(PAGES, false, false).unwrap();
    let second_stage = Arc::new(RecordingSecondStage::default());
    let gpa = GuestPhysicalAspace::create_with_second_stage(PAGES * PAGE_SIZE, second_stage.clone())
        .expect("Failed to create aspace");
    // The window maps all of the pages but the first.
    let vmar = gpa.root_vmar();
    let mapped = (PAGES - 1) * PAGE_SIZE;
    vmar.create_vm_mapping(
        PAGE_SIZE,
        mapped,
        0,
        VMAR_FLAG_SPECIFIC,
        vmo.clone(),
        PAGE_SIZE as u64,
        ARCH_MMU_FLAG_PERM_READ,
        "window",
    )
    .expect("Failed to map");
    vmar.range_op(RangeOpType::MapRange, PAGE_SIZE, mapped, VmAddressRegionOpChildren::Yes)
        .expect("Failed to populate");
    // The first page is old enough to evict, the rest just used.
    make_reclaimable(&pages);
    let page_queues = pmm::node().page_queues();
    for page in &pages[1..] {
        page_queues.mark_accessed(*page);
    }
    second_stage.take();

    let reclaimed = vmo.cow_pages().reclaim_page(pages[0], 0, EvictionAction::IgnoreHint).unwrap();
    assert_eq!(reclaimed.num_pages, 1);
    assert_eq!(second_stage.take(), vec![], "evicting the first page changed the window");
    for page in 1..PAGES {
        assert!(gpa.arch_aspace().query(page * PAGE_SIZE).is_ok(), "page {page} was unmapped");
    }
}
