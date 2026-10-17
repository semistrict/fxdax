---
id: TASK-26.6.1
title: >-
  Guest Fxfs without DAX: pgbench fails, compaction runs out of metadata
  reservation
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 07:54'
updated_date: '2026-10-17 09:49'
labels: []
dependencies: []
parent_task_id: TASK-26.6
priority: high
type: bug
ordinal: 92000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On GCE, bench-fs-fxfs-guest (guest Fxfs over the extent device, no DAX window) fails pgbench in its first iteration every time (pg26, pg27, pg28; the last with a kernel built from main at 01fb67c). The journal closes with "Compaction error ... Failed to flush object store 9 ... Allocation failed ... Failed to allocate 4096 bytes for owner 3 from reservation with 104696 bytes available (borrowed metadata space: 2325952, max store reservation: 350208) ... No space: Journal flush error", after which unlinks fail with "Filesystem inconsistency" and pgbench fails. fxfs-guest-dax passes the same workload (1464 tps). 569fb5e fixed the same symptom in initdb (borrowing transactions now reserve with the allocator first); something in the non-DAX write path still drains the metadata reservation faster than compaction repays it. Logs of the failing runs were in the session scratchpad (pg26-28); reproduce with: FXDAX_GUEST_KERNEL=<x86 vmlinux> FXDAX_BENCH_CASES=bench-fs-fxfs-guest FXDAX_BENCH_GROUPS=pgbench FXDAX_BENCH_PGBENCH_SECS=20 FXDAX_BENCH_ITERATIONS=3 scripts/bench-gce.sh run <instance> <dir>.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test that reproduces the metadata-reservation exhaustion fails before the fix and passes after it
- [x] #2 pgbench on bench-fs-fxfs-guest completes 3 iterations on GCE with no journal or allocation errors in the guest log
- [x] #3 fxfs-guest-dax pgbench and the guest suites (fxfs-guest, -checks, -crash, pjdfstest) still pass on GCE
- [x] #4 The root cause is stated in the commit message and the task notes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Feedback loop: the GCE run of bench-fs-fxfs-guest's pgbench, red every time (pg29).
2. Instrument the guest's Fxfs (object_manager, tagged [DEBUG-mr1]) to log what adds to borrowed metadata space, by transaction kind, at error level, which the guest log keeps.
3. Fix what dominates. Prove it with an Fxfs test that fails before the fix, and with GCE runs.
4. Remove the instrumentation. Run the workspace on macOS and Linux, pgbench x3 with and without DAX, and the guest suites on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause, from the instrumented runs (pg31-pg33): about two thirds of the borrowed space came from the journal's own empty transactions in pre_commit. When the next commit finds more than 256 bytes between the last transaction's end and the journal's offset (needs_borrow_for_journal), it accounts for that gap in a transaction of its own. Every sync leaves that gap by padding to the 4 KiB block. Those transactions were Borrowed (with the metadata reservation as theirs), so each fsync borrowed about 16 KB (4x the padding). The rest was the journal's growth (preallocate_range_in_extents_of, 128 KiB a chunk), which upstream borrows too.
initdb's ~1000 fsyncs borrowed ~6 MB. The metadata reservation fell to ~100 KB against a 350 KB max store reservation, so compaction could not allocate. 569fb5e's Borrowing excluded skip_journal_checks transactions, which this one is.
Fix 1c6b2cf: the padding-only transaction takes an ordinary reservation from the allocator, falling back to borrowing only on NoSpace. A first attempt made it Borrowing (dropping the skip_journal_checks exclusion), but that broke test_replay_is_identical: replay counts a borrowing transaction's whole journal usage as borrowed, so a part paid from a reservation diverges. 569fb5e's Borrowing has that latent mismatch for unlinks and tombstones too; it is not exercised by tests.
Test: journal::tests::test_syncs_padding_is_reserved_not_borrowed. Before the fix the reservation fell from 796 KB to 378 KB; it no longer shrinks. Fxfs: 645 tests pass.
GCE pg34 (fixed kernel, instrumented): pgbench passes, 1812 tps, no allocation errors; the reservation stays around 12 MB.

Verification of 1c6b2cf (6571c1e after the rebase):
- macOS 1945 and Linux 1955 workspace tests pass; Fxfs alone, 645.
- GCE pg35, kernel without instrumentation: pgbench completes 3 of 3 iterations in both cases, with no allocation or compaction errors. bench-fs-fxfs-guest runs at 1512-1584 tps, bench-fs-fxfs-guest-dax at 1839-1867 tps.
- GCE guest suites with the fixed kernel (gce-mr1, host at 36c382b): extent-guest, fxfs-guest, -dax, -checks, -checks-dax, -crash, pjdfstest-fxfs-guest and -dax pass. fxfs-guest-crash-dax failed once (a synced file missing after the crash). It then passed 2 of 2 on GCE at c1b8801 and 20 of 20 in Lima, alternating kernels with and without the fix. That failure is filed as TASK-26.6.2.
- Instrumentation ([DEBUG-mr1]) removed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each sync pads the journal to its 4 KiB block. The next commit accounted for that padding in a transaction of its own, which borrowed it, four times over, from the metadata reservation. initdb's thousand fsyncs drained the reservation until the guest's large journal could not compact, and the guest's Fxfs failed without DAX. That transaction now takes an ordinary allocator reservation and borrows only when the filesystem is full, which replay accounts for exactly. Verified by test_syncs_padding_is_reserved_not_borrowed, which fails before the fix, by pgbench on GCE (3 of 3 iterations with and without DAX), by the guest suites, and by the macOS and Linux suites.
<!-- SECTION:FINAL_SUMMARY:END -->
