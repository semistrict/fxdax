---
id: TASK-79.36
title: >-
  Allocations from a removed tenant arena are not refused: 'no arena' panics,
  lost chunks, VMs aborting
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
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
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC#1 is not proven for the 'VMs aborting' failure the task names: a re-granted tenant's VMs are handed store instances opened in the old arena, and their first STAGE_MAP aborts the VMM. Its two Linux-only tests have pass-after evidence only. stores_open_in_another_arena_are_busy (4249f9b5) and tenants_granted_again_while_alive_go_on (1f93178a) were type-checked, never run against the code before their fixes; the r1-929x.txt plan's step 3 before-runs never happened. Needed: a GCE run of stores_open_in_another_arena_are_busy at 04d0f609 with crates/fxdax/src/extentd/tests.rs from 4249f9b5 (expected to fail with left None, right Some(Some(EBUSY))). Also a GCE run of tenants_granted_again_while_alive_go_on at 4249f9b5 with crates/fxdax/tests/grants.rs from 1f93178a (expected EBUSY at connect_read_only). Keep both failures' output. The 'no arena' panic and the lease failure (EPIPE) do have before evidence. Audit: Agree it is not done. AC#1 is unproven for the 'VMs aborting' failure: a re-granted tenant's VMs are handed old-arena instances and abort at STAGE_MAP. The messages of 4249f9b5 and 1f93178a both say 'Type-checked ... not run: no Linux host was reachable'. A search of every scratchpad log and task output finds only PASS lines for stores_open_in_another_arena_are_busy and tenants_granted_again_while_alive_go_on, never a run before their fixes. Step 3 of r1-929x.txt (the before-runs) never happened. Needed: a GCE run of stores_open_in_another_arena_are_busy at 04d0f609 with 4249f9b5's extentd/tests.rs (expect left None, right Some(Some(EBUSY))), and one of tenants_granted_again_while_alive_go_on at 4249f9b5 with 1f93178a's tests/grants.rs (expect EBUSY at connect_read_only), with both outputs kept. The 'no arena' panic (run_7936_before.log) and the EPIPE lease failure (7936-lease-before.log) do have before-evidence. AC#2 holds: every named test PASSes in L, and in M where it is portable, and the two Shuttle tests PASS in F:115/109 and M:2999/2993 (L holds no Shuttle runs). DoD#3 holds: pmm.rs changes carry fxdax: markers and a module-doc note naming TASK-79.36, and the rest is fxdax's own code.
<!-- SECTION:NOTES:END -->
