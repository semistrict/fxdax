---
id: TASK-330.3
title: >-
  Degraded-dependency soak: 30+ min against a slow bucket, then a failing one,
  then recovery; memory bounded, errors surfaced, every commit uploaded
status: To Do
assignee: []
created_date: '2026-11-10 14:00'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 732000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A fault-injecting bucket (latency, ENOSPC, 5xx) behind extentd for a long run, with guests writing all the while. Assert: memory.peak under budget; failures in the stats line and metrics (TASK-314); commits wait at the lag bound and never fail; everything uploads within the bound once the bucket answers; no VM let go (TASK-249).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Passes on main; fails with TASK-314's error-dropping or TASK-317's per-attempt copy put back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
