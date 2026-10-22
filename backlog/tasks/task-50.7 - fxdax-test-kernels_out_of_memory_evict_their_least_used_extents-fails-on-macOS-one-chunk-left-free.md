---
id: TASK-50.7
title: >-
  fxdax test kernels_out_of_memory_evict_their_least_used_extents fails on
  macOS: one chunk left free
status: To Do
assignee: []
created_date: '2026-10-21 09:49'
updated_date: '2026-10-21 15:28'
labels:
  - bug
  - test
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 165000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On macOS (Darwin 27, 2026-10-21), at 6a0a3f8 (main), cargo nextest run -p fxdax -E 'test(kernels_out_of_memory_evict_their_least_used_extents)' fails alone and in scripts/test.sh: crates/fxdax/src/extents.rs:2198 'the kernel never ran out', node.count_free_chunks() is 1, not 0, after reading 64 extents twice through a 48 MiB kernel (TASK-50.3, 6f3f5b0). It blocks scripts/test.sh on macOS (nextest cancels the run on the first failure). Found by TASK-42's follower work, whose changes don't touch crates/fxdax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test passes on macOS and Linux, or its expectation is corrected with why
- [ ] #2 scripts/test.sh passes on macOS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-21 15:40: passes on macOS with the machine otherwise idle (cargo nextest run -p fxdax, 151/151); both failures were while a cargo-mutants sweep ran alongside (three cargo builds and tests at once): it depends on the host's memory or timing, which the test should not.
<!-- SECTION:NOTES:END -->
