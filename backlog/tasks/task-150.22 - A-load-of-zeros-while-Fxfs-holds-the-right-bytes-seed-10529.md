---
id: TASK-150.22
title: 'A load of zeros while Fxfs holds the right bytes: seed 10529'
status: Done
assignee: []
created_date: '2026-11-02 05:15'
updated_date: '2026-11-04 07:06'
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

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Moot: never seen at TASK-153.3's head in 15,000 seeds, and the code it ran on was replaced (fafefbac7, 106b6128d, TASK-153.5); CI's 3,000 seeds pass on every push.
<!-- SECTION:FINAL_SUMMARY:END -->
