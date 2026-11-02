---
id: TASK-150.21
title: 'A write invalidates a PMD entry a read fault just mapped: WARN at fs/dax.c:416'
status: To Do
assignee: []
created_date: '2026-11-02 05:15'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 567000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-153.3: seed 2677 of zloop, 1 in 3,000; steps with dax_pause reproduce it on the base commit too. A fix in iomap_end passes the seed but not those steps; it needs work in the fs/dax.c patch series. Also the WARN behind the weak catches of TASK-123, 125 and 79.12 in guest/sim, which nobody could explain. Steps are in TASK-153.3 notes and guest/sim/README.md on refactor/open-once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A scenario fails on the code as it stands
- [ ] #2 Fixed, red with the fix out
- [ ] #3 zloop clean at 3,000 seeds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
