---
id: TASK-395.2
title: >-
  Bucket layout: everything a cell's collection reads under its prefix (moves,
  collector lease, references), format 8
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 20:33'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.1
parent_task_id: TASK-395
priority: high
ordinal: 774000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Moves objects live at `moves/<key>` (and `moves/moves/...`), so a cell's compacted objects are spread over top-level prefixes no cell's listing covers; the collector lease is one per bucket. For a cell to be collected from a listing of its own prefix, both move under it, and fork references get keys there too. A format change: format 8, which refuses 7 (AGENTS.md: no migrations).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Moves of a cell's objects, and moves of those moves, have keys under the cell's prefix; readers following an object's moves find them there
- [x] #2 Each cell has a collector lease key under its prefix; `gc/lease` is gone
- [x] #3 Fork references have keys under the parent's cell naming the parent branch, the fork point and the child (tenant, cell, branch), laid out so that one delimiter listing gives a branch's distinct fork points
- [x] #4 `layout::kind` parses the new keys and `layout::owner` attributes every one to its cell; a bucket of format 7 is refused by name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. layout: format 9 (8 refused by name); <C>moves/<rest> and <C>moves/moves/<rest> (moves(), moved_from(), further() within a cell); <C>gc/lease, <C>gc/marks/<epoch>; <C>refs/<parent>/<at>/<tenant>.<cell>.<child> with references(), references_of(), fork_point(), reference(), referenced_parent/at() for delimiter listings; root gc/lease, gc/marks and moves/ gone (keys of no kind).
2. Kind: GcLease{cell}, Marks{cell,epoch}, Moves{cell,of}, Reference{parent,at,child}; owner() attributes each to its cell; layout::Reference bytes (format, owner).
3. Readers follow moves at the new keys (fetch, MovesMaps::read, writer/extent_store further); marks per cell in Store/marks::Watch (each cell read refreshed every REFRESH).
4. Cell Display/FromStr (TENANT/CELL), cell::listed() by delimiter listings.
5. Tests: layout kinds/owners/prefix/references/moves; record format 8 refused; cell listing; store marks per cell; fxdax-gc --usage over the new keys.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Format 8 is taken by TASK-395.7 (a point's record says whether it is shared): this task's layout change is format 9.

Round 1 (@claude, 2026-11-13), batch with TASK-395.3 and .4 on branch t3/395-per-cell-collector.
Landed:
- layout: FORMAT 9; format 8 refused by name (record::tests::records_of_other_formats_are_refused). Under a cell's prefix <C>: moves `<C>moves/<rest>`, their moves `<C>moves/moves/<rest>` (layout::moves, moved_from, further within a cell; moves of an object of no cell panic: only a cell's objects move); collector lease `<C>gc/lease` (gc_lease), marks `<C>gc/marks/<epoch>` (marks_of, marks); references `<C>refs/<parent>/<at>/<tenant>.<cell>.<child>` (references, references_of, fork_point, reference; referenced_parent/referenced_at parse delimiter prefixes). The root gc/lease, gc/marks/ and moves/ are keys of no kind now.
- Kind: GcLease{cell}, Marks{cell,epoch}, Moves{cell,of}, Reference{parent,at,child}; owner() gives every one its cell (references their parent's). layout::Reference {format, owner} is a reference's bytes. cell_of / cell_of_prefix helpers.
- extent_frame Cell: Display/FromStr TENANT/CELL. cell::listed(): every cell by delimiter listings of tenants/ and each tenant's cells/, listing none of their objects.
- Readers: MovesMaps::read and fetch follow the new keys; marks::Watch holds marks per cell, a cell's refreshed every REFRESH alongside a read of that cell (Store::refresh_marks(cell)); the collector's census of a cell isn't logged.
Where code differs from the spec: none in layout; marks per cell cost a LIST a minute per cell read (OPEN_QUESTIONS.md, Decisions).
Tests: layout::tests (a_cells_objects_are_under_its_tenants_prefix, references_list_by_parent_then_fork_point, moves_are_further_along_the_way_in_their_cell, objects_of_no_cell_have_no_moves, kinds, owners), cell::tests::cells_are_listed_by_their_prefixes, marks::tests (per cell), store::tests::stores_warn_of_reading_what_the_newest_marks_name (two cells), fxdax-gc unit + tests/gc.rs (usage over the new keys, file bucket layout).

Verified on macOS (branch t3/395-per-cell-collector at 675fd333b, rebased on origin/main c6ba0d117): scripts/test.sh before the rebase exit 0: clippy clean, workspace 2315/2315, sim 59/59, Shuttle 37+10+33+6+10+18 all passed, scripts 312 OK, mutant checks 0 broken, go ok. After it: clippy gate clean, nextest extent_remote+extent_store+fxdax 807/807, sim 59/59, mutant checks 0 broken. Commits 1d07b12e8 (code and tests), a05be7dc2 (doc-7, OPEN_QUESTIONS.md), 675fd333b (backlog). Not run: ubu1 (the parent's), TLA+ (specs untouched). Flake seen once under load, outside this change: TASK-399.

Review round (@claude, 2026-11-13; a6cc31ba1, 0de908c63): format 8 cell objects and records refused by name (cell::cells_of_other_formats_are_refused over FORMAT+1 and 8; collector's passes_take_no_lease_of_cells_empty_or_of_other_formats); layout::moves gives Error::Corrupt for a key of no cell's; new root key gc/marking (Kind::Marking).

ubu1 run t395w3 on bb07304a6 (under fxjob): workspace 2596/2596 passed (90 skipped), sim 59/59, the other nextest parts all passed, mutant checks 0 broken, guest tests exit 0. Merged to main as 1c1174bb4 (rebased over backlog-only commits).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Format 9: moves, the collector's lease and marks, and fork references live under each cell's prefix (<C>moves/, <C>gc/lease, <C>gc/marks/, <C>refs/<parent>/<at>/<tenant>.<cell>.<child>); the root gc/lease is gone; layout::kind and owner cover every key; format 8 refused by name. Verified by extent_remote's layout and format tests, the macOS suite (workspace 2325/2325) and ubu1 run t395w3 (workspace 2596/2596, sim 59/59, guests pass); scripts/upstream-changes.py --check passes. Merged to main (1c1174bb4).
<!-- SECTION:FINAL_SUMMARY:END -->
