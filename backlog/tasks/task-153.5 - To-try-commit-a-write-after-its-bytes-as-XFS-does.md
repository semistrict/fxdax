---
id: TASK-153.5
title: 'To try: commit a write after its bytes, as XFS does'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 22:52'
updated_date: '2026-11-03 17:53'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 549000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs_dax_map_write commits the move, losing the old bytes, before the copy. Explains TASK-79.11, 79.106, 119.13, 121, 133, 150.8, 150.13; a write that says EIO can still leave zeros (map_failed_writing). XFS allocates, maps, copies, then commits the extents written (xfs_reflink_end_cow). Costs a transaction spanning the gate.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A prototype
- [x] #2 pgbench-dax measured against it, several rounds a side
- [x] #3 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxfs-posix: dax_begin_write returns a DaxWrite (Transaction<'static> beside the Arc'd handle it borrows), committed or dropped later; dax_map_write and it share retrying_sealed and txn_dax_map_write.
2. fxfs-linux/fxfs.h: fxfs_dax_begin_write / fxfs_dax_end_write(write, commit).
3. dax.c: write(2) moves pending in the loop; the file's semaphore held from iomap to iomap_end; commit there, out of the gate; drop on seal, MAP failure, short copy, failed commit (range falls behind, file->unwritten corrects dax_iomap_rw's count). Prepare (commit-first) only for writes overwriting no whole in-file page. No bytes in hand first (-ENOTBLK, in_hand gone), no -ESTALE retry after prepare. Wait for faults past their iomaps before the PTEs go (replaces 153.3/153.6's second unmap and users check). Slices go through the region their blocks moved to and fall behind.
4. guest/sim model: per-file lock held by a pending write; commits fail with fxfs_sim_fail's odds; failures.c: a failed write leaves unwritten bytes as they were; map_failed_writing expects the old bytes; mutants 150.8, 92.28, 153.3, 153.6 rewritten for the new code.
5. Prove: fxfs-posix tests; sim zloop 3000 seeds, scenarios, mutants on GCE; DAX guest suites on GCE; scripts/test.sh.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03. Built and landed (decision in OPEN_QUESTIONS.md).

Design as built:
- fxfs-posix: Fxfs::dax_begin_write makes the move in a Transaction it doesn't commit, returned as DaxWrite { transaction: Transaction<'static>, file: Arc<handle> } (the transaction declared first, dropped first; the 'static borrow never leaves the struct). DaxWrite::commit commits; drop aborts. dax_map_write and it share retrying_sealed and txn_dax_map_write. fxfs-linux: fxfs_dax_begin_write / fxfs_dax_end_write(write, commit), each its own block_on; no executor state crosses the boundary but the boxed transaction.
- dax.c, write(2) only (faults and zeroing still commit first): the loop's move is pending; the file's semaphore is held from iomap_begin to iomap_end; the commit runs in iomap_end after store_end, out of the gate (a commit may wait for compaction's seal, a seal for the gate). A seal between move and gate drops the move and goes round; a MAP failure drops it and the range falls behind; a copy cut short or a failed commit drops it, the range falls behind, and file->unwritten has write_iter cut dax_iomap_rw's count back (iomap_iter ignores iomap_end's error once bytes advanced, and begins the next iomap, which is refused). The range a drop left behind is dropped by the write itself (else its second -ESTALE was EIO). Before the PTEs go, the write waits for faults past their iomaps (wait_var_event on range->users == 1): the window page maps the new blocks before the bytes are in, and a copy from a stale PTE of it read zeros; this one wait replaces TASK-153.3's second unmap and goes_through's users check. Slices whose blocks moved go through the new region and fall behind (-EXDEV); fxfs_dax_follow_slice maps the chunk again at once, a slice of that region.
- Prepare (commit first) stays only for writes that overwrite no whole page of the file's data (appends, holes, partial pages, whose kept blocks hold the old bytes): it keeps new small files slices. Gone: bytes in hand before a move (-ENOTBLK, in_hand, fxfs_dax_source_needed), the -ESTALE retry after prepare, a write(2)'s second move after a seal, map_moved for write(2).
- Not gone: map_moved, MOVED/RESEALED, fall_behind for faults; patches/0009 (no longer needed for write(2): open question).

Measured: pgbench-dax, one host, interleaved, 5 rounds a side, 25 runs each: main 834.3 mean / 822.3 median tps, this 826.3 / 821.4; round ratios 1.024 0.960 1.033 0.974 0.964; sd 41 a side. Equal. Fxfs per write 57.6 us against 33.8 + ~25 (split, same total); 0.73% of moves dropped for a seal. docs/measurements/pgbench-dax-commit-after-bytes-2026-11-03.md.

Proof:
- crates/fxfs-posix tests: a_dax_write_dropped_leaves_the_file_as_it_was, a_dax_write_committed_has_its_blocks_and_size, a_dax_write_holds_the_file_until_it_commits.
- guest/sim: the model holds a per-file lock across a pending write, fails commits with fxfs_sim_fail's odds and fxfs_sim_fail_commits. failures' oracle tightened (the class test): a failed write's unwritten bytes are each old or the write's, never zeros; it found, before passing: in-place stores of open pages a drop can't undo (so 'old or written', not 'old'), the write's own -ESTALE budget spent by its drops (fixed: the write drops its range), and a self-write from unknown bytes (oracle). Scenarios: map_failed_writing now expects the old bytes; new commit_failed_writing (MAPped and through); slice_refuses_a_piece: the fault past its iomap, which the write now waits for, reads the old bytes. zloop 3000 seeds 0 failed (e1d89f2d8), failures 3000 seeds 0 failed, every scenario passes. Mutants rewritten for the new code: 150.8 (commit before bytes: killed by short_copy), 92.28, 153.3 (no wait), 153.6 (wait after the unmap); guest mutant table: moves_before_bytes, moved_ptes_kept, moved_write_not_retried.
- The DAX diagnostic fxfs.dax_check_moves compared bytes past EOF a mapping stored (seed 611, a punch's zeroing, any code): it stops at EOF now.
- Guest checks changed with the semantics: write_killed (the file as it was), map_many_failed (the write fails EIO and leaves the file, then succeeds). append_writable found the slice rebuild shifted to the next write: fixed by fxfs_dax_follow_slice.
- Simplification shown in guest/sim: the mutants for 79.106/119.13's -ESTALE retry and 150.8's in-hand check had no code left to put back; they were rewritten as the commit-first bug they guard against.

Structure: TASK-160.6 (per-write state on fxfs_dax_file) gets four fields of this (noted there). Pre-existing, not from this change: UBSan 'offset to null pointer' at fxfs_dax_lagging_pieces under failures 408/448 at 1000 operations, on main too.

Found and fixed on the way, each by guest/sim, in code main shares: an offset from NULL in fxfs_dax_map_found (UBSan, failures 408/448 at 1,000); fxfs.dax_check_moves comparing what a mapping stored past EOF (interleave 611); a fault's move committed before the PTEs went, a stall across two seals leaving a stale PTE that read zeros (interleave 2376 at 1,000): now every move waits for faults past their iomaps (range->faulting, not range->users: an iomap waiting for the semaphore holds a user, which deadlocked) and unmaps before; a fault taking its held bytes from the window after a move of its own, the zeros of a freed block (failures 784 at 1,000).

State 2026-11-03 18:00 UTC: everything through f4ee7b196 passed (macOS scripts/test.sh; GCE workspace 2214/2214 and fxfs-guest-dax, checks-dax, checks-dax-delay, script-full-dax, xfstests-dax; zloop 3000; failures 3000; scenarios). The commits after it (rebase onto TASK-153.15's dax.c, the fault-path fixes above) passed macOS scripts/test.sh, the scenarios and the seeds above in guest/sim, but not yet a real kernel nor a full zloop: gcloud's credentials expired (OPEN_QUESTIONS). Branch t1535-commit-after, head ae8cd3098, local, not pushed.
<!-- SECTION:NOTES:END -->
