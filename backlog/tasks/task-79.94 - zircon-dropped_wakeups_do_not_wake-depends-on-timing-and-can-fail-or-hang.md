---
id: TASK-79.94
title: 'zircon: dropped_wakeups_do_not_wake depends on timing, and can fail or hang'
status: To Do
assignee: []
created_date: '2026-10-23 19:55'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 363000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.50's branch. crates/zircon/src/kernel/timer.rs's test dropped_wakeups_do_not_wake (around lines 252-266) failed once in a full suite run ('assertion failed: !dropped.0.is_signaled()') and passed on rerun. If the thread is descheduled 1 ms or more between wake_at and the drop, the dropped waker fires; if that happens between wake_at(replaced) and set_waker, serve() has already removed the waker, so set_waker does nothing and current.wait(infinite) hangs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test is deterministic: it controls the timer's clock or orders the steps it races, and passes 2000 of 2000 runs under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
