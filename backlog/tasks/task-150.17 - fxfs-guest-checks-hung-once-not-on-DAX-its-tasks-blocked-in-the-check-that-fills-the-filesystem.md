---
id: TASK-150.17
title: >-
  fxfs-guest-checks hung once, not on DAX, its tasks blocked in the check that
  fills the filesystem
status: To Do
assignee: []
created_date: '2026-11-01 17:18'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 538000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
r398: after store_past_end, guest-agent, kswapd0 and kworkers blocked for over 10 s, vCPUs idle, 180 s timeout, no panic. Passed in r395, r396 and r397 on near the same code. The console has no stacks of the blocked tasks, so the hang didn't diagnose itself. Results in /tmp/fxdax-r398-scripts. Hypothesis, unproven: a commit stalled on max_lag_bytes waiting for an upload that needs the next commit (TASK-150.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The hang is reproduced, or a test shows its cause
- [ ] #2 Fixed, the test red with the fix taken out
- [ ] #3 A hung guest's blocked tasks' stacks reach the results
- [ ] #4 The notes name the blind spot and what a search around it found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
