---
id: TASK-207
title: >-
  Guest DAX: a writeback failing ENOMEM leaves its pages clean, the store
  keeping what they held before
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 09:15'
updated_date: '2026-11-06 12:00'
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
- [x] #1 A writeback of a DAX file whose writing or commit fails ENOMEM leaves the pages it hadn't made durable dirty, their reservations kept, so that a later writeback writes them
- [x] #2 A guest/sim scenario fails a writeback's allocation and its commit with ENOMEM and checks the file, through a clone and after a rebind, holds what was written; red on main
- [x] #3 fxfs-guest-fsx-faults-dax passes 20 minutes on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Keep the pages dirty for ENOMEM (doc-13: writeback was not to fail for want of room, reservations taken at dirtying; ext4 keeps ENOMEM's pages), not the file marked failed: an I/O error's pages stay lost as TASK-149.15 decided. 2. fxfs_dax_write_pages records each run's pages; a run Fxfs had no memory for keeps them (batch's kept); fxfs_dax_write_back puts them back in the dirty set (pages.c's fxfs_dax_pages_redirty), reserves their room again (fxfs_reserve_dirty_what_fits: write_dirty took it), marks the inode dirty, and fails ENOMEM without the file's error. 3. Eviction tries again for a second, then gives them up as an I/O error's (fxfs_dax_write_back_keeping(false)). 4. guest/sim: fxfs_sim_fail_commits_with(count, ENOMEM); scenario cow_writeback_found_no_memory, red on main; mutant task-207-no-memory-writeback-lost; the writeback mutants rebased. 5. doc-13's writeback step 4; OPEN_QUESTIONS line. 6. GCE: fxfs-guest-fsx-faults-dax 20 min on the debug kernel (r780).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decision: keep the pages dirty for ENOMEM, not mark the file failed (doc-13: room is reserved at dirtying so that writeback doesn't fail for want of it; ext4 keeps ENOMEM's pages; a file failed for good loses data for a transient error). An I/O error's pages stay lost, TASK-149.15's rule. doc-13's writeback step 4; OPEN_QUESTIONS line.

Fix (522f22147, 74243d3e6): fxfs_dax_write_pages records each run's pages (fxfs_dax_run_part); a run whose fxfs_write_many failed ENOMEM keeps them (batch's kept, fxfs_dax_ran). fxfs_dax_keep, the writeback lock still held, puts them back in the dirty set (pages.c's fxfs_dax_pages_redirty: those still private, not dirtied again meanwhile), counts them, reserves their room again (fxfs_reserve_dirty_what_fits: write_dirty took it, success or not) and marks the inode dirty. The writeback fails ENOMEM without mapping_set_error; callers pass it on (fsync, truncate's resize_begin, remap_begin, the drop loop) or retry later (the budget's worker). Eviction tries again 100 times 10 ms apart, then fxfs_dax_write_back_keeping(false), as before. Also in 522f22147: a PRIVATE the host refused (ENOMEM past private_max) is EIO again, as TASK-170 decided, which c6a7fca30 had overturned by passing every ENOMEM; fxfs_device_private now tells the host's refusal (EIO, logged) from the guest's own request not allocated (ENOMEM).

Tests: guest/sim's fxfs_sim_fail_commits_with(count, -ENOMEM); cow_writeback_found_no_memory fails a writeback's commit with ENOMEM and checks the fsync's ENOMEM, Fxfs holding the file as it was, the room reserved again, a clone of the file, a rebind, and the next fsync: red on main (no room reserved again; the clone and the rebind brought back the old blocks), green with. The writeback's allocations failing are the sweep's, each in turn (108 calls of the scenario, 221 of the three): pass. Mutant task-207-no-memory-writeback-lost, killed. Eight writeback mutants onto the change (164.2-writeback-holds-no-range, 164.4-commit-failure-dropped, -fsync-skips-writeback, -protect-by-entries, -wb-sync-none, -writeback-unlocked, -writeback-unowned, 164.9-private-failure-ignored): all killed. scenarios, zloop -s 1000, mutants.sh --check 41/41, guest-mutants-gce.py --check 45/45, fxfs-sim-diff's mutants 4/4 and tests pass. GCE r782 (us-east1-c, debug kernel, jailed): fxfs-guest-fsx-faults-dax 20 min, 30 scripts, 900 operations refused for injected ENOMEM, 2,787 files read in DAX: pass. r780 (us-east4-a, the fallback): fxfs-guest-fsx-dax 42 scripts, fxfs-guest-checks-dax and fxfs-guest-faults-dax pass on the debug kernel; its -faults-dax died of the harness's own fail-nth read (fixed in 68ea2efd8). DoD: macOS scripts/test.sh clippy and workspace 2,039/2,039; Linux (Docker, aarch64) workspace 2,279: 2,276 and the 3 grants tests that need sudo, 16/16 once installed; upstream-changes.py --check passes.

Every-bug notes. Found by fxfs-guest-fsx-faults-dax's lineage of the differing byte (r699). Blind spot: guest/sim's failed commits were EIO alone, and its file model counted a failed writeback's pages lost (TASK-149.15), which hid that ENOMEM's should not be. Class test: commits failing with any errno (fxfs_sim_fail_commits_with); the fsx faults case with the store held to the model after any refusal. Looked around: the page cache's writeback (fxfs_vfs_write_run, super.c) leaves ENOMEM's folios clean too: filed as TASK-212, super.c being guest-199's (TASK-199 to 202) meanwhile. Structure: what a failed run's pages become is decided in one place (fxfs_dax_ran), not per caller; a file failed for good was the other shape, rejected above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A DAX writeback that Fxfs had no memory for now keeps its pages dirty, room reserved again, failing ENOMEM without the file's error, where they were left clean and a clone, rebind or eviction brought back the old blocks (r697, r699). I/O errors keep TASK-149.15's semantics; eviction retries, then gives up. The host's PRIVATE refusal is EIO again (TASK-170). Proven by guest/sim's cow_writeback_found_no_memory (red on main) and its mutant, the rebased writeback mutants, and fxfs-guest-fsx-faults-dax passing 20 minutes on the debug kernel (GCE r782). The page cache's writeback has the same flaw: TASK-212.
<!-- SECTION:FINAL_SUMMARY:END -->
