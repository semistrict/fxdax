---
id: TASK-377
title: >-
  fxdax
  extents::tests::followers_are_cut_off_once_the_collector_reclaims_what_they_read
  fails on main since TASK-371: the collector deletes 64 objects, not 65
status: To Do
assignee: []
created_date: '2026-11-12 09:46'
labels:
  - fxdax
  - collector
  - testing
dependencies: []
priority: high
type: bug
ordinal: 759000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
main 3037d0772 (TASK-371), macOS, deterministic (5 of 5 runs; the same on TASK-369's branch rebased on it): crates/fxdax/src/extents.rs:2400, assert_eq!(report, Report { deleted: 65, compacted: 0 }) gets deleted: 64. TASK-371's collector keeps each epoch's fence (State::fences, rule fences_kept) and every segment of a branch whose lease is past its state's (takeovers_kept), so a pass deletes one object fewer than the test counted. Either the test's count is now one too many (the kept fence is right to keep), or the collector keeps one it shouldn't; the test's comment should say which. Main CI run 43193248897 on 3037d0772 was running when found.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The count is justified under TASK-371's rules and the test passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
