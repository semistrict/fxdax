---
id: TASK-79.80
title: >-
  A segment's framing thread panics if the OS refuses a thread, and poisons the
  store
status: To Do
assignee: []
created_date: '2026-10-23 06:12'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 338000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.42 (round 2a), whose flush syncs now fall back when a thread is refused. Framing's thread::spawn (crates/extent_store/src/segments.rs, closing a segment) panics if the OS refuses a thread (EAGAIN under a pid or task limit, plausible on a host running hundreds of stores), and the panic unwinds through locks the store holds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A refused framing thread fails the closing (or frames inline) and leaves the store usable or failed cleanly, never poisoned
- [ ] #2 A Shuttle or unit test refuses the thread and checks the store's state
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
