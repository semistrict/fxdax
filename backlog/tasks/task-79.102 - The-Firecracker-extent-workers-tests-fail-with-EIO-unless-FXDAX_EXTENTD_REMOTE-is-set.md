---
id: TASK-79.102
title: >-
  The Firecracker extent worker's tests fail with EIO unless
  FXDAX_EXTENTD_REMOTE is set
status: To Do
assignee: []
created_date: '2026-10-24 03:39'
updated_date: '2026-10-25 04:44'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 373000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b. In vendor/firecracker, `cargo test -p vmm --features fxdax --lib extent::` has 4 tests that fail with EIO without FXDAX_EXTENTD_REMOTE in the environment; no script sets it or runs those tests, so they are not part of any suite.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The worker's tests pass without the variable, or a script sets it and runs them as part of the suites
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r97: the 9 extent device tests pass with FXDAX_EXTENTD_REMOTE=memory:///vmm FXDAX_EXTENTD_TOKEN=1 (n=1 is not it). Still no script runs them.
<!-- SECTION:NOTES:END -->
