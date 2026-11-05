---
id: TASK-92.12
title: >-
  Commit's write phase copies, zero-fills and scans each open extent's whole
  mapped length
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
ordinal: 287000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
commit_and_seal allocates vec![0; len], copies the whole VMO (extents.rs:1226-1227), scans back for the last non-zero block (486-492), and Store::write zero-fills and copies again (store.rs:2508-2514). len is the end of the writable mapping (1522-1524), committed eagerly (1532) up to 64 pages ahead of the data (dax.c:78-84), and the work runs under the open mutex. In pgbench this phase is about 30 µs per fsync for about 30 KiB; an estimated 20 µs is avoidable, about 20% of extentd's WRITE.

Where: crates/fxdax/src/extents.rs:1226-1234, crates/fxdax/src/extents.rs:486-492, crates/fxdax/src/extents.rs:1522-1524, crates/fxdax/src/extents.rs:1532, crates/extent_store/src/store.rs:2508-2514, crates/extent_store/src/framed.rs:29-33, guest/linux/fs/fxfs/dax.c:78-84.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. commit_and_seal does vec![0; len], vmo.read of the whole mapped length, then kept_len scans back for the last non-zero block (crates/fxdax/src/extents.rs:1164-1177, kept_len at ~471), all under the open mutex. Store::write zero-allocates an Aligned buffer and copies again (crates/extent_store/src/store.rs:1434-1441). There is no high-water mark from WRITE tracking, and extents never written are not skipped. Related: TASK-41, TASK-91.4, TASK-26.11.
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
Obsolete: made so by the deletion of writable DAX (37c331a6c). commit_and_seal, its vec![0; len] copy of each open extent's whole mapped length, kept_len's scan back and the open mutex are gone with the host's open pages: no extent is open in the window, so a commit copies nothing. What replaced it: the guest writes dirty pages back through fxfs_write_many (guest/rust/fxfs-linux/src/lib.rs:1053; from fxfs_dax_write_run, guest/linux/fs/fxfs/dax.c:1973), as WRITEs of exactly the bytes written; a commit is Store::commit_ordered and the wait for object storage (ExtentStore::commit_and_wait, crates/fxdax/src/extents.rs:1291-1313). A WRITE still copies its bytes twice (ExtentStore::write's Vec, extents.rs:1117-1125, then Store::write's Aligned buffer, crates/extent_store/src/store.rs:2082-2085), a smaller cost than this task's, not measured. Nothing was measured here, so the ACs stay unchecked.
<!-- SECTION:FINAL_SUMMARY:END -->
