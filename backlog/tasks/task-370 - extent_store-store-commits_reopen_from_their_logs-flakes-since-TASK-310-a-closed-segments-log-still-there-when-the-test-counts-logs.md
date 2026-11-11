---
id: TASK-370
title: >-
  extent_store::store commits_reopen_from_their_logs flakes since TASK-310: a
  closed segment's log still there when the test counts logs
status: To Do
assignee: []
created_date: '2026-11-11 23:29'
labels:
  - extentd
  - flake
dependencies: []
priority: medium
type: bug
ordinal: 753000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main CI 43117460854 (0e044dea5, the first full CI with TASK-310's e2cbf7ec4), macos (workspace): crates/extent_store/tests/store.rs:442 assert_eq!(logs(), (1, 1)) got (2, 1): two live logs after 200 commits, where the open segment's alone is expected (plus one spare). Passes 6 of 6 locally on macOS. e2cbf7ec4 makes framed segments durable together on a thread of the store's, off their framings; a closed segment's log presumably goes only once that thread has made it durable, so the test, counting at once, races it. Decide which is right: the test waiting for the store to settle (a quiesce/flush of framings before counting), or the store letting a closed segment's log go before commit returns, as it did; whichever, a test that pins the order.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 commits_reopen_from_their_logs passes 200 of 200 runs on macOS and Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
