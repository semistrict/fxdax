---
id: TASK-149.20.1
title: xfstests runs in no suite
status: To Do
assignee: []
created_date: '2026-11-07 13:36'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: medium
type: task
ordinal: 666000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. xfstests-fxfs-guest and xfstests-fxfs-guest-dax (guest/tests/test_cases/src/test_xfstests.rs) are tagged Manual: no script's suite, nor the nightly, runs them, so the kernel's own filesystem suite runs only when someone names it, and its KNOWN_FAILURES list (now failing the case when a listed test passes, TASK-149.20) is checked only then. Last run: TASK-150.21's, GCE 2026-11 (16 of 650, the known list).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 xfstests (both cases) runs on a schedule (the nightly, or a weekly run of its own), its results kept as the guest suites' are
- [ ] #2 A run on main with the known list as it is passes, or what fails is filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
