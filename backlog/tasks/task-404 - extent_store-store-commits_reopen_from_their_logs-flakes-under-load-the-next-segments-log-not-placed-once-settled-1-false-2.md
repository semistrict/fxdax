---
id: TASK-404
title: >-
  extent_store::store commits_reopen_from_their_logs flakes under load: the next
  segment's log not placed once settled ((1, false, 2))
status: To Do
assignee: []
created_date: '2026-11-13 21:06'
labels:
  - testing
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 788000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-13 in scripts/test.sh's workspace part on t3/395-integrated (the per-cell collector's round-two merge; no extent_store store code changed), the laptop loaded by a TLC run beside it: crates/extent_store/tests/store.rs:466 assert_eq!(logs(&store), (1, true, 2)) got (1, false, 2): after settled() and prepared(), store.placed() is None, the next segment's prepared log not yet placed under its name (TASK-384). Passes 6/6 run alone right after. TASK-370 fixed another counting race in the same assertion (closed logs counted before they went). The test should wait for what it counts (the placement), not the host's speed; or the placement is not covered by settled()/prepared() as the test assumes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test waits for the placement explicitly, or prepared() returns only once the prepared log is placed
- [ ] #2 Passes 20 runs beside a loaded workspace run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
