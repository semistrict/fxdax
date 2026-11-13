---
id: TASK-395.4
title: >-
  A collector pass per cell: lists, recovers and deletes within the cell,
  keeping referenced fork-point states
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 14:30'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.1
  - TASK-395.2
  - TASK-395.3
parent_task_id: TASK-395
priority: high
ordinal: 776000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Replaces the whole-bucket pass. A pass over one cell takes the cell's collector lease, lists the cell's prefix once (plus the relisting the spec requires), recovers the cell's live branches and the parent states at each referenced fork point, and compacts and deletes only under the prefix. Objects outside the prefix that the cell's branches name (inherited from other cells) are never candidates: the cells holding them keep them by reference. Today's pass costs 5+ full listings, a map of every object, and every branch recovered; per cell all of that is bounded by the cell.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A pass over a cell makes no LIST outside the cell's prefix and deletes nothing outside it; its memory and requests grow with the cell's objects, branches and references, not the bucket's (shown by a test counting requests and listed objects over a bucket of many cells)
- [ ] #2 A pass keeps everything the parent state at each referenced fork point needs (objects, moves ways, what recovering it reads), and a fork point's state shared by many references (a point cloned many times) is recovered once
- [ ] #3 A cell with no live branch and no references is reclaimed whole, but its cell object (kept, doc-7)
- [ ] #4 The whole-bucket pass and `gc/lease` are removed; `--dry-run`, `--mark-only`, `--branches` and `--delete-branch` work per cell or over all cells
- [ ] #5 Every existing collector test, the simulation, Shuttle and crash-round suites pass against the per-cell collector
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. collector: Collector keeps per-cell memory; collect_cell takes <C>gc/lease, lists <C> (refs filtered out), records under <C>branches/, recovers live branches (cross-cell forks in states -> fork points), relists records while checkpointed, then lists references twice by delimiter, keeps each fork point's state once (recovery::recover_at: extents along ways from seals, touched), lost fork point fails only with a live child naming it; compaction/deletes as before within the cell; retires dead cell's children's references. collect() sweeps cell::listed() in a plain loop. Lost clone being made: point re-read, given up if deleted.
2. mark_cell/dry_run_cell per cell; DryRun.retired; Report.retired.
3. fxdax-gc: --cell TENANT/CELL for passes, --mark-only, --dry-run, --branches; else every cell; --usage unchanged.
4. Rules references_held, references_relisted, fork_points_kept.
5. Port every collector/sim/fault test; testing::uncollected conservative per cell; collect_until_quiet helper.
6. New tests: request counting over many cells (no LIST/DELETE outside the cell but the retirement), shared fork point recovered once, dead cell reclaimed whole, each rule's mutation.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), batch with TASK-395.2 and .3 on branch t3/395-per-cell-collector.
Landed (crates/extent_remote/src/collector.rs; the whole-bucket pass and the root gc/lease are gone):
- Collector::collect_cell: takes `<C>gc/lease`, lists `<C>` once (references filtered out: never candidates), reads the cell's records (listing `<C>branches/` until none new; cached per cell by version), gives up forks of deleted points being made (a point of another cell read there), recovers the live branches; forks in their states to other cells keep their fork points, within the cell recover the children as before; relists the records while a state is from a checkpoint; then lists the references twice by delimiter (`<C>refs/` then each `<C>refs/<parent>/`) and keeps each fork point's state once (recovery::recover_at: its extents along their ways from their seals, and what recovering it read); a lost fork point fails the pass only if a child named under it lives on it (its record read then). Compaction and deletes as before within the cell (TASK-395.5's concurrency kept); then, a dead cell (no live branch, no reference), retires its deleted children's references under their parents' cells, keys from the records' origins.
- collect(): a sweep over cell::listed() in a plain loop, a failing cell stopping none; collect_cell(), mark_cell(), dry_run_cell() (DryRun.retired); Report.retired. A Collector remembers each cell apart.
- A clone being made whose recovery fails, its point read again deleted, is given up on (doc-7, beyond the model), under the rule collector_abandons.
- fxdax-gc: --cell TENANT/CELL for passes, --mark-only, --dry-run and --branches; every cell otherwise; one line per sweep (deleted, compacted, retired, cells, failed), a line per cell for mark-only; --usage unchanged (owner() covers the new keys). fxdax's in-process collector of an image's bucket sweeps every cell.
- testing: uncollected() per cell and conservative (fork points whole, references garbage once retirable); live(); collect_until_quiet() (until two sweeps in a row delete and compact nothing).
Where code differs from the spec: references listed twice (pages not atomic); the clone give-up; retirement repeated every pass of a dead cell (counted in Report.retired: the pass can't tell those gone); a lost fork point retried each pass; passes over a cell its listing finds empty still take its lease. Rules references_held, references_relisted, fork_points_kept are fields; NoReferencesRelisted has no code test (TLA+ only): its interleaving needs a clone's reference created between the candidates' listing and the states with a checkpoint dropping the point, which neither a direct test nor a sim profile was built for.
Ported tests: every existing collector/sim/fault test runs against the per-cell collector. Changed expectations, each by the design: mutation tests of records_relisted and recoveries_by_origins turn references off too (as their TLA+ mutations); TASK-180's fenced-pass test gives the clone up, and passes_fenced_stop is shown needed by a claim with a writer; fork_faults' orphaned fork needs its parent's checkpoint to drop the deleted fork before its fork point goes; inherited extents are compacted once, not twice, while their fork lives; sweeps until quiet where tests expected one pass to settle the bucket.
New tests: passes_over_a_cell_list_and_delete_within_it_alone (AC #1: no LIST/PUT/DELETE outside the cell but the retirement; identical requests with 3 or 30 other cells), fork_points_shared_by_many_references_are_recovered_once (1 clone vs 8: same requests, the fork point's segment read once), references_hold_while_their_childrens_cells_hold_references (AC #3: dead cells reclaimed whole but records), fork_points_kept_for_children_in_other_cells_need_what_recovering_them_reads, fxdax-gc passes_and_lists_go_over_a_cell_or_all, sweeps_say_what_their_passes_did, tests/gc.rs --cell and --branches --cell.
Simulation: the per-cell passes moved the seeds of five rules' searches, which then found nothing in 20,000: Profile::one_cell (chance of a pass over one branch's cell alone, default 0.5); deleted_forking and recreated sweep only (records_reread, newest_checkpoint found again); takeovers_need_takings_pinned searches recreated (late_checkpoints found none in 60,000); collector_abandons now also gates the clone give-up (without it the give-up hid the rule's case); candidates_first searches a new profile fresh_cells (passes over one cell, the child's among them before its record; every sim fork is to a cell of its own), with a soak passes_over_cells_being_made. New searches: collection_needs_references, _fork_points_kept, _references_held (each found within a few seeds); records_listed_after_states turns references off too, as its TLA+ mutation.
Upload's Shuttle tests gate a cell's whole listing, a pass's of its candidates, as they gated the bucket's.
<!-- SECTION:NOTES:END -->
