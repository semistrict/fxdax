---
id: TASK-147
title: 'Guest DAX: generic/638''s write at 8192 ends in EIO after 256 dropped ranges'
status: Done
assignee: []
created_date: '2026-11-01 00:53'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
priority: medium
ordinal: 500000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r363 and r368: 'a write at 8192 found no range to take it', in generic/638, which passes all the same. Not a slice: a file refused twice has no_slices set and it still happens, so -EXDEV comes from elsewhere, perhaps fxfs_dax_iomap_write's 'end <= pos', a range mapped too small. Before TASK-140's bound this would have hung for ever.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The write's -EXDEV is explained, and it succeeds or fails with a right errno
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A write past EOF records how far it grows the file, so ranges a fault maps meanwhile reach its end (loff_t growing, e14371f81; xfstests generic/638); guest/sim's mutant task-147-growing puts the bug back.
<!-- SECTION:FINAL_SUMMARY:END -->
