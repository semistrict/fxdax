---
id: TASK-135
title: >-
  Flaky: fxdax::control health_follows_whether_the_daemon_serves sees Serving
  before the daemon serves (Docker, full fxdax run)
status: To Do
assignee: []
created_date: '2026-10-30 03:38'
labels:
  - bug
  - flaky
dependencies: []
priority: low
ordinal: 482000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
OrbStack Docker, full fxdax run of the batch worktree (3e38f4d81 + fix, nothing in control touched): crates/fxdax/tests/control.rs:337, status("") Serving where NotServing was expected, before the test starts serving. Passed 5 of 5 alone. Each test is a process of its own under nextest, so something in the process (the health service's initial state, SERVING's watch, a thread of serve_control) says Serving early under load.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found and fixed; the test passes 50 runs under the full suite's load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
