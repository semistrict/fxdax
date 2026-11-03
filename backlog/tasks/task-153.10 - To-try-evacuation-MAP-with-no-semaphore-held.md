---
id: TASK-153.10
title: 'To try: evacuation MAP with no semaphore held'
status: Done
assignee: []
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 554000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-112.5, parked for a race the host bug cae97000a may have been: +2-3.5% then. r453: writes wait 83 s of 403 s behind evacuations, and readers take the same semaphore. Rerun branch wip/task-112-5 against the race on today tree, guest/sim checking it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Rerun, with delay injection and guest/sim
- [ ] #2 Landed or the dead end updated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded: evacuation is gone. fafefbac7 (TASK-153.6) keeps a file's page at one window address, so nothing moves pages back after a seal, and 106b6128d takes a write's own MAP out; nothing under guest/ mentions evacuation on main. Nothing of this task was built.
<!-- SECTION:FINAL_SUMMARY:END -->
