---
id: TASK-111.6
title: >-
  Latency distributions for guest fault, seal, evacuation and window MAP, not
  only sums
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 420000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Guest-side fault, seal, evacuation and MAP timings exist only as totals, so tails and shifts within a run are invisible (TASK-111). extentd already has request histograms.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Guest fault, seal, evacuation and MAP latencies are exported as histograms
- [ ] #2 The bench JSON (see the pgbench entry point task) includes their percentiles per run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
