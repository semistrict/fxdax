---
id: TASK-44.19
title: >-
  Hand mutants of the guest's DAX locks and rechecks: which removed lock no test
  notices
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-25 03:23'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 398000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Before TASK-99's fix, taking region->lock out of fxfs_dax_region_open would have failed no test; cargo-mutants (TASK-44.1) covers Rust only. Removing each lock, gate check and seal recheck in dax.c and device.c, one at a time, and running the DAX suites, shows the code no test guards.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each lock acquisition, gate check and seals recheck in dax.c and device.c is removed in turn and the DAX suites run on it
- [ ] #2 Every surviving mutant gets a test that kills it, or a reason it is equivalent
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r83 (kernel built per mutant from HEAD): killed region_open_unlocked, map_chunk_no_seals_recheck, new_slice_no_seals_recheck, write_no_seals_recheck, seal_keeps_writable, relocate_unmarked, cow_not_shared, held_not_copied_back; MISSED prune_in_use (fxfs_dax_prune without its users == 1 check: a range lets go of regions while another iomap uses it), which needs a test that kills it (AC2).

GCE r88: prune_while_read green x3 on HEAD (pushed, da13b9094), but MISSED the prune_in_use mutant: AC2 still open.

GCE r92: prune_while_read with fxfs.dax_delay_us=300 (and the new delay point between a read's iomap and its read) still MISSED prune_in_use, 6 rounds; green x6 on HEAD. The mutant may be equivalent in practice: a prune that ignores the range's users needs a reader's iomap to point into a region the prune drops, i.e. a page moved to a staging region, evacuated back, and the region's last reference dropped while the read is between iomap_begin and its copy. Next: a check that forces that sequence (write a page, fsync to seal, wait for the evacuation, read with the delay) or record the mutant as unkilled with that reasoning.
<!-- SECTION:NOTES:END -->
