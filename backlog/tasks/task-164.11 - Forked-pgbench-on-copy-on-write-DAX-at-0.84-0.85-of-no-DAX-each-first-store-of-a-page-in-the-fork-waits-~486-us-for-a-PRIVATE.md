---
id: TASK-164.11
title: >-
  Forked pgbench on copy-on-write DAX at 0.84-0.85 of no DAX: each first store
  of a page in the fork waits ~486 us for a PRIVATE
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 04:48'
updated_date: '2026-11-06 23:23'
labels:
  - performance
  - dax
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 644000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
After TASK-164.10 the perf gate's forked-dax is 0.836 (GCE r650: 1,217 against 1,455 tps) and 0.854 (r650's rerun: 1,216 against 1,424), against its 0.9. What is left (run 2 of dax-forked, a minute): 26,068 PRIVATEs, nearly all of them pgbench's data pages' first stores in the fork (the accounts table's 164k pages are rewritten every ~5 minutes on average, the run is 5 minutes, so private_max can't keep them; 36,910 pages of the log made private ahead take ~600 more). A PRIVATE costs the guest ~486 us: the VM process waits 320 us for the daemon's reply (vm-private-request-ns), of which the daemon takes 83 us and the VM process's own run of ~3.9 pages 135 us (stage-private-ns), 89% of the runs over pages installed (an mmap over them, so a TLB flush of every vCPU, nested); ~100 us between the VM process and the daemon, ~165 us in the VMM's virtqueue path, which every request takes (a WRITE is 59 us in the daemon, ~385 us in the guest). The vCPUs are 84% busy against no DAX's 91%. Also seen: window_full and reclaims 124 a minute with half the 2 GiB window free (its largest free run ~1.1 MiB). Options, unmeasured: the VMM's request path (both ways, every request); a private page made by the host kernel's own copy on write (the read-only arenas mapped MAP_PRIVATE, a PRIVATE a write fault in the VM process, no mmap, a rebind MADV_DONTNEED), where it is an mmap over the run today; the private pages' pre-fault off the critical path; private_max, against the density gate (DAX at 0.862 of no DAX's memory a VM in r632).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 forked-dax 0.9 or better in the perf gate's single phase on GCE, the density gate still passing
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure where a fork's PRIVATE goes on GCE, host side (scripts/extent-transport.bt).
2. Try the cheap levers: guest halt polling, guest spinning for the reply, private_max.
3. Take the PRIVATEs off the file's lock: a write(2) makes its pages private before the inode's lock (dax.c), sim scenario first.
4. Full perf gate on GCE; sim zloop, sweep, mutants; dax-gate.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: the user accepted ~0.84 (OPEN_QUESTIONS; the gate's forked-dax is 0.8), so this is later work, its AC (0.9) a goal, not the gate. The two options that needn't add complexity are subtasks of their own: TASK-164.11.1 (the VMM's request path, ~165 us of every request) and TASK-164.11.2 (a page made private without an mmap over its run, ~135 us a run). Left here, unmeasured: the private pages' pre-fault off the critical path, private_max against the density gate, the window's fragmentation.

2026-11-06. What the fork's gap was: not a PRIVATE's time alone, but how many there were and the lock they held. A window kept a quarter of its pages private at most (512 MiB of 2 GiB), less than pgbench's accounts table (640 MiB), so pages went back to their extents and their next stores asked again (r802: 309,608 PRIVATEs); and a write(2)'s PRIVATE held the inode's lock, which the file's other writes waited for as long as the PRIVATEs took (160 s of lock waits for 163 s of PRIVATEs). TASK-164.11.1's measurement is in its notes: the transport's ~245 us of a PRIVATE are nested exits, which halt polling and spinning didn't change.

Changes: (1) vendor/firecracker b7c332c52: private_max half the window's pages by default (decision in OPEN_QUESTIONS.md: twice the reservation against the tenant's arena), test a_window_holds_half_its_pages_private_at_most, red with a quarter (131072 against 262144). (2) dax.c: a write(2) makes the pages it writes private before taking the inode's lock (fxfs_dax_private_before), as a fault does, holding the invalidate lock and the file's semaphore shared; not a log's following writes, which keep their pages ahead; a failed PRIVATE fails the write as before (fxfs_dax_private_failed, one function for both paths). Test: guest/sim cow_writes_private_before_the_lock, red without (a write to a private page waited for another write's PRIVATE). (3) A bug the new schedule found: a range whose MAPs failed part way was UNMAPped only as its pages said, leaving what a MAP_MANY's earlier requests mapped (guest/sim failures seed 972 at 200): it is UNMAPped whole now (range->unsure); test range_map_failed_part_way, red without, mutant task-164.11-unsure-range-unmapped-as-counted killed by it.

Measured (docs/measurements/dax-cow-gate.md): r804, half the window private: forked-dax 0.912, PRIVATEs 108,006; r805 its density phase: density-memory 0.823, 2,256 MiB a VM added. r806 full gate, half alone: 0.893. r807 full gate, both: forked-dax 0.912 (dax-forked 1203, 1328, 1300, 1442, 1296 against no-dax-forked's 1425 median), dax 0.995, density-memory 0.811, density-memory-mib 2,164.0, every gate passing; lock waits 7.4 s from 37. Checks: guest/sim zloop (every test, 3,000 seeds each of interleave and failures, 0 failed), the full sweep (41 runs, 16,142 calls failed in turn, 0 failed), mutants.sh (35 killed, 6 none; three re-cut against fxfs_dax_private_failed), guest-mutants-gce.py --check (46), the fork's extent device and config tests (Docker; the 8 worker tests that fail there fail without the change too, EIO opening the store in the container), upstream-changes.py --check.

Not of this task: the fork's extent worker tests (vendor/firecracker, vmm --lib extent) fail 8 of 21 on main as here, EIO opening a store from a daemon of their own (store_of), in Docker and on GCE r800 alike, with private_max's change taken out too; CI doesn't run them.
<!-- SECTION:NOTES:END -->
