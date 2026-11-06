---
id: TASK-212
title: >-
  Guest Fxfs: the page cache's writeback failing ENOMEM leaves its folios clean,
  the store keeping what they held before
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 11:58'
updated_date: '2026-11-06 14:26'
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
- [x] #1 A page-cache writeback whose fxfs_write_many fails ENOMEM redirties its folios (folio_redirty_for_writepage), their room reserved again, without setting the file's error
- [x] #2 A guest check, or guest/sim if it gains the page cache's writeback, fails a writeback with ENOMEM, evicts the folios and reads what was written; red on main
- [x] #3 fxfs-guest-fsx-faults-dax passes 20 minutes on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
As TASK-207: a run whose fxfs_write_many failed ENOMEM keeps its folios dirty, the room of its pages reserved again; an I/O error's stay lost. Guest check through fail-nth (guest/sim doesn't build super.c), red with a debug-kernel mutant on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (6c4ae999d): fxfs_vfs_writepages notes each part's folio and pages (fxfs_vfs_run_piece); fxfs_vfs_write_run, on ENOMEM, has fxfs_vfs_keep set those pages dirty in the folio's state, reserve the room of those without (fxfs_reserve_dirty_what_fits: the failed fxfs_write_many took the run's), and dirty the folio (filemap_dirty_folio), all before the folios' writeback ends, which a truncation waits for; mapping_set_error only for other errors. OPEN_QUESTIONS: TASK-207's line extended to the page cache.

Test: guest check writeback_out_of_memory (page cache suites, PageCache, 20 s budget): a file's 16 pages written, its fsync with each of its first 64 allocations failing in turn (Faults::failing, /proc/thread-self/fail-nth), the fsync ENOMEM or nothing, the file's clean pages dropped (POSIX_FADV_DONTNEED, drop_caches 1) read as written, synced and evicted after. Without fault injection it says so and passes (a release kernel); the debug kernel's suites run it (soak-sanitized). Red: mutant task-212-no-memory-folios-lost (Kernel: debug), killed on GCE r784 (us-west1-b): 'allocation 3 failing, the fsync Err(ENOMEM): the file dropped differs'. Green: r783 (us-east1-c, debug kernel, jailed): fxfs-guest-checks passes, writeback_out_of_memory with 4 of 64 fsyncs' allocations failed. r785 (us-east1-b, the fallback from us-central1-b): fxfs-guest-fsx-faults-dax 20 min passes, 30 scripts, 901 refusals. r783's own -faults-dax failed on the harness, an O_DIRECT read's untouched buffer faulting under the injected failure (EFAULT): buffers now faulted in first (5c7329da7). It didn't replay: fail-nth's choice is the seed's, but which allocation is the Nth depends on what the kernel has cached, so a replay from a boot, or a shrink's try, may fail another. check-guest (arm64, cocci, Rust check, fs/fxfs compiled) passes; guest-mutants-gce.py --check 46/46. DoD 1: no workspace crate changed here (super.c and the guest test crate, whose tests pass 90/90 in Docker).

Every-bug notes: found reading the code for TASK-207's look-around. Blind spot: no test failed a page cache writeback's allocations and then dropped the clean folios. Class test: writeback_out_of_memory fails each allocation of the fsync in turn. Structure: the page cache's and DAX's writebacks now decide a failed run's pages the same way, each in its own run function.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A page cache writeback Fxfs had no memory for keeps its folios dirty, room reserved again, failing ENOMEM without the file's error, as TASK-207 made DAX's. Proven by guest check writeback_out_of_memory (each fsync allocation failed in turn through fail-nth): red with mutant task-212-no-memory-folios-lost on GCE r784, green on r783; fxfs-guest-fsx-faults-dax 20 min passes (r785).
<!-- SECTION:FINAL_SUMMARY:END -->
