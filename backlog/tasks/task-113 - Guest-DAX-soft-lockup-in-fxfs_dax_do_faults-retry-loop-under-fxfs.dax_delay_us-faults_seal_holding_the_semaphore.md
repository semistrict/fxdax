---
id: TASK-113
title: >-
  Guest DAX: soft lockup in fxfs_dax_do_fault's retry loop under
  fxfs.dax_delay_us (faults_seal_holding_the_semaphore)
status: In Progress
assignee: []
created_date: '2026-10-28 04:53'
updated_date: '2026-10-29 07:00'
labels:
  - bug
dependencies: []
priority: high
ordinal: 430000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r202 2026-10-28 (run-20261028T044814Z-ed84aee8, round 1 of fxfs-guest-checks-dax with fxfs.dax_delay_us=200; tree = main 32aeffade plus counters timing the -EDEADLK wait, which add only ktime reads): after faults_unheld, both vCPUs soft-locked 26 s then 52 s: CPU0 in fxfs_dax_do_fault+0x68 (do_read_fault), looping; CPU1 in down_read from fxfs_dax_do_fault -EDEADLK retry (fxfs_dax_mkwrite, do_wp_page). The retry loop (-EDEADLK: down_read/up_read then retry; -EBUSY; -EXDEV; -ESTALE) has no cond_resched; with PREEMPT_NONE and 2 vCPUs, a faulting task busy-retrying and another spinning on the rwsem can keep the semaphore holder (a fault sealing while holding it, dax_fault_seals) off both CPUs. Main passed 5 rounds the same way earlier that night, so it is rare or new; console at /tmp/fxdax-r202-finald on the laptop.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The livelock is explained (which task holds the semaphore, why it does not run)
- [x] #2 fxfs_dax_do_fault cannot busy-loop without yielding; fxfs-guest-checks-dax under fxfs.dax_delay_us=200 passes 5 rounds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
3c539db1b (pushed): cond_resched at the top of fxfs_dax_do_fault's retry loop. GCE r204 (2026-10-28): fxfs-guest-checks-dax under fxfs.dax_delay_us=200 passed 5/5, no soft lockup; r205: pgbench-dax, dax, checks-dax, crash-dax, follow-dax, extent-guest all pass, pgbench tps 1172/1085/1022/983/983 (= main). AC1 open: the livelock was seen once and not reproduced, so which task held the semaphore is a hypothesis (PREEMPT_NONE, both vCPUs busy-retrying), not shown. DoD1: guest C only; no workspace change.

2026-10-29, the console (/tmp/fxdax-r202-finald, round 1, after faults_unheld, so in faults_seal_holding_the_semaphore, fxfs.dax_fault_seals on): both soft-locked CPUs run threads of the check process, guest-agent 639 and 640, on the same file (R12 ffff88800520d000 in both): 640 a read fault (do_read_fault) looping at the top of fxfs_dax_do_fault's retry loop, 639 a write-protect fault (fxfs_dax_mkwrite) going round the -EDEADLK retry, down_read then up_read then iomap_iter and get_range, for 26 to 122 s. No hung-task report names a sleeper, and the console has no blocked-task dump. So a third task, the one their retries wait on (the fault that seals holding the file's semaphore, or the seal's kworker), was runnable but got no CPU: PREEMPT(none), two vCPUs, both in kernel loops that never yield, which 3c539db1b's cond_resched breaks. Which task that was can't be told from this console: it shows the loop, not the holder. A blocked-task dump at a soft lockup (TASK-44.13) would name it if it recurs.
<!-- SECTION:NOTES:END -->
