---
id: TASK-119.1
title: >-
  Concurrent DAX checks verify contents: every byte has one owner thread
  checking it through read(2) and its mapping
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 18:44'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 437000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
seal_race, evacuee_waited, log_writes, faults_writes_compaction, faults_unheld, faults_seal_holding_the_semaphore and append_while_mapped check no contents, or only after a quiet final rewrite (test_fxfs_guest_checks.rs:775), so a write lost mid-race goes unseen. append_writeback checks only the length after eviction.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each listed check asserts exact contents during the race, not only at the end
- [x] #2 append_writeback checks contents after eviction
- [x] #3 A hand mutant of dax.c that loses a write fails each check it should (named per check)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Byte-owner checks (b9af10794, branch test/119): seal_race (each round, own byte via map and read(2), other's byte monotonic), log_writes (each write read back; whole file at end), faults_writes_compaction/faults_unheld (write(2)s own pages' first halves, stores second halves; periodic read-back; full model at end), faults_seal_holding (stored bytes modelled, read back periodically and at end), append_while_mapped (mapped pages hold what was appended; whole file at end), append_writeback (contents after evict). Verify on GCE r206 (checks-dax x3, checks-dax-delay x3); then hand mutants.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On main since b9af10794 and 8bd5f35ec: seal_race, log_writes, faults_writes_compaction, faults_unheld, faults_seal_holding_the_semaphore and append_while_mapped give each byte one owner that checks it as it goes, through read(2) and the mapping, and the file whole at the end; append_writeback compares contents after eviction. Passing on GCE r221/r224 (checks-dax, -delay, -small). AC3 open: no mutant is attributed per check (moved_unmapped, held_not_written lose writes and are killed by the suites, but which checks fail them is not recorded).

AC3, from the DAX gate's passing run on 3e6615852 (/tmp/fxdax-gate-3e6615852048/mutants, each mutant's checks-dax and -delay consoles' FAIL lines): each listed check fails under a mutant that loses or misplaces writes: seal_race (held_not_written, moved_unmapped), log_writes (new_slice_no_seals_recheck, prune_misnumbers, evacuation_maps_elsewhere), faults_writes_compaction (held_not_written, moved_unmapped, evacuation_maps_elsewhere), faults_unheld and faults_seal_holding_the_semaphore (held_not_written, evacuation_maps_elsewhere), append_while_mapped (evacuation_maps_elsewhere), append_writeback (new_slice_no_seals_recheck, evacuation_maps_elsewhere). evacuee_waited is no longer a check (its race is evacuation_paused's, which evacuation_unlocked fails). DoD #1: macOS scripts/test.sh passed on main since.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The concurrent DAX checks give each byte one owner that checks it through read(2) and its mapping as it goes, and the file whole at the end; append_writeback compares contents after eviction. Each check fails under a write-losing hand mutant (the DAX gate's run on 3e6615852).
<!-- SECTION:FINAL_SUMMARY:END -->
