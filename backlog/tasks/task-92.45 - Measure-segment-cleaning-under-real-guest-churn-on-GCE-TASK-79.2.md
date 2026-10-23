---
id: TASK-92.45
title: Measure segment cleaning under real guest churn on GCE (TASK-79.2)
status: To Do
assignee: []
created_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: task
ordinal: 329000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.2's cleaner (round 1) copies the live remainder out of mostly-dead segments behind the commits, and extentd runs a pass when Store::cleaning_due. It was measured only on macOS (APFS, F_NOCACHE), not on a network persistent disk where framing and fsync cost more, and never under a real guest: how often cleaning is due under pgbench and fs-bench, what a pass costs the guest's requests, and what a write burst past a segment's bound costs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench and fs-bench on GCE report how often cleaning runs, its copies, and the guest's request latency while it runs
- [ ] #2 The numbers are compared with main before TASK-79.2
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
