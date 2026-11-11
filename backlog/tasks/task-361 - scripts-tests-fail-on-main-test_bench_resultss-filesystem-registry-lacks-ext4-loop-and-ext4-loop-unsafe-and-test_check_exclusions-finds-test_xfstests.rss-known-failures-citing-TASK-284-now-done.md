---
id: TASK-361
title: >-
  scripts' tests fail on main: test_bench_results's filesystem registry lacks
  ext4-loop and ext4-loop-unsafe, and test_check_exclusions finds
  test_xfstests.rs's known failures citing TASK-284, now done
status: To Do
assignee: []
created_date: '2026-11-11 15:32'
labels:
  - testing
  - ci
dependencies: []
priority: high
type: bug
ordinal: 746000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
2026-11-11, main d2290c224 (and since ae0bcb4ca / TASK-350's bench commits): scripts/test.sh's Python tests fail 2 of 272, so scripts/test.sh exits 1 on every branch. (1) test_bench_results.Registry.test_every_filesystem_the_cases_measure_is_named: the cases now measure ext4-loop and ext4-loop-unsafe (TASK-350.1's loop-device baseline), which the test's list doesn't name. (2) test_check_exclusions.CheckExclusionsTest.test_the_tree_says_why_of_everything_it_leaves_out: guest/tests/test_cases/src/test_xfstests.rs:105 marks a known failure under TASK-284, which ae0bcb4ca marked done; check_exclusions wants a known failure's task open (TASK-348/349 may be the ones now).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Both tests pass on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
