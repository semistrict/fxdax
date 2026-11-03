---
id: TASK-160.29
title: 'Guest test cases tagged, not listed by hand in six scripts'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 600000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
What runs is listed by hand in test-e2e.sh, test-guest-kernel.sh, dax-gate.sh, guest-mutants-gce.py, soak-workloads-gce.sh and perf-gate-gce.sh; xfstests, the script cases, crash-dax-daemon-killed and pgbench variants are named by none; nightly runs no follow, snapshot, volumes, faults-dax, pjdfstest or xfstests case. Explains 3981ad18f, 6d6f1846c, 69227bac0. Proposal: tags on TestCase and runner list --tag; a unit test fails an untagged case.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cases tagged; scripts take tags
- [ ] #2 A unit test fails an untagged case
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
