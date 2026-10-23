---
id: TASK-91.14
title: >-
  Guest without DAX: find the hot spinlock (__pv_queued_spin_lock_slowpath
  20-42% of guest samples)
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-23 22:26'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 352000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: in the default no-DAX run __pv_queued_spin_lock_slowpath was 20% of guest samples, 42% with synchronous_commit=off where the guest is CPU-bound; the sandbox team's sampler has no stacks. A guest perf with call graphs on that run would say which lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The lock is identified with a guest profile with call graphs
- [ ] #2 Its contention cut, or why it can't be stated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxfs-guest-pgbench-profile: no-DAX pgbench whose first run is profiled in the guest (perf record -e cpu-clock -a -g), reporting time by symbol and the call chains into __pv_queued_spin_lock_slowpath. 2. Run it on main with TASK-91.12 (writeback no longer whole folios). 3. Cut that lock's contention.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxfs-guest-pgbench-profile (70d9f65c), perf record -e cpu-clock -a -g in the guest, no-DAX run 1 on GCE r63: __pv_queued_spin_lock_slowpath under 1% of samples both before TASK-91.12 (vmlinux-m: halt 34.5%, _raw_spin_unlock_irqrestore 2.6%) and after (drv2: halt 24.6%, _raw_spin_unlock_irqrestore 2.9%). Not reproduced in our harness at synchronous_commit=on. The guest has no PMU: cpu-clock samples are hrtimer interrupts, which can't land while interrupts are off, so time in spin_lock_irqsave slow paths is charged to the unlock that re-enables them. Next: the sandbox team's synchronous_commit=off run (42% there), with a lock_stat kernel (CONFIG_LOCK_STAT) to name the lock by contention rather than samples.
<!-- SECTION:NOTES:END -->
