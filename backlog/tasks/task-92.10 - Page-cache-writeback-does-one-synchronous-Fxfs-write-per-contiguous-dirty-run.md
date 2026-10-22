---
id: TASK-92.10
title: Page-cache writeback does one synchronous Fxfs write per contiguous dirty run
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
ordinal: 285000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
writepages starts a run at every gap in the file (super.c:1411-1417) and writes each run with a blocking fxfs_write, its own transaction and an awaited WRITE (fs.rs:752-758), one after another, through a 1 MiB kvmalloc bounce per call (1406). At ef225e3, non-DAX rand-write and mmap-rand-write ran at 2,379 IOPS against 11,814 for ext4, about one round trip per run. Large folios now lengthen runs for cached files, so it hits scattered order-0 folios.

Where: guest/linux/fs/fxfs/super.c:1394-1431, guest/rust/fxfs-linux/src/fs.rs:752-758, crates/fxfs/src/object_store/data_object_handle.rs:1114.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. fxfs_vfs_writepages starts a new run at any gap and writes each run with a blocking fxfs_write, one Fxfs write per run, through a 1 MiB kvmalloc bounce (guest/linux/fs/fxfs/super.c:1365-1431, the run break at 1411-1417, the bounce at 1406). There is no gathering of non-contiguous folios and no multi_write. Related: TASK-26.19.11, TASK-80, TASK-25.
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
