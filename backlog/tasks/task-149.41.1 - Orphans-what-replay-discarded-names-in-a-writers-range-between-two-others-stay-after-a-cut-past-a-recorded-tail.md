---
id: TASK-149.41.1
title: >-
  Orphans what replay discarded names, in a writer's range between two others,
  stay after a cut past a recorded tail
status: Done
assignee: []
created_date: '2026-11-08 01:24'
updated_date: '2026-11-08 03:29'
labels:
  - testing
  - fxfs
  - wontfix
dependencies: []
parent_task_id: TASK-149.41
priority: medium
ordinal: 674000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.41's hunts (seed 4 of 1,200 cases; its case pinned as orphans_named_by_a_discard_between_two_ranges_stay_unsought). A mount kills the orphans that transactions its replay discarded name with FreeAfter::RecordedFlush: a replay of the same journal re-reads those transactions and verifies writes to them, and journal/checksum_list.rs's verify fails the whole replay on an extent it can't read (NoExtent), as CI run 41876989384 found (TASK-210). Their frees wait for Journal::flush_for_frees, after renew_journal_tail and record_journal_tail have written past the mount's floor. A cut between the super-block and those frees leaves them, of the range before the floor; the next mount seeks orphans past what the tree holds and below the device's next (allocator::orphan_windows), which misses a range between two others. The model test's second replays count such extents apart (outside what the mount sought) rather than failing. Fixes, each a decision: (a) verify takes an extent the device doesn't hold for a checksum that doesn't match, so these orphans can be freed and sealed before the mount writes, as the others are: weaker, since a bug freeing a live transaction's extent would then lose it silently rather than fail the mount; (b) the super-block records the orphans to free, a format change; (c) windows past the highest extent alive of each writer's range.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 These orphans are freed, or found by the next mount, and the model test's second replays count none apart
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: the user decided not to fix this: it leaks room only, rarely (0-13 extents per about 4,100 replays of cuts during replays in TASK-149.41's hunts), and loses no data. fxfs-posix's model test keeps counting these extents apart (holds_no_orphans_but_unsought; orphans_named_by_a_discard_between_two_ranges_stay_unsought pins 16), and fails on any orphan in a window a mount sought.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Won't fix, the user's decision (2026-11-07): a space leak only, rare, with no data loss. The orphans that replay's discarded transactions name, of a writer's range between two others, after a cut past the recorded journal tail, stay until something frees their room. fxfs-posix's model test keeps counting them apart and fails on any orphan inside a window a mount sought.
<!-- SECTION:FINAL_SUMMARY:END -->
