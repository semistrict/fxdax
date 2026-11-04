---
id: TASK-160.5
title: 'Guest DAX: diagnostics out of the protocol code'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 07:34'
labels:
  - structure
dependencies:
  - TASK-164
parent_task_id: TASK-160
priority: low
type: task
ordinal: 576000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 900 of dax.c's lines are diagnostics (shadow, trace, move checks, seal verification, stray accesses) woven into the protocol: iomap_write keeps before/before_at/before_where beside held. The diagnostics have had bugs of their own: TASK-119.21, fb440d189, 19031ef69, 119.3. Proposal: dax_diag.c behind a few hooks gated by static keys; held stays (it is protocol, TASK-79.11).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 iomap_write holds no diagnostic state
- [ ] #2 checks-dax passes with the diagnostics on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: waits for TASK-164 (read-only DAX), which changes what this covers; re-scope it then.
<!-- SECTION:NOTES:END -->
