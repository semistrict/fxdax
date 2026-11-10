---
id: TASK-291.2
title: >-
  extent_store crash rounds: a collector pass cut at a random request, then the
  store recovers
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 09:00'
labels: []
dependencies:
  - TASK-88
parent_task_id: TASK-291
priority: medium
ordinal: 693000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_store's crash rounds (tests/crash) never run the collector. A pass stopped midway (a host dying mid-delete or mid-compaction) is only modelled by the simulation's stop_collector, against its own writers, not against a real store's recovery from its WAL and part cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A crash round runs collector passes during the load and cuts one at a random bucket request
- [ ] #2 After the cut the store reopens, every acknowledged write reads back, and a later full pass completes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. tests/crash/kept.rs: a Cut bucket over the host's: after a random number of requests, its requests never complete, as a dead host's (bucket::* retries failures for ever, so an error would not cut a pass).
2. During each kept round's workload, a task on the host's runtime loops collector::collect (compact_below 0.9) over the Cut bucket; aborted at the crash; an error from a pass not cut fails the round.
3. After the crash, a full pass over the bucket as the crash left it completes; then the branch taken from the bucket and the store reopened from its disk hold what they held (every acknowledged write reads back).
4. The test counts rounds whose pass was cut before the crash, and asserts enough are.
Verify: cargo nextest run -p extent_store --test crash
<!-- SECTION:PLAN:END -->
