---
id: TASK-286
title: >-
  Debug kernel KCSAN: data race in folio_clear_dirty_for_io /
  fxfs_vfs_page_mkwrite fails fxfs-guest-checks-dax-small
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 07:51'
updated_date: '2026-11-10 09:15'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE t257dbg1 (TASK-257's branch on main 142195ae2, debug kernel, fxdax-keep-n2-standard-4-us-west1-a-1), fxfs-guest-checks-dax-small round 3 of 5 failed on the console: 'BUG: KCSAN: data-race in folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite': a marked read-write of the folio's flags (8 bytes) in fxfs_vfs_page_mkwrite (do_wp_page, a write fault), against a plain read in folio_clear_dirty_for_io (writeback_get_folio, writeback_iter, fxfs_vfs_writepages, wb_workfn); value changed 0x000fffe00000412d -> 0x000fffe0000041ad (PG_dirty set). Near the torture check. Decide whether it is the kernel's benign folio-flag read (as TASK-44.8.1's reviewed readers) to be listed, or a page_mkwrite that dirties a folio writeback is cleaning without the folio lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The race reviewed: either fixed in fxfs_vfs_page_mkwrite or documented and listed as the kernel's benign reader
- [x] #2 fxfs-guest-checks-dax-small passes 20 rounds on the debug kernel with no KCSAN report
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the report against fxfs_vfs_page_mkwrite, fxfs_vfs_writepages and 6.18.54's writeback_get_folio/folio_clear_dirty_for_io; decide from the locking.
2. Benign (folio-lock waiter vs writeback's zone read under the lock): review folio_clear_dirty_for_io as a peer, and narrow every reviewed peer to the flag bits its race may change, so a dirtying of a folio writeback holds locked still fails.
3. Unit test (consoles_fail_on_their_kernels_complaints) with the report's shape and a PG_dirty-changing one that must fail.
4. ubu1: 20+ debug-kernel rounds of fxfs-guest-checks-dax-small with no KCSAN report.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Analysis (6.18.54 tree)
- Benign: the folio lock's own protocol. Writeback holds the folio's lock: writeback_get_folio locks it, folio_prepare_writeback (inlined) calls folio_clear_dirty_for_io, which asserts it (VM_BUG_ON_FOLIO(!folio_test_locked)). After folio_test_clear_dirty it counts the folio clean: lruvec_stat_mod_folio (out of line, __lruvec_stat_mod_folio, TASK-44.8.1's report 3) and zone_stat_mod_folio, inline, whose folio_zone reads the flags word plainly for the zone bits, fixed for the folio's life: this report's read. (test_bit is instrumented as an atomic read under KCSAN_ASSUME_PLAIN_WRITES_ATOMIC, so the plain read is the zone read.)
- fxfs_vfs_page_mkwrite's marked read-write is folio_lock's inline folio_trylock (test_and_set_bit_lock) failing because writeback holds the lock; it then waits, setting PG_waiters. The change 0x...412d -> 0x...41ad is bit 7, PG_waiters, not PG_dirty, as the task's description said: PG_dirty (bit 4) is clear in both values, which places the read after folio_test_clear_dirty. 0x412d = locked, referenced, uptodate, lru, active, private.
- mkwrite dirties the folio only under its lock (folio_lock, then folio_mark_dirty, VM_FAULT_LOCKED so the PTE is made writable before the unlock), the exclusion folio_clear_dirty_for_io's comment asks of fault handlers; writeback's folio_mkclean write-protects under the same lock. No store is lost.
- doc-13 invariant 3 (the semaphore) concerns window pages; fxfs_vfs_page_mkwrite serves page-cache inodes, which never map the window (doc-13's table). The folio lock is the rule here, as for any filesystem.
## Change
- vm.rs: folio_clear_dirty_for_io a reviewed peer. Every reviewed peer now names the flag bits its race may change (PG_locked|PG_waiters for writeback's peers; plus PG_uptodate for the reads' end): a race whose change has other bits (a dirtying of a folio writeback holds locked) fails, whatever its marking.
- Unit test consoles_fail_on_their_kernels_complaints: the report's shape passes, the same with PG_dirty changed (0x412d -> 0x413d) fails; red (left: Ok) with the bits condition removed (hand mutant, ubu1).

## Verification (ubu1)
- Debug kernel (KCSAN), fxfs-guest-checks-dax-small: 23 rounds passed (8+3+7+5 over 4 runs), none failing on a console complaint. Runs ended by budget overruns from the host's load (40-150 on 16 cores, other agents), FXDAX_CHECKS_SCALE=4 then 8. The folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite report did not recur in them, so its review is checked by the unit test alone, as 44.8.1's report 3 was. Reports naming fxfs in the 23 rounds: fxfs_vfs_read_folios / lru_gen_add_folio (4), / lru_add (3), folios_put_refs (1), folio_batch_move_lru (1), all within PG_locked|PG_uptodate|PG_waiters; kthread_is_per_cpu with fxfs_vfs_writepages, fxfs_rt_work_run and fxfs_dax_write_pages (the reviewed change).
- Unit test consoles_fail_on_their_kernels_complaints green; red with the bits condition removed (hand mutant). test_cases host 113/113, guest 114/114 on Linux.
- DoD 1: the root workspace is unchanged (guest/tests is a workspace of its own); scripts/test-gce.sh is retired. Not run on macOS.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Benign: writeback holds the folio's lock in folio_clear_dirty_for_io and reads the folio's zone plainly (zone_stat_mod_folio's folio_zone, inlined) while fxfs_vfs_page_mkwrite's folio_trylock fails and it waits, setting PG_waiters (0x412d -> 0x41ad is bit 7, not PG_dirty). mkwrite dirties only under the folio lock, the exclusion the kernel asks of fault handlers. Invariant 3 concerns window pages, which page-cache inodes never map. folio_clear_dirty_for_io is now a reviewed KCSAN peer, and every peer names the flag bits its race may change, so a dirtying of a folio that writeback holds locked still fails. Verified: the unit test (red under the hand mutant), and 23 debug-kernel rounds of fxfs-guest-checks-dax-small on ubu1 with no complaint.
<!-- SECTION:FINAL_SUMMARY:END -->
