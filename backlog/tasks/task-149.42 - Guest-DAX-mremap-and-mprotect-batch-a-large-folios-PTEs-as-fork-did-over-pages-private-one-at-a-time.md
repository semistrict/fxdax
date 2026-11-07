---
id: TASK-149.42
title: >-
  Guest DAX: mremap and mprotect batch a large folio's PTEs, as fork did, over
  pages private one at a time
status: To Do
assignee: []
created_date: '2026-11-07 10:24'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 658000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.16 found fork copying a large DAX folio's PTEs as one, all writable if any was (6.18's copy_present_ptes, FPB_MERGE_WRITE): a chunk read in whole (a PMD, so its window pages one folio), split by a store, gave a forked child writable PTEs of pages no PRIVATE had made the VM's, and the host refused its store till the VM went down (GCE r861). Fixed by dropping VM_MIXEDMAP, so fork copies no PTEs of Fxfs's mappings. Other paths batch a large folio's PTEs too and may carry one PTE's write bit to its neighbours: mremap's move_ptes (folio_pte_batch, set_ptes of the first PTE's bits, 6.17) and mprotect's change_pte_range (6.17's batching, MM_CP_TRY_CHANGE_WRITABLE). Copying on write makes each page private on its own, so a folio's pages differ in writability, which those paths assume they don't. The hostile scripts' Mremap moves a mapping made for its stores, not the kept one; nothing calls mprotect.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest check moves (mremap) and mprotects a mapping of a chunk read in whole, split by a store, then stores through the result to a page it only read: the store faults and makes the page private, or the check fails as r861 did
- [ ] #2 Fxfs keeps a chunk's folio from mixing writable and read-only PTEs, or each path is shown not to merge write bits, with the kernel's code cited
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
