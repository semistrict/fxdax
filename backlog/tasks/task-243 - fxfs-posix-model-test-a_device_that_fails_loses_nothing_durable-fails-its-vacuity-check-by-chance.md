---
id: TASK-243
title: >-
  fxfs-posix model test: a_device_that_fails_loses_nothing_durable fails its
  vacuity check by chance
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 06:02'
updated_date: '2026-11-08 06:02'
labels:
  - bug
  - flake
dependencies: []
priority: high
ordinal: 677000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI 42446163807 (land-h11, guest/sim only) failed it on macOS: "1 requests failed, 1 of them flushes: 0 operations failed" then "no operation failed". Its 24 sequences were drawn afresh each run; a sequence fails requests of one kind, one in 4, 32 or 256, as its seed says, and about two in three fail no operation (400 one-case runs: 272 failed none, 245 no request at all), so all 24 did so about once in 10,000 runs. Not timing: a seed gives the same counts under load and on Linux.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test runs the same sequences every run unless a hunt asks for others, its vacuity check kept
- [ ] #2 The old test is shown failing the check for a recorded seed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Fixed proptest seed by default (FAILING_SEQUENCES), random when hunting; vacuity check kept.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: land-h16 029471ace, CI 42453087403. Fixed seed 2 gives 16 requests, 6 flushes, 12 operations failed, 6 syncs on macOS (three runs) and Linux (Docker) alike. Old test: PROPTEST_RNG_SEED=7 FXDAX_MODEL_FAILURES=4 fails the check (0 requests failed); a search for a seed failing it at the default 24 cases is running.
<!-- SECTION:NOTES:END -->
