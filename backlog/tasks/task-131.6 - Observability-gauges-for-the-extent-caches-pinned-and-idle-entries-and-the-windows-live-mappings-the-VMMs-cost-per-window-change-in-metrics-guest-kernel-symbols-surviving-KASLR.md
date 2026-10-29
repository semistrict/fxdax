---
id: TASK-131.6
title: >-
  Observability: gauges for the extent cache's pinned and idle entries and the
  window's live mappings; the VMM's cost per window change in /metrics; guest
  kernel symbols surviving KASLR
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - observability
dependencies: []
parent_task_id: TASK-131
priority: medium
ordinal: 477000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8, the parts no task has: gauges for the extent cache's pinned and idle entries and the window's live mappings; the VMM's cost per window change in its /metrics; guest kernel symbols that survive KASLR (profiles and stacks symbolized). Latency histograms are TASK-111.6, tracepoints TASK-44.16, the build-commit stamp TASK-111.7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each gauge and the VMM's cost exported, and a guest profile symbolized under KASLR
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
