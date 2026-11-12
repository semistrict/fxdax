---
id: TASK-339
title: >-
  Flaky on macOS CI:
  remote::tests::forks_read_what_they_inherited_compacted_twice, a
  once-compacted moves key still listed after the collector's fourth pass
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 00:57'
updated_date: '2026-11-12 03:45'
labels:
  - testing
  - extent_remote
dependencies: []
priority: medium
type: bug
ordinal: 739000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42932581818 (branch t330-land at cd307c317, macos (workspace) job 130234811891, 2026-11-11): 2227 of 2228 passed; this one failed at crates/fxdax/src/remote.rs:559, assert!(!keys.iter().any(|key| key.starts_with(&once))): the listing held 'moves/moves/segments/3cc4a6f15455bb07/0000000000000001' and still 'moves/segments/3cc4a6f15455bb07/0000000000000001'. The same commit's code (no fxdax crate change from main) passed it in CI runs 42915833387 and 42924964007 and in two local macOS suites. So the moves compacted once sometimes outlive collect(4): the test's collections race something (the writer's checkpoint upload, the uploader's interval) or the collector's keep rule (TASK-195's) differs by timing. Not reproduced yet.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The race named and a test that fails without the fix (the checkpoint landing late) passes with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12: again on main CI 43145010141 (09fa51f08), macos (workspace), crates/fxdax/src/remote.rs:559.

2026-11-12 (claude): found. A test race, not a product bug. The test commits CHECKPOINT_EVERY times and collects at once, but the writer's checkpoint, the state the collector frees the segment and the first moves by, is written by the uploader once it has uploaded those segments, after the commits returned: probed, it is not in the bucket even unloaded when collect(3) lists it, landing during that pass. Landing later (a loaded macOS runner), collect(3) reads the fork as behind and keeps the segment; collect(4) then compacts the moves, the once-moves still there: CI's listing. The collector is right to keep them without the checkpoint. Not reproduced by 200 plain runs nor by delaying uploads (remote::gather / remote::acknowledged sleep: the pass takes as long), so: a failpoint before the uploader's periodic checkpoint (extent_store upload.rs, remote::checkpointing, a no-op unless set) and forks_read_what_they_inherited_compacted_twice_their_checkpoint_late (sleep(1000) there): red without the fix (panicked at remote.rs:580), green with it. Fix: crate::test_checkpoints::{after, checkpointed}: run the commits, then wait for a checkpoint past the newest before them in the bucket (60 s at most). Blind spot: tests synchronising on their own commits for a state another thread makes. Looked around: the same race in its two siblings (extents_compacted_read_once_their_host_died_and_its_cache_went, compacted_twice's two tests) and in extents.rs's followers_are_cut_off_once_the_collector_reclaims_what_they_read (deleted: 65 exactly); all four now wait. Structure: fine; the checkpoint is rightly asynchronous, and tests now wait on the condition the collector reads. Stress: the five tests 100/100 iterations; fxdax + extent_store suites on macOS 577/577.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Test race: tests collected right after the commits meant to make the writer checkpoint, but the uploader writes that checkpoint asynchronously once it has uploaded them; when it landed during or after the collector's pass (a loaded macOS runner), the pass kept what it frees, and the test's listing still held the once-compacted moves. The four tests with this pattern now wait for the checkpoint in the bucket (crate::test_checkpoints). A failpoint before the periodic checkpoint (remote::checkpointing) makes it late on purpose: the new test fails without the wait, passes with it.
<!-- SECTION:FINAL_SUMMARY:END -->
