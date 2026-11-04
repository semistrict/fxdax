---
id: TASK-164.6
title: Copy-on-write DAX by default
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 616000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The dax-gate and perf gate pass
- [ ] #2 Writes of created files at no-DAX's tps, base first reads at today's DAX's, the forked pgbench within the gate's threshold
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
