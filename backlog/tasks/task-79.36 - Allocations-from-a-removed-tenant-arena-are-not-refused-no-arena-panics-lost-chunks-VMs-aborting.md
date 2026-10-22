---
id: TASK-79.36
title: >-
  Allocations from a removed tenant arena are not refused: 'no arena' panics,
  lost chunks, VMs aborting
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 224000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
remove_arena only lifts the arena's cap, so a later allocation from it pops a chunk and panics with 'no arena', and the chunk is lost. Boot-profile replay threads reach this, and so does a re-granted tenant whose VMs are handed store instances opened in the old arena; their first STAGE_MAP aborts the VMM.

Where: crates/zircon/src/vm/pmm.rs:1070, crates/zircon/src/vm/pmm.rs:314, crates/zircon/src/vm/physmap.rs:223, crates/zircon/src/vm/physmap.rs:250, crates/fxdax/src/extentd/server.rs:122, crates/fxdax/src/extents.rs:1726, crates/fxdax/src/extentd/grants.rs:64, crates/fxdax/src/extentd/stores.rs:73, crates/fxdax/src/dax/uffd.rs:786.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-43.4, TASK-43.1, TASK-38, TASK-71. TASK-43.4 (Done) designed Revoke and arena retirement (remove_arena retires the arena until its last chunk returns). No task tracks allocations from a removed arena panicking with 'no arena'. Nothing tracks boot-profile replay threads (TASK-38) or re-Grant either.
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
