---
id: TASK-330.4
title: >-
  Pin-to-pin A/B: the qualification on the previous pin LangSmith runs and on
  the candidate, interleaved, with a report of every metric that moved
status: To Do
assignee: []
created_date: '2026-11-10 14:00'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: medium
ordinal: 733000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
LangSmith bumps pins (6a513509a to 96c7808b8 went bad). Before a pin is offered, run the topology run and the budgets on both, interleaved (scripts/bench-ab.py with fxjob --bench), and publish what changed: disk bytes, memory, latencies, failures. A store-format bump that stops the old kernel reading new stores (Invalid future version 63.0 > 62.0) needs fresh stores per side.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Run once for 6a513509a vs current main, report in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
