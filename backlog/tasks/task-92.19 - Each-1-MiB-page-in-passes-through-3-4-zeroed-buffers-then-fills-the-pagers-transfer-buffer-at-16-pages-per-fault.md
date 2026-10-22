---
id: TASK-92.19
title: >-
  Each 1 MiB page-in passes through 3-4 zeroed buffers, then fills the pager's
  transfer buffer at 16 pages per fault
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 294000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A packed part is read into a zeroed Aligned and copied with to_vec (framed.rs:29-33, 46-53). It is unpacked into vec![0; len] (lib.rs:127-131), copied into an Arc (framed.rs:240), then into aligned_read's zeroed data (store.rs:1615-1618; fxextent.rs:213-214), then into the transfer buffer (215). That buffer's pages left with the last supply, so the copy faults through SIGSEGV 16 pages at a time (vm_mapping.rs:51, 975-984), with an unpopulated mmap per fault (mmu.rs:249-258), about 256 host faults per part and a shootdown at supply. Estimated 0.3-0.6 ms extra per part on the 6 shared pager threads, for cold fork boots and refaults.

Where: crates/extent_store/src/framed.rs:29-33, crates/extent_store/src/framed.rs:46-53, crates/extent_store/src/framed.rs:240, crates/extent_frame/src/lib.rs:110, crates/extent_frame/src/lib.rs:127-131, crates/extent_store/src/store.rs:1609-1618, crates/fxfs-platform/src/fuchsia/fxextent.rs:211-216, crates/storage_device/src/buffer_allocator.rs:67-71, crates/zircon/src/vm/vm_mapping.rs:51, crates/zircon/src/vm/vm_mapping.rs:975-984, crates/zircon/src/arch/host/mmu.rs:249-258, crates/zircon/src/arch/host/mmu.rs:442-444, crates/zircon/src/vm/vm_aspace.rs:226-230.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. aligned_read commits the buffer's pages, then reads into vec![0; read] and copies into the transfer buffer (crates/fxfs-platform/src/fuchsia/fxextent.rs:201-217). Part::unpack goes through Stored::unpack's zeroed Vec into an Arc (crates/extent_store/src/framed.rs ~238-243; crates/extent_frame/src/lib.rs:127-131). read_aligned still uses a zeroed Aligned plus to_vec (framed.rs:46-53). Non-guest faults still map at most PAGE_FAULT_MAX_OPTIMISTIC_PAGES = 16 (crates/zircon/src/vm/vm_mapping.rs:59, 971-975), with one mmap per install (mmu.rs:217-262). HOST_ASPACES is still a Vec scanned linearly (vm_aspace.rs:89, 222-227). Related: TASK-38, TASK-28.8, TASK-26.17.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
