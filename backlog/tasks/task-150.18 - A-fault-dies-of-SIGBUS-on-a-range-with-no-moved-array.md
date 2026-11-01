---
id: TASK-150.18
title: A fault dies of SIGBUS on a range with no moved array
status: To Do
assignee: []
created_date: '2026-11-01 19:09'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 539000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim's candidate 5, unconfirmed on a real kernel: fxfs_dax_place returns early on a range with no moved array and TASK-124's relocation is skipped. Seed 8083 at 200 operations, without its candidate fix in guest/sim/fixes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
