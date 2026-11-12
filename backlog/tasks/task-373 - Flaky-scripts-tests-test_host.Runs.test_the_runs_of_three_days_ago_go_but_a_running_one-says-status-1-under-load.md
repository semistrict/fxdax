---
id: TASK-373
title: >-
  Flaky: scripts/tests
  test_host.Runs.test_the_runs_of_three_days_ago_go_but_a_running_one says
  status 1 under load
status: To Do
assignee: []
created_date: '2026-11-12 05:00'
labels:
  - scripts
  - flaky
dependencies: []
priority: low
ordinal: 752000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-12 in a macOS scripts/test.sh run (parts clippy shuttle scripts go) while ubu1 and the Mac were loaded: the cleanup of runs three days old printed `status 1` where the test expects `status 0`, the removal itself as expected. Run alone it passes. Likely a timing assumption in how the test decides a run is still running (a process or a lock looked at while the machine is slow).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test passes under load: 20 runs of the scripts part beside a parallel cargo build, none failing
- [ ] #2 The blind spot is named: what timing the test or scripts/lib/host.sh relied on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
