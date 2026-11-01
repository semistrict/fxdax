---
id: TASK-149.19
title: The suites under CPU starvation
status: To Do
assignee: []
created_date: '2026-11-01 16:54'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 536000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
SIGTERM killed extentd in 216 of 600 runs under 40 CPU hogs, and once in six full runs otherwise. TASK-44.18 shapes guests; this starves test processes, so a thread not yet scheduled stays so.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts run cargo nextest with more busy threads than processors, seeded by nothing but time
- [ ] #2 It runs on demand on a GCE host, and its failures are filed
- [ ] #3 The SIGTERM bug, put back, fails it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
