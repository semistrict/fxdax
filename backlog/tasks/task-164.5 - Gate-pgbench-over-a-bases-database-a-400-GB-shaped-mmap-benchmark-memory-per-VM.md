---
id: TASK-164.5
title: >-
  Gate: pgbench over a base's database, a 400 GB-shaped mmap benchmark, memory
  per VM
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 615000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The density gate measures memory per VM, from which the user's density targets are set (2026-11-04).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cases in perf-gate-gce.sh and pgbench-gate.py with thresholds
- [ ] #2 A run recorded in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
