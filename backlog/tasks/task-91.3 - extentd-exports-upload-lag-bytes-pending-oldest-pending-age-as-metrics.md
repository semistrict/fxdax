---
id: TASK-91.3
title: 'extentd exports upload lag (bytes pending, oldest pending age) as metrics'
status: To Do
assignee: []
created_date: '2026-10-22 18:22'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 268000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Durable commits wait for everything pending upload ahead of them. The sandbox team can't see whether the uploader keeps up at 25 ms object-store latency (550-930 MB written per 60 s pgbench run) and asks for upload lag as a metric.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The metrics endpoint exports bytes pending upload and the age of the oldest pending, per store
- [ ] #2 Through three 60 s pgbench runs at 25 ms, the lag stays bounded (measured)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
