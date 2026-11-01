---
id: TASK-149.16
title: Hostile arguments in the DAX scripts
status: To Do
assignee: []
created_date: '2026-11-01 16:53'
labels: []
dependencies:
  - TASK-149.2
parent_task_id: TASK-149
priority: high
type: task
ordinal: 533000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.2 scripts are well-behaved callers: their buffers never fault, sources never overlap targets, and sizes are rarely unaligned at a short stop. The model oracle would have seen the short write bugs had the generator made these inputs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Scripts write from buffers that fault partway, and from mappings of the same file, same offset and overlapping
- [ ] #2 Scripts stop writes short at unaligned sizes, then go on
- [ ] #3 A write whose source fails is checked against the model at once
- [ ] #4 A short write bug, put back, fails it, shrunk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
