---
id: TASK-140
title: >-
  Guest DAX: soft lockup in fxfs_dax_map_write under xfstests generic/111,
  kernel WARNs at dax.c:2074 and fs/dax.c:461
status: Done
assignee: []
created_date: '2026-10-31 16:32'
updated_date: '2026-11-03 02:10'
labels:
  - bug
  - dax
dependencies: []
priority: high
ordinal: 486000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r342, xfstests-fxfs-guest-dax: generic/111 (reflinks): xfs_io stuck 26 s in fxfs_dax_map_write, which loops: soft lockup, panic. Before it, generic/013 (fsstress): WARNING fs/fxfs/dax.c:2074 fxfs_dax_assert_mapped_within: mapped pages past a region's, from fxfs_dax_map_chunk's fxfs_dax_move, in a write; generic/075 (fsx): WARNING fs/dax.c:461 dax_associate_entry from a read fault: a page in two files' entries, reflinks under DAX, which dax_associate_entry asks to be shared. Logs: /tmp/fxdax-r342-xfstests/e2e/xfstests-fxfs-guest-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 generic/013, 075 and 111 pass under DAX, no WARN, no lockup
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: the hang is a livelock, caught with sysrq l every 300 s (GCE r357, 14 minutes in, xfs_io in one pwrite: fxfs_dax_write_iter, dax_iomap_rw, fxfs_dax_iomap_write, fxfs_dax_reserve_write, fxfs_dax_map_write, in all three samples). The test's files are small, and non-DAX it takes 0-1 s. fxfs_dax_new_slice could make a slice of an open extent whose region stayed read-only (fxfs_dax_region_open: already mapped): Fxfs leaves the blocks in place, fxfs_dax_map_slice refuses them (-EXDEV), the write drops the slice, and the chunk is made the same slice again, with no bound. Such a chunk now gets a range of its own; the retry is bounded and takes a fatal signal.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed the generic/111 livelock (a slice that could never take a write is no longer remade without bound, 826183635) and the dax.c:2074 and fs/dax.c:461 WARNs (8af34848c); 7caa43578 later removed the cause, as shared blocks no longer enter regions. Verified by GCE r347 and r430 (generic/013, 075, 111 pass, no WARN, no lockup) and the guest/sim mutant task-140-read-only-slice.
<!-- SECTION:FINAL_SUMMARY:END -->
