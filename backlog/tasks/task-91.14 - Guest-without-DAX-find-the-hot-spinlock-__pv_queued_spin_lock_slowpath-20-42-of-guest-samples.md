---
id: TASK-91.14
title: >-
  Guest without DAX: find the hot spinlock (__pv_queued_spin_lock_slowpath
  20-42% of guest samples)
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 00:46'
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
- [x] #1 The lock is identified with a guest profile with call graphs
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

Lock named with lock_stat (vmlinux-lockstat, fxfs-guest-pgbench-locks, no DAX, synchronous_commit=off, 60 s, GCE r63): rq->__lock 66,680 contentions, 1.21 s total wait (try_to_wake_up 54.6k, __schedule 46.6k); p->pi_lock 10,513 / 0.19 s (try_to_wake_up); futex hash 7,114 / 0.003 s; workqueue pool->lock 1,752 (queue_work). The scheduler's runqueue lock, contended by cross-CPU wakeups: Postgres's futexes and Fxfs's completions handed to workqueues. Here ~1% of the 2 vCPUs' time. The sandbox team's 20-42% fits lock-holder preemption on an overcommitted host: a pv spinlock waiter halts while the holder's vCPU is descheduled, each wait then a host scheduling slice. To ask them: vCPUs per host CPU. Ours to cut: cross-CPU wakeups per completion in Fxfs's runtime (queue_work per completion).

Sandbox team (via the user, 2026-10-23): their hosts are slightly overcommitted at times, which fits lock-holder preemption making rq->__lock's waits long there.
<!-- SECTION:NOTES:END -->
