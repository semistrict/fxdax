---
id: TASK-153.4
title: 'To try: one owner of a file size'
status: To Do
assignee: []
created_date: '2026-11-01 22:52'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 548000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
i_size, Fxfs size, growing and range capacity: four places, explaining TASK-92.28, 133, 147, 79.28, 150.9, 150.15 and 150.19. Buffered writes rely on the inode being ahead of Fxfs until writeback, so it needs a design first: who owns the size at each moment, and the one path every non-DAX growth takes to zero the tail.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A design says who owns the size at each moment
- [ ] #2 Decided, from the design: do it or say why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
