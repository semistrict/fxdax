---
id: TASK-395.6
title: >-
  Incremental collection: a cell unchanged since its last pass costs one
  listing, fork-point states cached
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 19:48'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.4
parent_task_id: TASK-395
priority: medium
ordinal: 778000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most cells are idle between sweeps: a stopped VM, a base image, a snapshot. With everything a cell's collection depends on under its prefix, a listing identical to the last pass's means the pass would decide the same, so the recoveries can be skipped. A parent's state at a fork point never changes, so it can be kept across passes (only what compaction moves of it changes, which is the parent cell's own doing).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A pass over a cell whose listing matches the one its last pass recorded makes no request beyond the listing and its lease, deleting what that pass deferred to a grace period once due
- [ ] #2 What a pass records to skip by (the listing's digest, deferred deletes, cached fork-point states) is kept under the cell's prefix, so that any collector process can use it, and is never itself a candidate
- [x] #3 A test shows a sweep over many idle cells making one LIST per cell, and a write to any of them (a segment, a reference, a record) making the next pass recover it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::passed: the cell's collector lease <C>gc/lease holds what the last pass to end recorded (format, epoch, holder, Passed {listing digest, options, deferred deletes with due times, usage}); taking the lease reads it with no request more and carries it on; a pass records with a CAS on its own taking.
2. Digest: SHA-256 over the pass's candidate listing (key, size, modified, ETag, version), but <C>gc/ (lease, marks).
3. collect_cell: take lease (passable only where no lease), list, skip if the recorded digest and options match: delete deferred deletes once due (fence check, record with no digest), else nothing; otherwise pass in full and record. Mark-only and dry runs never skip nor record.
4. Unsettled passes record no digest: a live clone being made of a point in another cell, a fork point found lost, a recovery found lost then ok.
5. Pass::fates: Stays/Goes/GoesAt(due) replaces doomed's age checks; deferred = GoesAt in the future.
6. Fork-point state cache: decide (OPEN_QUESTIONS) after checking what it saves.
7. Tests: idle sweep = lease GET+PUT and one LIST per cell; segment/reference/record/deletion each make the next pass full, then idle again; deferred deletes once due; unsettled clone case; digest unit tests.
8. doc-7 (Passes that skip, safety argument), OPEN_QUESTIONS decisions; check, clippy, nextest, scripts/test.sh, Linux check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), batch with TASK-395.8 on branch t3/395-incremental-usage (base t3/395-per-cell-collector bb07304a6).
Landed:
- extent_remote::passed: <C>gc/lease holds {format, epoch, holder, Passed}. Passed = digest of the pass's candidate listing (SHA-256 over key, size, modified, ETag, version; <C>gc/ left out), options, deferred deletes with due times, retired references (dead cell), usage. Taking the lease reads and carries it; a full pass records it at its end with a CAS on its own taking (fenced: nothing).
- collect_cell: lease (passable() only when no lease yet), list, skip when digest and options match: no reads; deletes deferred deletes once due (fence check, then records no digest so the next pass goes in full), re-sends a dead cell's retirements. Otherwise full pass + record. Mark-only and dry runs never skip nor record. Without rule candidates_first no early listing, no skip.
- No digest recorded when a pass went by more than its cell: a live clone being made of a point elsewhere, a fork point found lost, a recovery lost-then-ok.
- Pass::fates (Stays/Goes/GoesAt) replaces doomed's age checks.
Found by the simulation (restores seed 53) once dead cells skipped: record_fork that read a child live re-puts its reference after the child's deletion and the reference's retirement; nothing in the dead cell changes, so the reference (and the parent's fork-point checkpoint) would stay for good. Blind spot: no direct test re-created a retired reference. Fix: every pass over a dead cell, skipping or not, retires again (recorded in the lease). Class test: passes_skip_only_what_the_cell_alone_decides re-creates the reference and sees the next skipping pass retire it (red with the re-send removed). Structure: the race is in who writes references; filed TASK-395.14 to make re-writing impossible so dead cells cost lease+listing only.
Also found by Shuttle: the early listing taken for the skip check satisfied the gate of a_pass_keeping_what_a_child_made_as_it_lists_uploads_needs_candidates_first, hiding the mutation; fixed by taking no early listing with the rule off.
Safety argument (doc-7, Passes that skip): everything under the prefix is in the listing; outside it: a foreign parent's checkpoint at the fork point (create-only), children reading (no effect; references under the prefix), the cell's own fork-point states (own compactions/checkpoints), leases (under the prefix), a point's record (-> no digest), time (deferred). A skipping pass = the recorded pass's deletes made late; Bucket.tla unchanged.
Not done: fork-point states not cached (decision; TASK-395.12 filed).
Tests (collector::tests): idle_cells_cost_a_listing_until_written (6 cells, each idle = GET+PUT lease + 1 LIST; a segment, a reference, a record, a lease takeover, a deletion each make the next pass full, then idle again), passes_skipping_delete_what_was_deferred_once_due, passes_skip_only_what_the_cell_alone_decides; passed::tests (digest parts, lease counting itself, formats refused). Hand mutants (reverted): digest by keys only -> takeover case red; clone-being-made ignored -> red; retirement re-send removed -> red. Patch own-extents-in-flight-collected re-anchored (sim kills it, seed 12).
Verified (macOS): cargo nextest -p extent_remote -p extent_store -p fxdax 821/821 (sim excluded); scripts/test.sh parts: clippy + workspace 2333/2333 (--no-fail-fast), sim 59/59, Shuttle 37+10+6+33+10+18, scripts 312 OK, mutants 0 broken, go ok. Linux cross-check (zig) clean. Flake TASK-328 seen (fails 3/6 alone with skipping disabled too; noted there).
ACs: #3 proven. #1 holds for every cell but dead ones with children in other cells, which pay a lease GET and a DELETE more (TASK-395.14): left unchecked. #2: recorded under the prefix, read by any process, never a candidate (tested), but no fork-point states cached by decision: left unchecked.
<!-- SECTION:NOTES:END -->
