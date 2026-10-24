---
id: TASK-92.9
title: >-
  Cold sequential reads without DAX: readahead stays at 128 KiB and blocks the
  reader for each window
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 284000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fill_super never raises ra_pages or io_pages (super.c:2328), so 1 MiB reads become 128 KiB windows. ->readahead runs fxfs_read under block_on before returning (1289-1290), so only one READ is ever in flight, and each byte is copied three times. seq-read-cold runs at 212 MiB/s against 2,327 for ext4, about one round trip per window; verifiers expect about 2-3x from 1 MiB windows alone. A window larger than FXFS_IO_MAX falls back to one read per folio.

Where: guest/linux/fs/fxfs/super.c:2328, guest/linux/fs/fxfs/super.c:1283-1306, guest/rust/fxfs-linux/src/fs.rs:745-746, crates/fxfs/src/object_store/data_object_handle.rs:2390-2420.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. fill_super calls super_setup_bdi and never raises ra_pages or io_pages (guest/linux/fs/fxfs/super.c:2329-2332). fxfs_vfs_readahead runs a synchronous fxfs_read into a kvmalloc bounce and memcpys into each folio (super.c:1283-1306). A window larger than FXFS_IO_MAX gets buf=NULL, so its folios fall back to per-folio reads. Related: TASK-26.19.11, TASK-25, TASK-80.
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
075723b80: ra_pages and io_pages at FXFS_IO_MAX (1 MiB). Not measured yet (seq-read-cold in fs-bench); the synchronous readahead (one READ in flight) and its copies remain.
<!-- SECTION:NOTES:END -->
