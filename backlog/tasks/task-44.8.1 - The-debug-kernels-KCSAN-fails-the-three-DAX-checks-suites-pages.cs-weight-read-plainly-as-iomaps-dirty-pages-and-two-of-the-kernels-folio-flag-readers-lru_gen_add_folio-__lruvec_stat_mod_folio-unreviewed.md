---
id: TASK-44.8.1
title: >-
  The debug kernel's KCSAN fails the DAX checks suites: pages.c's weight read
  plainly as iomaps dirty pages, and three of the kernel's folio-flag readers
  (lru_gen_add_folio, folios_put_refs, __lruvec_stat_mod_folio) unreviewed
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 09:58'
updated_date: '2026-11-05 13:05'
labels: []
dependencies: []
parent_task_id: TASK-44.8
priority: high
type: bug
ordinal: 636000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On the debug guest kernel (FXDAX_GUEST_KERNEL_BUILD=debug, KCSAN), fxfs-guest-checks-dax, -dax-delay and -dax-small fail on KCSAN reports naming fs/fxfs (GCE r611 on 38691000f, r613 on 490faad2c, land-157): `_find_next_bit / fxfs_dax_pages_dirty` (a write fault and a pwrite racing on pages.c's dirty and private bitmaps, which TASK-160.4 made a module), `fxfs_vfs_read_folios / lru_gen_add_folio`, and `__lruvec_stat_mod_folio / fxfs_vfs_page_mkwrite`. The runner fails a test on any KCSAN report naming fxdax's code (TASK-44.8) unless it was looked into (vm.rs KCSAN_REVIEWED_PEERS, KCSAN_REVIEWED_CHANGE). Each needs deciding: a real race fixed, a benign one marked or reviewed, none silenced unread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each of the three reports is analysed in this task: real or benign, and why, against pages.c, dax.c, super.c and the kernel code it races
- [x] #2 Real races fixed with locking or atomics; benign ones in fxdax's code read as races (READ_ONCE or data_race) with a comment saying why they are safe; benign kernel peers reviewed in KCSAN_REVIEWED_PEERS, narrowly enough that a plain access of fxdax's own is still reported
- [x] #3 guest/sim: zloop.sh -s 3000 passes, and the mutants are caught as before
- [x] #4 GCE on the debug kernel: fxfs-guest-checks, -dax, -dax-delay, -dax-small and fxfs-guest-dax pass with no KCSAN report of fxdax's left; the release kernel's run of them passes too
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read each report against pages.c, dax.c, super.c and the kernel functions it names (6.18.54 tree).
2. pages.c: read private and dirty as the race they are under the shared semaphore: fxfs_dax_pages_weight word by word with READ_ONCE, any_dirty through it; pages.h says so.
3. vm.rs: lru_gen_add_folio, folios_put_refs (r617) and __lruvec_stat_mod_folio reviewed peers; a peer race reviewed only while fxdax's side is marked (parse the report's accesses); unit test with real report shapes, red with the condition removed.
4. Verify: test_cases unit test on Linux; guest/sim zloop.sh -s 3000 and the mutants; the debug guest kernel builds (Docker, arm64); GCE debug runs of fxfs-guest-checks, -dax, -dax-delay, -dax-small and fxfs-guest-dax, jailed; a release kernel run of the same.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## The reports (GCE r611 on 38691000f, r613 on 490faad2c; r617 on this branch)

