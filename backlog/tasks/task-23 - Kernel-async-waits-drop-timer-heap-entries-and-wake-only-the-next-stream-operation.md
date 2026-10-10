---
id: TASK-23
title: >-
  Kernel async waits: drop timer heap entries and wake only the next stream
  operation
status: To Do
assignee: []
created_date: '2026-10-10 14:41'
labels: []
dependencies: []
type: chore
ordinal: 23000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in the pre-commit review of TASK-16. (1) crates/zircon/src/kernel/timer.rs: a dropped Wakeup removes its waker but leaves its entry in the global deadline heap until the deadline; pager waits arm 20 s wakeups (userpager_overtime_wait_seconds) and anonymous-page waits 5 s, so under load the heap fills with dead entries, all pushed and popped under one process-wide mutex, which is also taken on every Pending poll and every drop, even for infinite deadlines. (2) crates/zircon/src/vm/stream_size_manager.rs: dequeuing an operation wakes every future waiting to be at the head of a queue, so N queued writes cost O(N^2) polls under the manager's lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Dropping a wakeup removes its deadline (e.g. a BTreeMap keyed by (deadline, id)), and infinite deadlines take no timer lock
- [ ] #2 Dequeuing a stream operation wakes only the futures whose operations may now be at the head of a queue
- [ ] #3 Tests cover a dropped wakeup leaving no heap entry, and many queued writers completing in order
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
<!-- DOD:END -->
