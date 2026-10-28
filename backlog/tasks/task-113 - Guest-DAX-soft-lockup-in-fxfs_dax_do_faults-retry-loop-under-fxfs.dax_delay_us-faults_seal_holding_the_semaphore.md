---
id: TASK-113
title: >-
  Guest DAX: soft lockup in fxfs_dax_do_fault's retry loop under
  fxfs.dax_delay_us (faults_seal_holding_the_semaphore)
status: To Do
assignee: []
created_date: '2026-10-28 04:53'
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
- [ ] #2 fxfs_dax_do_fault cannot busy-loop without yielding; fxfs-guest-checks-dax under fxfs.dax_delay_us=200 passes 5 rounds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
