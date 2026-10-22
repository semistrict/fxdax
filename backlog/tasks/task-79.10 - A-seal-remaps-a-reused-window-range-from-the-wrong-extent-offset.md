---
id: TASK-79.10
title: A seal remaps a reused window range from the wrong extent offset
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-22 10:39'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 198000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
seal_mapped walks a never-pruned history of MAPs, matches by VMO identity only, and takes the offset from the history record rather than the live mapping. After MAP E@0 at W, UNMAP, then MAP E@2 MiB at W, the commit remaps W to E@0, so the guest reads the wrong bytes and snapshots record the wrong offset. The history also grows by one entry per MAP until commit.

Where: crates/fxdax/src/dax.rs:544, crates/fxdax/src/dax.rs:569, crates/fxdax/src/extents.rs:1292, crates/fxdax/src/extents.rs:383, crates/fxdax/src/extents.rs:1525, crates/fxdax/src/extents.rs:1597.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.12, TASK-26.16. No task covers seal_mapped's never-pruned MAP history remapping with a stale offset. Commit-time remap of open extents' window mappings came from TASK-26.12 (Done).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in b5bc162: seal_mapped remaps Dax::kept(identity), each part at its Mapped record's offset; OpenExtent::mappings removed. Test extents seals_map_a_reused_range_at_its_offset failed before (sealed to offset 0), passes after on macOS; Linux suite on GCE passes. It was the cause of TASK-59's corruption under pgbench -i -s 50.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Seals remap the window's live mappings of an open extent, each at its own offset, instead of a history of every MAP; a reused range is no longer sealed to the wrong part of its extent. Shown by a unit test (red before, green after), the suites, and pgbench -i -s 50 completing 3 of 3.
<!-- SECTION:FINAL_SUMMARY:END -->
