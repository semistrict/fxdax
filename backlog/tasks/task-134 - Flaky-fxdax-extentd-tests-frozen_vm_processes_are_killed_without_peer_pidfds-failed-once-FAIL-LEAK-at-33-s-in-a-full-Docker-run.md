---
id: TASK-134
title: >-
  Flaky?: fxdax
  extentd::tests::frozen_vm_processes_are_killed_without_peer_pidfds failed once
  (FAIL + LEAK at 33 s) in a full Docker run
status: To Do
assignee: []
created_date: '2026-10-30 01:37'
labels:
  - bug
  - flaky
dependencies: []
priority: low
ordinal: 481000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
OrbStack Docker, the TASK-111.7 worktree (e04c0daa9's parent tree with the build stamp), one full fxdax run: the test failed and leaked at 33 s. It passed in two more full runs and 10 of 10 alone. Unknown whether it predates the change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reproduced, or shown to pass 50 runs under the full suite's load on Linux; the cause fixed if reproduced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
