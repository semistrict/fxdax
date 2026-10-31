---
id: TASK-87
title: >-
  fxdax: kernels_out_of_memory_evict_their_least_used_extents fails under
  parallel load
status: Done
assignee: []
created_date: '2026-10-22 13:26'
updated_date: '2026-10-30 18:15'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-30: moot. The test went with the Zircon kernel port in f897ce4a2. Its successor, extents::tests::memory_run_out_evicts_the_least_used_extents, passed in the full suites on macOS and in Docker Linux today. ACs left unchecked: the cause of the old test's flake was never found, and it can no longer be.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as moot: the test was deleted in f897ce4a2; its successor passes in the full macOS and Docker Linux runs.
<!-- SECTION:FINAL_SUMMARY:END -->
