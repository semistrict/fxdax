---
id: TASK-395.4
title: >-
  A collector pass per cell: lists, recovers and deletes within the cell,
  keeping referenced fork-point states
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 20:33'
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
- [x] #1 A pass over a cell makes no LIST outside the cell's prefix and deletes nothing outside it; its memory and requests grow with the cell's objects, branches and references, not the bucket's (shown by a test counting requests and listed objects over a bucket of many cells)
- [x] #2 A pass keeps everything the parent state at each referenced fork point needs (objects, moves ways, what recovering it reads), and a fork point's state shared by many references (a point cloned many times) is recovered once
- [x] #3 A cell with no live branch and no references is reclaimed whole, but its cell object (kept, doc-7)
- [x] #4 The whole-bucket pass and `gc/lease` are removed; `--dry-run`, `--mark-only`, `--branches` and `--delete-branch` work per cell or over all cells
- [x] #5 Every existing collector test, the simulation, Shuttle and crash-round suites pass against the per-cell collector
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
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

Verified on macOS (branch t3/395-per-cell-collector at 675fd333b, rebased on origin/main c6ba0d117): scripts/test.sh before the rebase exit 0: clippy clean, workspace 2315/2315, sim 59/59, Shuttle 37+10+33+6+10+18 all passed, scripts 312 OK, mutant checks 0 broken, go ok. After it: clippy gate clean, nextest extent_remote+extent_store+fxdax 807/807, sim 59/59, mutant checks 0 broken. Commits 1d07b12e8 (code and tests), a05be7dc2 (doc-7, OPEN_QUESTIONS.md), 675fd333b (backlog). Not run: ubu1 (the parent's), TLA+ (specs untouched). Flake seen once under load, outside this change: TASK-399.

Review round (@claude, 2026-11-13; a6cc31ba1, 7e1eeac79, 0de908c63 on t3/395-per-cell-collector):
1. AC #1 made true: a recovery reads a parent in another cell by its checkpoint at the fork point, GET by key (recovery::state_at's from cell), no LIST of its checkpoints; record_fork (and a live claim in delete_fork) writes that checkpoint first if missing (recovery::checkpoint_at, create-only). Test passes_over_a_live_forks_cell_list_nothing_of_its_parents (live uncheckpointed cross-cell child; parent checkpointed before the fork or not); red with the LIST back. Spec unchanged: argued in doc-7 and OPEN_QUESTIONS.md (the newest checkpoint up to the fork point is what StateAt reads).
2. Collector keeps nothing across passes (Remembered pass-local); passes_read_records_and_checkpoints_once_and_delete_at_once shows every record read again on the next pass.
3. Marks watched only while gc/marking exists (written by mark-only passes; readers HEAD it once a minute); cells unread for 2 min forgotten. Tests in marks.rs and store.rs (a blind reader lists no marks). Decision in OPEN_QUESTIONS.md.
4. References listed once, by delimiter: a clone's reference and its point's share the fork point's prefix, so the prefix holds a key all along; doc-7 and OPEN_QUESTIONS.md say why; NoReferencesRelisted unchanged (the model's rule is listing references after the states, which the code keeps).
7. passable() checks the cell before the lease: the cell object's format (a GET; format 8 refused by name, tested), else the first record's, and emptiness; layout::moves returns Error::Corrupt for a key of no cell's (objects_of_no_cell_have_no_moves).
Also: a fork across cells whose parent's state at the fork point is gone (sim restores seed 11) is recorded deleted, not failing the uploader (TASK-395.11 filed for why); a cross-cell claim with that state gone is deleted from the first (seed 143). The pre-lease GET moved two seed searches out of 0..20,000: takeovers_need_takings_pinned (first seed now 62,844) and deletions_need_fences (32,625); wider parallel searches showed the rules still needed; their windows now start there.
Linux: extentd's Report literal fixed (ubu1 t395w2). cargo check --workspace --tests --target x86_64-unknown-linux-gnu WORKS on macOS with CC_x86_64_unknown_linux_gnu='zig cc -target x86_64-linux-gnu' (CXX likewise, AR='zig ar') and CRATE_CC_NO_DEFAULTS=1; also guest/tests (-p test_cases -p runner --features test_cases/host). Both clean.
Tests (macOS): scripts/test.sh exit 0: workspace 2325/2325, sim 59/59, Shuttle and scripts parts pass; hand mutants 22+2 apply.

ubu1 run t395w3 on bb07304a6 (under fxjob): workspace 2596/2596 passed (90 skipped), sim 59/59, the other nextest parts all passed, mutant checks 0 broken, guest tests exit 0. Merged to main as 1c1174bb4 (rebased over backlog-only commits).
<!-- SECTION:NOTES:END -->
