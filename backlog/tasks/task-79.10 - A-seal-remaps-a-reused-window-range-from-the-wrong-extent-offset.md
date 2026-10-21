---
id: TASK-79.10
title: A seal remaps a reused window range from the wrong extent offset
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
