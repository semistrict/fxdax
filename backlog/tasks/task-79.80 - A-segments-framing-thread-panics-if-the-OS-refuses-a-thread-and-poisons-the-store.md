---
id: TASK-79.80
title: >-
  A segment's framing thread panics if the OS refuses a thread, and poisons the
  store
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A refused framing thread fails the closing (or frames inline) and leaves the store usable or failed cleanly, never poisoned
- [x] #2 A Shuttle or unit test refuses the thread and checks the store's state
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A segment's framing thread was started with thread::spawn, so when the host refused a thread the commit panicked while holding the store's locks and poisoned the store. Framing now goes through spawn_or_run, which frames on the committing thread when the host refuses one. That holds up the writes behind it for that time, and the store carries on. Under Shuttle, the store's thread module now refuses threads as std does, so a test can have the host refuse them and any bare spawn is caught. a_host_refusing_threads_frames_on_the_committing_thread failed with the old spawn (failed to spawn thread, then PoisonError and SIGABRT) and now passes on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
