---
id: TASK-92.53
title: >-
  zircon port: a VMO's range changes visit every mapping of the VMO, where
  Zircon's visit the range's alone
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 19:55'
updated_date: '2026-10-23 20:43'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 362000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found investigating TASK-92.50 (branch r6/zircon-map). The port kept a VMO's mappings keyed by pointer (crates/zircon/src/vm/vm_cow_pages.rs), so range_change_update_mappings_locked visited every mapping of the VMO on each range change: a zero-fork commit or fault (map_open's COMMIT hits it), a page added, supply, eviction, writeback's RemoveWrite, decommit. Upstream keys mapping_list_ by {object_offset, VmMapping*} in a B-tree augmented with min/max offsets (vm_object.h:709-727) and RangeChangeUpdateMappingsLocked skips subtrees outside the range (vm_object.cc:295-345). vmar-overwrite-bench: a commit's range change at 1k/10k/100k mappings of one VMO took 3.4/28/398 us. Not what TASK-92.50 measured: map_unsafe's overwrite makes no range change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A range change visits only the mappings overlapping it, shown by a test counting what it reaches
- [x] #2 vmar-overwrite-bench's commit row stays flat from 1k to 100k mappings, and maps cost no more than a stated amount
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A VMO's range changes now visit only the mappings overlapping the range, as upstream Zircon's do: the port kept a VMO's mappings by pointer and walked them all on each range change; they are now kept by (offset, address) in an augmented AVL tree (crates/zircon/src/vm/vm_mapping_tree.rs, marked fxdax: where it deviates from upstream's B-tree), each node holding its subtree's greatest end, so a walk prunes subtrees outside the range. vmar-overwrite-bench: a commit's range change at 1k/10k/100k mappings of one VMO went from 3.4/28/398 us to 1.2/1.3/1.7 us; plain maps cost up to 13-17% more at 100k mappings of one VMO (0.2-0.3 us), documented next to the deviation. Verified by tests counting exactly the nodes a walk looks at, a model test against a sorted list with drain phases, every add/remove order of up to five mappings, the hypervisor test of what range changes reach, macOS 2479/2479 and Linux (GCE) 2557/2557 with all Shuttle suites.
<!-- SECTION:FINAL_SUMMARY:END -->
