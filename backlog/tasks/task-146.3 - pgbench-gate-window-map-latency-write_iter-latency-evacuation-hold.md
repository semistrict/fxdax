---
id: TASK-146.3
title: 'pgbench gate: window map latency, write_iter latency, evacuation hold'
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: medium
ordinal: 499000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their ask 4. /proc/fs/fxfs has window_maps_ns, window_map_requests and dax_evacuate_hold_ns already; write_iter latency it hasn't.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/dax-gate.sh reports all three and fails on a regression
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
