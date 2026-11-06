---
id: TASK-207
title: >-
  Guest DAX: a writeback failing ENOMEM leaves its pages clean, the store
  keeping what they held before
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-06 09:15'
labels:
  - bug
dependencies: []
priority: high
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-fsx-faults-dax (TASK-44.15), the debug kernel failing Fxfs's calls' allocations (failslab, fail_page_alloc). GCE r699, run seed 1793568930182594002: an fsync of file 0 refused ENOMEM (op 546); a clone of 0x2000 bytes within file 0, 0x3da000 onto 0xf0000 (op 556), succeeded, and file 0 then read at 0xf0000 what 0x3da000 held before op 528's write, which read(2) of 0x3da000 still returned. The failure's lineage (script_run's Run::lineage): 556 clone, its source last written by 528, the fsync between refused. r697 (run seed 1793606963020038581) is likely the same: a truncate of file 2 refused ENOMEM, whose fxfs_dax_resize_begin writes back first, then a clone of file 2 onto file 0 carrying old bytes past 0x1fe4f6. Neither replays: fault injection isn't seeded.

Why: fxfs_dax_write_back (guest/linux/fs/fxfs/dax.c) swaps the file's dirty set out before it writes, and if fxfs_dax_write_pages fails, with any error, the pages stay clean, 'as the page cache's writeback: the pages are clean, the error the file's'. The private pages still hold the data, so reads look right, but a writeback after finds nothing dirty, and a clone (which clones the store's blocks), a rebind or an eviction brings the old blocks back: data lost after a transient failure, no I/O having failed. The page cache's writeback drops pages on EIO, but filesystems keep them dirty on ENOMEM (ext4 redirties and retries). guest/sim's scenarios hold a failed commit to leave the file as a crash would (scenarios.c, an fsync whose writeback's commit failed), which is right for EIO, not for ENOMEM. Since the same batch's ENOMEM fix (Run's refusals: TASK-44.15), a device's ENOMEM reaches the caller as ENOMEM, so the error is now seen as transient, not EIO.

Until fixed, fxfs-guest-fsx-faults-dax (soak-sanitized) fails within minutes of fault injection.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A writeback of a DAX file whose writing or commit fails ENOMEM leaves the pages it hadn't made durable dirty, their reservations kept, so that a later writeback writes them
- [ ] #2 A guest/sim scenario fails a writeback's allocation and its commit with ENOMEM and checks the file, through a clone and after a rebind, holds what was written; red on main
- [ ] #3 The page cache's writeback (fxfs_vfs_writepages) is checked for the same, and fixed or recorded as not losing data
- [ ] #4 fxfs-guest-fsx-faults-dax passes 20 minutes on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
