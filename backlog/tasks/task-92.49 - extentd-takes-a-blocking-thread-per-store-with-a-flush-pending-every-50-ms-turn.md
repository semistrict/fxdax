---
id: TASK-92.49
title: >-
  extentd takes a blocking thread per store with a flush pending, every 50 ms
  turn
status: To Do
assignee: []
created_date: '2026-10-23 13:18'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 349000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing the thread fix (TASK-79.2, review-r1). extentd's flush_behind (crates/fxdax/src/extentd/server.rs) still takes a tokio blocking thread for every store whose flush is pending on each 50 ms turn, as main did before TASK-79.2. With hundreds of writing stores the blocking pool grows to one thread per store, each idle 10 s after. Capping flush concurrency per daemon (or max_blocking_threads on the IO runtime) would bound it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The daemon's threads stay within a fixed bound with hundreds of stores flushing at once, shown by a test like serves_hundreds_of_forks_of_a_base for writers
- [ ] #2 Flushes keep their latency under that bound, measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