1. `_find_next_bit / fxfs_dax_pages_dirty` (checks-dax-small and -dax-delay, r611 and r613): a pwrite's iomap in fxfs_dax_pages_dirty (test_and_set_bit, marked RMW) and a write fault's in fxfs_dax_pages_would_dirty -> fxfs_dax_pages_weight -> find_next_bit (plain loads) on the same word of `private` or `dirty`, both in fxfs_dax_dirty holding the file's semaphore shared (values 0x1ff -> 0x3ff, 0xfffffffe000 -> 0x1fffffffe000: a bit set). A race, benign: only transitions under the semaphore held for writing clear bits (rebind, cut, unmap, swap), so what a reader holding it shared sees set stays set and its count of clear bits is at least what fxfs_dax_pages_dirty then makes so. fxfs_dax_dirty already reserves and counts for the would-count, then gives back the difference from what fxfs_dax_pages_dirty returns (newly > made.private, fresh > made.dirty). The order that matters (private before dirty; a store after the PRIVATE that made a page private) comes from fxfs_dax_pages_dirty's own test_and_set_bit, fully ordered, not from the count. Fix: fxfs_dax_pages_weight reads each word once with READ_ONCE and popcounts it, masked at the ends, so the reads are marked and each word a single snapshot; not data_race(), which would hide a plain read we didn't mean too. Looking around: fxfs_dax_pages_any_dirty (bitmap_empty, plain) is called by writeback's count of dirty ranges under down_read (dax.c, fxfs_dax_write_back: `most`), beside iomaps: the same race, unreported so far; now through weight. Every other read of private or dirty is under the semaphore held for writing (fill's and rebind's WARNs, next_rebindable, check, swap) or test_bit, which the kernel instruments as an atomic read (unwritten).
2. `fxfs_vfs_read_folios / lru_gen_add_folio` (checks-dax r611, -dax-delay r613): the readahead worker's folio_mark_uptodate and folio_unlock (marked, 0x1 -> 0x8: PG_locked cleared, PG_uptodate set) as filemap_add_folio -> folio_add_lru -> folio_batch_move_lru -> lru_add -> lru_gen_add_folio reads the flags word plainly for folio_zonenum (its zone bits, fixed for the page's life; folio_lru_gen is READ_ONCE, the rest test_bit). The race reviewed in r565 as folio_batch_move_lru/lru_add, KCSAN naming the multi-gen LRU's function this time. Benign; any filesystem whose reads end off the reader races it. Reviewed peer.
3. `__lruvec_stat_mod_folio / fxfs_vfs_page_mkwrite` (checks-dax-small r611): writeback (writeback_get_folio, holding the folio's lock) -> folio_clear_dirty_for_io -> __lruvec_stat_mod_folio reads the flags word plainly for folio_pgdat (node bits, fixed), as fxfs_vfs_page_mkwrite's folio_lock (test_and_set_bit_lock, marked) fails and waits, setting PG_waiters (0x412d -> 0x41ad: bit 7). Benign; filemap_page_mkwrite does the same. Reviewed peer.
4. `folios_put_refs / fxfs_vfs_read_folios` (fxfs-guest-checks, r617, not in r611/r613): folio_batch_move_lru ends with folios_put, whose folios_put_refs reads the flags word plainly for folio_is_zone_device (zone bits) as the readahead worker unlocks the folio (0x21 -> 0x28). The same race as 2. Main's filter fails it too (its title names neither reviewed peer). Reviewed peer.

## Reviewing peers narrowly
KCSAN_REVIEWED_PEERS was a list of names: a report naming one passed whatever fxdax's side did. complaints() now reads the report's access lines ("write (marked) to 0x... by task N on cpu M:", then the stack's top frame) and reviews a peer's race only if fxdax's access in it is marked: a plain access of fxfs's to a folio's flags would be a race of its own, and still fails its test. consoles_fail_on_their_kernels_complaints has the four reports' shapes, and a peer race whose fxdax side is plain, or not in the report, failing; red (left: Ok) with the condition removed.

## Blind spot and structure
The sim can't see this class (README: atomics are sequentially consistent, data_race doesn't show); KCSAN on the debug kernel is its oracle, and found it within one run of each DAX suite once TASK-160.4 put the counts in pages.c. Structure: pages.h says the semaphore held shared lets fxfs_dax_pages_dirty run beside queries; the race lives only in queries callable under it (would_dirty, any_dirty, unwritten), all marked now, and pages.h says they read the bitmaps as the race it is. Making every read in pages.c marked whatever the lock was considered and not done: the rest run under the write hold, where a plain read is right.

## Verification
- test_cases (guest/tests) host tests in Docker (rust:1, arm64): 57 of 60, consoles_fail_on_their_kernels_complaints passing; the 3 failures are sqlite::tests, no sqlite3 binary in the image (they pass on GCE, 57/57).
- Debug guest kernel (KCSAN, arm64) builds: scripts/build-guest-kernel.sh in debian:trixie, OrbStack, 8m52s.
- guest/sim (Docker, seccomp unconfined: the sim calls personality() to turn ASLR off, which the default profile refuses): zloop.sh -s 3000, 0 of 3,000 seeds failed (721 s); mutants.sh --check 31 checked, 0 broken; mutants.sh: every mutant caught but the five the README expects (task-164.4-truncate-unwritten, task-79.28-range-too-small, task-153.1-fault-limit, patch-0012-invalidate-unmaps-after, task-44.19-map-to-unlocked).
- GCE, FXDAX_WORKSPACE_TESTS=0 FXDAX_FIRECRACKER_JAIL=1, fxfs-guest-checks, -dax, -dax-delay, -dax-small, fxfs-guest-dax: r615 debug, 5/5, on the pages.c fix and the first three peers; r616 release, 5/5; r617 debug, 4/5, fxfs-guest-checks failing on report 4; r618 debug, 5/5, with folios_put_refs; r619 debug on d46c8fb12, 5/5. In r615, r618 and r619 the reports naming fxfs left are the reviewed peers (fxdax's side marked) and kthread_is_per_cpu's 0x40000 change; no _find_next_bit / fxfs_dax_pages_dirty in any. Report 3 didn't recur in r615-r619: its review is checked by the unit test alone.
- scripts/upstream-changes.py --check passes. The root workspace's nextest not run: nothing of it changed (guest/linux, guest/tests are outside it).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Four KCSAN reports of the DAX checks suites on the debug kernel, each read and found benign. pages.c's fxfs_dax_pages_weight (and any_dirty through it) reads the private and dirty bitmaps word by word with READ_ONCE, as iomaps holding the semaphore shared set bits beside it; the counts were already reconciled by fxfs_dax_dirty. lru_gen_add_folio, folios_put_refs and __lruvec_stat_mod_folio are reviewed KCSAN peers (the kernel reading a folio's zone or node plainly as fxfs changes its flags atomically), and any peer's race is reviewed only while fxdax's side is marked. Verified: test_cases unit test (red without the condition), debug kernel builds, zloop 3,000 seeds and the mutants, GCE r615/r618/r619 debug and r616 release all 5/5 (r617 found report 4). Commits ca14dcc72, d46c8fb12.
<!-- SECTION:FINAL_SUMMARY:END -->
