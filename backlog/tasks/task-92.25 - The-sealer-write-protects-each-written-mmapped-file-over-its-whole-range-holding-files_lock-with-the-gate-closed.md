---
id: TASK-92.25
title: >-
  The sealer write-protects each written, mmapped file over its whole range,
  holding files_lock, with the gate closed
status: Done
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-05 03:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 300000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
At each seal, fxfs_dax_seal holds files_lock and calls wp_shared_mapping_range(mapping, 0, ULONG_MAX) on every written file (dax.c:1954-1959), with stores drained (device.c:204-215). That walks every present PTE, though range->writable lists exactly the pages to protect (1962-1963). mmap-rand-write pays about 0.1-0.5 ms per seal, and a 1 GiB mmap store pays 1 ms or more per commit, blocking writes, igets (2159) and reclaim (766).

Where: guest/linux/fs/fxfs/dax.c:766, guest/linux/fs/fxfs/dax.c:1953-1966, guest/linux/fs/fxfs/dax.c:2159, guest/linux/fs/fxfs/device.c:204-215.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. fxfs_dax_seal takes files_lock for each writable file and calls wp_shared_mapping_range(mapping, 0, ULONG_MAX) inside it (guest/linux/fs/fxfs/dax.c:1973-1980). It runs with stores drained (device.c:fxfs_device_seal_begin waits for !stores before sealer). range->writable is only zeroed afterwards and is not used to narrow the range (1982-1984). Not tracked. TASK-91.2 is about the seal's object-storage wait, not this walk. Related: TASK-91.2, TASK-25.4, TASK-26.14.
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

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Obsolete: made so by the deletion of writable DAX (37c331a6c). Nothing a seal does write-protects any more: fxfs_dax_seal's whole-range wp_shared_mapping_range(mapping, 0, ULONG_MAX) under files_lock, range->writable and the gate that drained stores (fxfs_device_store_begin/end) are gone. What replaced it: each writeback write-protects only the dirty runs of the pages it took (fxfs_dax_protect, guest/linux/fs/fxfs/dax.c:2054-2069, called from fxfs_dax_write_back at 2140), holding only its file's writeback lock, after its semaphore is let go; the seal (fxfs_device_seal_begin/end, guest/linux/fs/fxfs/device.c:167-192) waits for no stores. Nothing was measured, so the ACs stay unchecked.
<!-- SECTION:FINAL_SUMMARY:END -->
