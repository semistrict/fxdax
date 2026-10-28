---
id: TASK-111.4
title: >-
  fxdaxctl stats --json: one consistent snapshot of extentd, VMM and guest
  counters, each with its build commit
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 418000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team diffs Prometheus scrapes, /proc/fs/fxfs/root and per-thread /proc CPU separately to get per-phase counters (TASK-111); the reads are not taken at one moment and do not say which build produced them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One command prints extentd, VMM and guest counters as JSON from one read
- [ ] #2 Each source carries its build commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
