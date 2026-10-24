---
id: TASK-44.19
title: >-
  Hand mutants of the guest's DAX locks and rechecks: which removed lock no test
  notices
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 398000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Before TASK-99's fix, taking region->lock out of fxfs_dax_region_open would have failed no test; cargo-mutants (TASK-44.1) covers Rust only. Removing each lock, gate check and seal recheck in dax.c and device.c, one at a time, and running the DAX suites, shows the code no test guards.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each lock acquisition, gate check and seals recheck in dax.c and device.c is removed in turn and the DAX suites run on it
- [ ] #2 Every surviving mutant gets a test that kills it, or a reason it is equivalent
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
