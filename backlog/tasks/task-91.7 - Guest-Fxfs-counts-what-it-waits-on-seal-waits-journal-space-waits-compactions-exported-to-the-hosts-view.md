---
id: TASK-91.7
title: >-
  Guest Fxfs counts what it waits on: seal waits, journal-space waits,
  compactions, exported to the host's view
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-23 10:15'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 272000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs waits on the device in block_on, so the guest reports those waits as idle with zero iowait; the sandbox team can't attribute fxdax's 17 s of extra idle in the load step from outside.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Seal waits (count, time), journal-space waits and compactions are readable from the guest (debugfs or /sys)
- [ ] #2 fs-bench reports them per step
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
263a3c34: /proc/fs/fxfs/<tag> (device requests, seal waits and holds, writes and DAX stores held by seals, journal-space waits, compactions, window MAP/UNMAP time, window full/reclaims/regions). fxfs-guest-pgbench prints them after each step; fs-bench doesn't yet (AC2 open).
<!-- SECTION:NOTES:END -->
