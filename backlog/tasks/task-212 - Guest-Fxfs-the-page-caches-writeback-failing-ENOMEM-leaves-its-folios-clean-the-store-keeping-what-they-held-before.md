---
id: TASK-212
title: >-
  Guest Fxfs: the page cache's writeback failing ENOMEM leaves its folios clean,
  the store keeping what they held before
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-06 11:58'
labels:
  - bug
dependencies: []
priority: high
ordinal: 649000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reading the page cache's writeback for TASK-207 (2026-11-06): fxfs_vfs_write_run (guest/linux/fs/fxfs/super.c) ends every folio's writeback after fxfs_write_many, setting the file's error on any error, ENOMEM too: the folios are clean, Fxfs holds the blocks they replaced, and an eviction of the folios (fadvise, reclaim, drop_caches) brings those back, as TASK-207 found of DAX's private pages. TASK-207 fixed the DAX path only: super.c was being changed by the guest-199 branch (TASK-199 to 202) meanwhile. Files a DAX mount creates are in the page cache until read in again, so fxfs-guest-fsx(-faults-dax) runs this path; r782's 20 minutes found no mismatch, but no run checks a folio whose writeback found no memory once it is evicted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A page-cache writeback whose fxfs_write_many fails ENOMEM redirties its folios (folio_redirty_for_writepage), their room reserved again, without setting the file's error
- [ ] #2 A guest check, or guest/sim if it gains the page cache's writeback, fails a writeback with ENOMEM, evicts the folios and reads what was written; red on main
- [ ] #3 fxfs-guest-fsx-faults-dax passes 20 minutes on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
