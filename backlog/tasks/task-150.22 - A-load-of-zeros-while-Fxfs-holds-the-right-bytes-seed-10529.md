---
id: TASK-150.22
title: 'A load of zeros while Fxfs holds the right bytes: seed 10529'
status: To Do
assignee: []
created_date: '2026-11-02 05:15'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 568000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim, on the base of TASK-153.3, not seen at its head in 15,000 seeds, undiagnosed. A zero read is data loss if real: find out whether open-once fixed it or moved it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Diagnosed, or shown fixed by what changed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
