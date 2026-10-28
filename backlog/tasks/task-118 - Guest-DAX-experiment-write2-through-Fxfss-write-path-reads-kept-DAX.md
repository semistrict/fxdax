---
id: TASK-118
title: 'Guest DAX: experiment, write(2) through Fxfs''s write path, reads kept DAX'
status: Done
assignee: []
created_date: '2026-10-28 12:47'
updated_date: '2026-10-28 13:00'
labels:
  - perf
dependencies: []
ordinal: 435000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Idea 1 of a review of the DAX write gap (2026-10-28): read-write pgbench DAX 1364 tps vs no-DAX 1791 vs ext4-dax 1496. No-DAX commits cost 250 us vs DAX 483 us: a write(2) + fsync is a WRITE and a COMMIT, nothing to seal, no restage or evacuation MAP, no file->sem hold (17.8 s/run of read waits). Experiment: module param fxfs.dax_write_fxfs makes every write(2) to a DAX inode copy into a buffer and call fxfs_write, after dropping the chunks it touches (as punch does), so they map again from where Fxfs put the data. mmap stores keep the DAX path. If pgbench-dax tps reaches no-DAX, build a heuristic (fsync-per-write streams) instead of all writes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs.dax_write_fxfs=1 implemented; DAX suites pass with it on
- [x] #2 pgbench-dax 5 runs with it on vs off on GCE, tps and read waits recorded
- [x] #3 Decision recorded: heuristic worth building or not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r205 2026-10-28 (branch exp/118-write-fxfs, 97eb31c2d; fxfs.dax_write_fxfs=1): pgbench-dax tps 609/261/235/236/311 vs 1172/1085/1022/983/983 off (same host, same tree). Cause: each write(2) drops its 2 MiB chunk, so reads map it again: window_maps_range ~1.4M per 60 s run, window_maps_ns ~35 s per run, window_unmaps_ns ~13 s per run. Data correctness held: checks-dax under dax_delay_us=200 passed every data check; only mechanism asserts failed (regions_evacuated; fxfs-guest-dax read_maps 19 vs 17 MAPs), crash-dax/follow-dax/extent-guest pass. Decision: not worth a heuristic in this form; a write path that bypasses the window needs per-page invalidation of the window (unmap just the written pages, map them again from Fxfs's new blocks) to avoid the chunk remaps, which is close to what the window write path already does. AC1 not checked: suites don't pass with it on, by design.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Experiment negative: write(2) through Fxfs with chunk drops cut pgbench-dax to ~25% (chunk remaps). Parked on exp/118-write-fxfs.
<!-- SECTION:FINAL_SUMMARY:END -->
