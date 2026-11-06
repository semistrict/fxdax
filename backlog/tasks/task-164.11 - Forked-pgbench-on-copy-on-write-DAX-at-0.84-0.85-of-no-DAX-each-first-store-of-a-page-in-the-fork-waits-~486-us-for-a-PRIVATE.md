---
id: TASK-164.11
title: >-
  Forked pgbench on copy-on-write DAX at 0.84-0.85 of no DAX: each first store
  of a page in the fork waits ~486 us for a PRIVATE
status: To Do
assignee: []
created_date: '2026-11-06 04:48'
updated_date: '2026-11-06 06:27'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: the user accepted ~0.84 (OPEN_QUESTIONS; the gate's forked-dax is 0.8), so this is later work, its AC (0.9) a goal, not the gate. The two options that needn't add complexity are subtasks of their own: TASK-164.11.1 (the VMM's request path, ~165 us of every request) and TASK-164.11.2 (a page made private without an mmap over its run, ~135 us a run). Left here, unmeasured: the private pages' pre-fault off the critical path, private_max against the density gate, the window's fragmentation.
<!-- SECTION:NOTES:END -->
