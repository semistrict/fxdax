---
id: TASK-87
title: >-
  fxdax: kernels_out_of_memory_evict_their_least_used_extents fails under
  parallel load
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: bug
ordinal: 261000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen while testing the lineage refactor on macOS (integrate-2): the test failed once with the suite running in parallel, and passes alone. It looks timing-dependent: memory pressure measured while other tests' processes load the machine. A flaky test hides real failures.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is identified with evidence, and the test made deterministic without a timeout or a retry
- [ ] #2 It passes 50 runs of 50 with the whole fxdax suite in parallel (nextest --stress-count)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
