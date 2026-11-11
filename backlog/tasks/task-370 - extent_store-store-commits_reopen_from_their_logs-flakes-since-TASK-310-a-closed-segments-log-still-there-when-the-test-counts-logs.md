---
id: TASK-370
title: >-
  extent_store::store commits_reopen_from_their_logs flakes since TASK-310: a
  closed segment's log still there when the test counts logs
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 23:29'
updated_date: '2026-11-11 23:57'
labels:
  - extentd
  - flake
dependencies: []
priority: high
type: bug
ordinal: 753000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main CI 43117460854 (0e044dea5, the first full CI with TASK-310's e2cbf7ec4), macos (workspace): crates/extent_store/tests/store.rs:442 assert_eq!(logs(), (1, 1)) got (2, 1): two live logs after 200 commits, where the open segment's alone is expected (plus one spare). Passes 6 of 6 locally on macOS. e2cbf7ec4 makes framed segments durable together on a thread of the store's, off their framings; a closed segment's log presumably goes only once that thread has made it durable, so the test, counting at once, races it. Decide which is right: the test waiting for the store to settle (a quiesce/flush of framings before counting), or the store letting a closed segment's log go before commit returns, as it did; whichever, a test that pins the order.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 commits_reopen_from_their_logs passes 200 of 200 runs on macOS and Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11: failed again on the rerun of main CI 43117460854's macos (workspace) (same line, (2, 1)): on GitHub's macOS runner every time so far, 0 of 6 locally. Main is red on it.

Correction (TASK-310's session, which owns this): the CI value was (1, 2), one live log and two spares, not (2, 1); the race predates TASK-310, which widened it. Its fix (test settles the store before counting) is theirs to push.

2026-11-11 (claude): fixed in a50c4a5cf. Cause: the test counted logs as its last commit returned, never settling the store. A closed segment's raw file and log go once its framed file is durable, by a flush after (TASK-165.9's framings run off the commits; TASK-310 adds the make-durable thread), and a log gone is kept as a spare, two at most (segments::SPARES): on a loaded 3-vCPU runner, the full suite in parallel, two closed segments' logs went in one flush, two spares kept. Not new with TASK-310: with framings 50 ms slow (fail point segments::framing), main at 23dfe74bf, before TASK-310, fails the same check, (12, 0); TASK-310 lengthened the window by the making durable. The store's order is right (a log must stay until its framed file is durable: framings_wait_for_no_other_framings_syncs pins it). Fixed in the test: it holds every making durable (segments::making_durable) until its commits are made, then settles, and counts (1, 2), then (1, 1) once reopened: deterministic whatever the host's speed; red without settling, (13, 0). Loops of it with the crate's other recovery tests (11, cargo nextest, a process each): before the fix 0/200 on macOS and 0/200 on ubu1 (idle hosts don't hit it); with it 200/200 on macOS and 200/200 on ubu1; extent_store whole 240/240 on macOS.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
commits_reopen_from_their_logs counted logs before the store settled; closed segments' logs go after the commits return (since TASK-165.9, widened by TASK-310). The test now holds making durable, settles, and expects the two spares; 200/200 on macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
