---
id: TASK-98
title: >-
  fxdaxctl import deadlocks: tokio workers blocked in Store::begin_write while
  commit_locked waits for writes in progress
status: Done
assignee: []
created_date: '2026-10-24 20:05'
updated_date: '2026-10-24 21:10'
labels:
  - bug
dependencies: []
priority: high
ordinal: 384000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, fxdax at ae54e01: the second pass's fxdaxctl import of the base image hung ~3 h after 147 of 455 MB. All-thread stacks (their box: ~/fxdax-evidence/import-hang-ae54e01-stacks.txt): the commit thread holds the store's write gate in extent_store::Store::commit_locked, waiting for writes in progress to drain; every tokio worker, Fxfs's journal flush task among them, is parked synchronously on a condvar in Store::begin_write, waiting for that gate. Intermittent: the first pass's import, same binary, worked. A synchronous wait inside async tasks: the writes that would drain can't run.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test drives a commit holding writes back while a write of the same task is in progress, and shows the hang
- [x] #2 Fixed, and the test passes on macOS and Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The image device no longer blocks its task's thread in Store::begin_write while a commit holds writes back (try_begin_write, waiting off the thread): the import deadlock. Pinned by writes_held_back_by_a_commit_leave_their_own_task_running, red with the old wait, green on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
