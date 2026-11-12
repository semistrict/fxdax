---
id: TASK-385
title: >-
  scripts/tests test_host
  test_a_migration_restores_on_the_second_run_what_the_first_left failed once on
  main's CI: cat .held/bundle/snapshot: No such file
status: To Do
assignee: []
created_date: '2026-11-12 14:00'
labels:
  - bug
  - ci
  - scripts
dependencies: []
priority: medium
type: bug
ordinal: 764000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main CI run 43231642989 (26241e965, TASK-380's fix, which touches no script), job 'test (clippy scripts)': scripts/tests/test_host.py:1005 Drivers.test_a_migration_restores_on_the_second_run_what_the_first_left: status 1, 'cat: /mnt/tmp/tmp.../scratch/run-20261112T135539Z-c241fd05.held/bundle/snapshot: No such file or directory'. Passed on the three runs before (a8ba9e944, 0c2d489a4, 3a9c5181e), so a race in the fake host's migrate driver (migrate-host.sh with FXDAX_FAKE_HOST=migrate) or its held bundle, not a change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The race found and fixed, the test passing 200 times in a row
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
