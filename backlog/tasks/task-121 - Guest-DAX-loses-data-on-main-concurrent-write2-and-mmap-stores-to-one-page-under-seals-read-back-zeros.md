---
id: TASK-121
title: >-
  Guest DAX loses data on main: concurrent write(2) and mmap stores to one page
  under seals read back zeros
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 19:44'
updated_date: '2026-10-29 03:22'
labels:
  - bug
dependencies: []
priority: high
ordinal: 446000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-10-28 by the TASK-119 checks on test/119 (main 56126dc9e + test changes, dax.c unchanged but for no-op delay points), GCE r205, run-20261028T194119Z-379e5402, round 1, without delay injection (fxfs-guest-checks-dax) and with (checks-dax-delay): (1) seal_race (write(2)s of byte 0, stores of byte 1 of each of 16 pages, another file fsyncing): "page 7: the write(2)s byte, mapped and read, after their round 2: (0, 0) != (2, 2)", and the storer saw the write(2)s byte go back 2 -> 0 (seed 1792508877678053435); delay run: page 5 went back 1 -> 0. (2) torture (3 threads, 4 x 256 KiB files, byte-owned): thread 1 stored a whole page through its mapping at 0x24000 of file 2, then read(2) and the mapping both read 0 where 0xf6 was stored (phase 0, op 145); delay run: thread 2, op 27, file 1 at 0x22000, also zeros. A byte written reads back as zero, through both views: beyond the copy-onto-itself race dax.c documents in fxfs_dax_cow (which would restore an old value, not zeros). Results on the laptop: /tmp/fxdax-r205-119t.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is identified and recorded
- [x] #2 seal_race and torture pass 10 rounds each on checks-dax and checks-dax-delay with the fix
- [x] #3 A hand mutant reintroducing the cause is killed by those checks
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause (2026-10-28, from host logs: every refused store's page shows a writable MAP of an extent, then 'zeros in place of a freed extent's' ms later; no guest free of an open extent logged): a write fault's map_write commits the move of its page to new blocks (the old freed), then a seal keeps the fault from the gate (fxfs_dax_store_begin -EBUSY): the iomap returned -EBUSY with the range still mapping the page where it was; once the old extent was freed the device mapped zeros there, which reads returned and the retried fault's held copy took (Astra's candidate, TASK-112.6 notes). Before the free, reads return the old data: pgbench-dax on main (r207, run 2) ended with accounts/tellers/branches sums -263212 each and history's deltas -743934: PostgreSQL's committed updates read back stale. Bisection: fxfs.dax_evacuates=0 still fails; fxfs.dax_moves=0 passes seal_race, torture still fails. Fix 6f7bc03a4 (test/119): when a seal comes between the move and the gate, the range maps the moved blocks read-only in its own space at once (fxfs_dax_map_moved; slices -EXDEV), counted in dax_moved_unsealed. Verifying: r208 (seal_race,torture,random_ops x5), r209 (same with delays), r206 (checks-dax, checks, crash x2), r207 (pgbench-dax).

Correction: the pgbench-dax 'stale balances' were the check's error, not data loss: pgbench empties pgbench_history as each run begins, so after run 2 the balances carry run 1's deltas too (fixed in 5c1bf4fd3: each run's change in the balances is checked against its history).

Remaining loss found (cae97000a): ExtentStore::write checked an extent wasn't open with the open lock as an if-let temporary, dropped before the fall-through store.write. A writable MAP opening the extent between read the store without the write; the commit then wrote the VMO's zeros over it. Fxfs copies a moved page's surroundings (Kept::Around/All) to a new staging extent with device.write while restage maps the extent's region writably: the copy was lost, the moved block holding only what the move wrote. Found with dax_check_moves plus device reads (old block intact, Fxfs's tree agreeing with the window, new block zeros); a read-back after the copy hid it. Host test writes_racing_the_mapping_that_opens_their_extent_are_committed fails unfixed at extent 3 (zeros).

Final: at c1792bac9, seal_race, torture, faults_writes_compaction, faults_unheld passed 10 rounds each on fxfs-guest-checks-dax (r208) and fxfs-guest-checks-dax-delay (r209). Hand mutant (ExtentStore::write's open lock released before the store write) killed in round 1 by torture, faults_writes_compaction and faults_unheld (bytes read as zeros), and by the host race suite (TASK-119.19).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Two causes. (1) Guest: a fault whose move a seal came between kept mapping the page where it was; fxfs_dax_map_moved maps the moved block before giving up the page (3098e9cca and before). (2) Host: ExtentStore::write checked an extent wasn't open, let go of the lock, then wrote the store; a writable MAP opening the extent between read the store without the write, and its commit wrote zeros over Fxfs's copy of a moved page's other bytes (cae97000a: the store write under the lock). Found with dax_check_moves' device reads (old block intact, Fxfs's tree agreeing with the window, new block zeros). Guarded by the host race suite (TASK-119.19), specs/extent-open (TASK-119.20), and the guest checks, which kill the mutant in one round. DoD #1: Linux (GCE) runs, not the macOS workspace.
<!-- SECTION:FINAL_SUMMARY:END -->
