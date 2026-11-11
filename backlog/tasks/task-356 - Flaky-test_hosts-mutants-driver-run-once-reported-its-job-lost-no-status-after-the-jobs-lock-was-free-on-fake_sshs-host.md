---
id: TASK-356
title: >-
  Flaky: test_host's mutants driver run once reported its job lost (no status
  after the job's lock was free) on fake_ssh's host
status: To Do
assignee: []
created_date: '2026-11-11 13:15'
labels:
  - bug
  - testing
dependencies: []
priority: low
ordinal: 744000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
During TASK-344, python3 -m unittest test_linux_host test_host on macOS failed once: test_host.Drivers.test_mutants_sweep_the_crates_asked, the driver saying 'run-... on ubu1 is lost: its job ended without its status.' (scripts/lib/host-remote.sh's wait found the job's lock free and no job/status). Not reproduced in 50 runs of that test alone nor 4 runs of test_host at once (2026-11-11). The job protocol: launch locks job/lock on fd 9 (flock -n 9) and starts the job under setsid/nohup, which inherits fd 9 and writes job/status before exiting; wait blocks on flock of job/lock. On the fake host, flock is scripts/tests/fake_flock.py and setsid a python os.setsid() then exec: a job that never started (setsid's python failing, as EPERM) would leave no status and release the lock as the launching shell exits; on ubu1 util-linux's setsid forks instead. Find the window: make the lost message say whether the job's pid ran and keep its log in the test's failure, and run the suite under load until it reproduces.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause of the lost job named, or the window closed in host-remote.sh or the fakes
- [ ] #2 test_host passes 200 runs under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
