---
id: TASK-149.15
title: 'Fail each allocation, MAP and UNMAP in turn in guest/sim, under ASan'
status: To Do
assignee: []
created_date: '2026-11-01 16:53'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 532000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fork sweep failed each step in turn and found 7 bugs. TASK-44.10 injects at a probability on real kernels, which samples; this enumerates, in a process, with seeds, on the paths two audit findings and TASK-99 lived on. Would find the double free, the unmount panic and stale ranges after a failed MAP.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 guest/sim fails the Nth allocation, MAP, UNMAP and device call for every N a scenario reaches, one-shot
- [ ] #2 Each run is checked by ASan, dax.c WARNs and the file model
- [ ] #3 A past bug of this class, put back, fails it
- [ ] #4 A hunt has run, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
