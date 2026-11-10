---
id: TASK-291.2
title: >-
  extent_store crash rounds: a collector pass cut at a random request, then the
  store recovers
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 10:33'
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
- [x] #1 A crash round runs collector passes during the load and cuts one at a random bucket request
- [x] #2 After the cut the store reopens, every acknowledged write reads back, and a later full pass completes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. tests/crash/kept.rs: a Cut bucket over the host's: after a random number of requests, its requests never complete, as a dead host's (bucket::* retries failures for ever, so an error would not cut a pass).
2. During each kept round's workload, a task on the host's runtime loops collector::collect (compact_below 0.9) over the Cut bucket; aborted at the crash; an error from a pass not cut fails the round.
3. After the crash, a full pass over the bucket as the crash left it completes; then the branch taken from the bucket and the store reopened from its disk hold what they held (every acknowledged write reads back).
4. The test counts rounds whose pass was cut before the crash, and asserts enough are.
Verify: cargo nextest run -p extent_store --test crash
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Passes running nonstop spent the cut's request budget before anything uploaded: paced by the uploader's interval. None compacted: recoveries read every segment after the newest checkpoint and the collector keeps what they read, and the uploader checkpointed every 64 segments: upload::Options::checkpoint_every, 4 in the rounds. Compressible segments never compact (TASK-293): half the rounds' writes incompressible. Measured: 30 of 100 rounds compact, 60 delete. Collector futures are not Send to tokio's spawn (higher-ranked lifetimes): passes on a std thread under futures::executor, Abortable. Found TASK-318 (fixed), TASK-321 (kept rounds' ack threshold fails optimized, pre-existing).

Verified: macOS extent_store+extent_remote 417/417, shuttle 36/36; ubu1 crash 3/3 at d1b93c62, the rest 417 (ZFS test excluded, TASK-315). compacted threshold KEPT_ROUNDS/10: ubu1 measured 19.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
tests/crash/kept.rs: during each kept round's workload, collector passes (compact_below 0.9, grace 0) run every uploader interval over a Cut bucket that lets a random number of requests go and none after, as a dead host's; aborted at the crash; then a full pass over what the crash left completes before the branch is taken from the bucket and the store reopened from its disk, both checked against the model. upload::Options::checkpoint_every (4 in the rounds) and half-incompressible data, so that passes compact. The test asserts rounds cut (>25), compact (>10), delete (>33). Found and fixed TASK-318 on the way; TASK-321 filed.
<!-- SECTION:FINAL_SUMMARY:END -->
