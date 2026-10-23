---
id: TASK-91.17
title: >-
  extentd: segment closes land inside the no-DAX load's fsync commits (~41 ms
  each)
status: To Do
assignee: []
created_date: '2026-10-23 18:01'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 355000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: without DAX, pgbench -i's 206 fsync commits average ~41 ms, 6.34 s of their 8.37 s store-commit phase closing segments; with fsync=off the load takes 12.4 s (their runtime 12.3 s). TASK-41 framed closed segments off the ordered commit, yet after the load's large writes the closes still land in the commit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No-DAX load within 10% of its fsync=off time (12.4 s on their host)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
