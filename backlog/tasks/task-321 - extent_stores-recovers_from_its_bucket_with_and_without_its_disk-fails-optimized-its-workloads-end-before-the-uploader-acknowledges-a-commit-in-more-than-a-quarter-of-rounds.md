---
id: TASK-321
title: >-
  extent_store's recovers_from_its_bucket_with_and_without_its_disk fails
  optimized: its workloads end before the uploader acknowledges a commit in more
  than a quarter of rounds
status: To Do
assignee: []
created_date: '2026-11-10 10:08'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 725000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Under --cargo-profile sim (optimized) on macOS the kept rounds' test fails at origin/main 653f18e5e: '1 rounds acknowledged a commit' (needs > 25 of 100), 0.2 s for the test: the workload, its uploader's interval 1 to 20 ms of real time, ends before any upload is acknowledged. scripts/test.sh runs only the sim:: tests optimized, so CI does not see it; ubu1 (Linux, sim profile) passed it. Its count of rounds acknowledging depends on how fast the workload runs against wall-clock intervals.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The kept rounds acknowledge commits in most rounds whatever the build's optimization: the workload waits for an upload at a random point, or the uploader is asked, not timed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
