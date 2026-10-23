---
id: TASK-79.79
title: >-
  extent_store crash tests: a crash round is not a function of its seed, so a
  failing round may not reproduce
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 06:48'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 337000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.42 (round 2a). In crates/extent_store/tests/crash.rs a round's crash state depends on more than its seed: digesting the crashed directory of the first workload of recovers_a_prefix_of_its_commits, 6 runs per round, 63 of 100 rounds differed between runs on review-r1's base and 65 of 100 after round 2a. A failure printed as 'round N' may not reproduce, and passing runs say less than they seem.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A crash round's crashed directory is the same on every run of its seed
- [ ] #2 A test runs a round twice and compares the digests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
