---
id: TASK-79.12
title: >-
  A fault holding a DAX entry waits on file->sem, which another fault holds
  across Rust calls that wait for a seal: the whole filesystem hangs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-27 14:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 200000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The cycle: a write(2) in the gate waits for a fault's DAX entry; that read fault waits on file->sem; a write fault holds file->sem across fxfs_dax_map_write, which waits for compaction; compaction's seal waits for the write(2). After that every store_begin blocks, breaking device.h:85's rule that stores never wait for Rust.

Where: guest/linux/fs/fxfs/dax.c:1273, guest/linux/fs/fxfs/dax.c:1571, guest/linux/fs/fxfs/dax.c:1610, guest/linux/fs/fxfs/dax.c:1416, guest/linux/fs/fxfs/dax.c:473, guest/linux/fs/fxfs/device.h:85.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.2, TASK-58. No task covers the file->sem / DAX entry / compaction seal deadlock. TASK-58 (To Do) saw postgres tasks hung over 120 s before its reservation panic: possibly this cycle, unconfirmed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Faults take file->sem only by trylock (fxfs_dax_down_read/down_write, iomap_read, iomap_write, get_range, prepare_write): -EDEADLK, and fxfs_dax_do_fault waits for the semaphore without the DAX entry and tries again.
2. Fault injection fxfs.dax_fault_seals: a write fault making room seals first, holding the semaphore, as map_write waiting for compaction does.
3. Check faults_seal_holding_the_semaphore: write(2)s into holes, read faults on the page being written, sealing write faults. Run on a kernel with the injection and without the fix (expected hang), and with the fix.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24 attempt, reverted: DAX write(2) held the invalidate_lock (exclusive) across dax_iomap_rw, so no fault holds a DAX entry the write waits for in the gate. On GCE fxfs-guest-crash-dax then hung at its first step (no output in 15 s); bisected to the write_iter change (without it crash-dax passes). Not diagnosed further. The check faults_writes_compaction (on main with this batch) passes on old and new kernels alike: the cycle needs compaction to land during a fault, rare.

r150 (GCE, exp/79.12-repro 5cc3bc3d2: main with fxfs.dax_fault_seals and the check, without the fix): faults_seal_holding_the_semaphore hung within 10 s, guest-agent's threads and two kworkers blocked, a write I/O request blocked 120 s, the runner's timeout. AC1 met.
<!-- SECTION:NOTES:END -->
