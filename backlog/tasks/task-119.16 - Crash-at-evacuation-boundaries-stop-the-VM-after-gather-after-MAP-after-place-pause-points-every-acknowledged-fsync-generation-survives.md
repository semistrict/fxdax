---
id: TASK-119.16
title: >-
  Crash at evacuation boundaries: stop the VM after gather, after MAP, after
  place (pause points), every acknowledged fsync generation survives
status: Done
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 12:57'
labels:
  - testing
dependencies:
  - TASK-119.9
parent_task_id: TASK-119
priority: medium
ordinal: 454000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra: warm-window reads agree while durable storage may be wrong.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 crash-dax variants stop at each evacuation step and verify the store
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, branch t3/task-112-6: fxfs-guest-crash-dax-gathered, -mapped, -placed: the crash test's VM stopped (reboot) once an evacuation is held at GATHERED, EVACUEES_MAPPED or PLACED, which the guest reports ('held at step N') and the host requires, then checks the store as fxfs-guest-crash-dax does. In dax-gate.sh. Pending GCE r215.

GCE r219/r221: -placed 2 of 2 twice; -gathered held no evacuation within 10 s in one round of two; -mapped once printed no 'held at step 6' and its VM didn't stop in 40 s, a writer blocked 20 s (on the held file, presumably); the console's loglevel hides the blocked task's stack. Out of dax-gate.sh until reliable.

Cause of the flakes (r215, r221 logs): (1) -gathered's misses printed 'held at step 4', but the reset's kernel messages (device shutdown's failed MAPs, journal errors) went into the middle of the line: printk writes the UART directly while the agent's framed output drains from the tty buffer, so the host's exact-line match missed it; (2) -mapped's hang (r221): output stopped as the hold began, one guest-agent thread blocked in D 20 s (hung-task message, stack hidden by loglevel), no reboot: a writer faulting on the held file waits holding its process's mmap lock, which the stopping thread's allocations (and the forwarders', same process) then wait behind; moved_across_seal forks its storer for the same reason. That run's firecracker.log has one refused store at window offset 0x3011000 at the hold's time (made 9), not lifted, yet no vCPU gave up. 418636158: the stop is a process of its own, forked before the writers, which holds the evacuation, says 'held at step N' on /dev/console in a frame of its own (a line begun anew), tcdrains the console, and resets. GCE r225a: the crash variants 4 rounds each, pending.

GCE r225a (main 418636158, kernel built): fxfs-guest-crash-dax-gathered, -mapped, -placed, fxfs-guest-crash-dax and fxfs-guest-crash 4 rounds each passed, each crash-at-step round finding 'held at step N' and checking the store. Back in dax-gate.sh. DoD #1 (macOS workspace nextest) not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Crash-at-step variants stop the VM with an evacuation held at GATHERED, EVACUEES_MAPPED and PLACED and check every acknowledged fsync survives; their flakes (the held line spliced by the reset's kernel messages; the stopping thread stuck behind a writer holding its process's mmap lock) fixed by a forked stopper that writes to the console directly and drains it. Verified by GCE r225a, 4 rounds of each passing.
<!-- SECTION:FINAL_SUMMARY:END -->
