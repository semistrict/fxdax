---
id: TASK-43.8
title: 'extentd: the cross-process store lock waited for without a thread'
status: To Do
assignee: []
created_date: '2026-10-18 15:22'
labels: []
dependencies:
  - TASK-43.6
parent_task_id: TASK-43
priority: low
type: feature
ordinal: 144000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-43.6 made the control API's waits futures, but one stays blocking: extent_store's Dir::take polls the store directory's flock for up to 10 s while another process (a local fxdaxctl) holds it, on a blocking thread. Doing it without a thread needs extent_store to take the lock without waiting (an Options::take_wait, 0 for the daemon, which then retries with an async sleep); that changes extent_store's Options, whose struct literals in its tests (views.rs, shuttle.rs) the follower work (TASK-42) was changing, so it was left until that settles. A probe of the lock from fxdax cannot replace it: the daemon's own open stores hold the same lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening or deleting a store another process holds waits for the lock holding no thread, up to the same 10 s, then EBUSY
- [ ] #2 extent_store's callers that wait keep waiting as before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
