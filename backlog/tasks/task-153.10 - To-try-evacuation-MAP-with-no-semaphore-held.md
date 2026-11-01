---
id: TASK-153.10
title: 'To try: evacuation MAP with no semaphore held'
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
