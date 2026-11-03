---
id: TASK-150.8
title: A short DAX write zeroes pages it never wrote
status: Done
assignee: []
created_date: '2026-11-01 16:51'
updated_date: '2026-11-03 07:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 523000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. dax.c:2800 and :3284 pass keep = false; crates/fxfs/src/object_store/data_object_handle.rs:2148-2161 copies nothing for whole pages. Blocks a write fully covers move to new zeroed blocks before the copy, so a source faulting partway leaves zeros where old data was, and a source mapping the same page copies zeros over itself and reports success. test_short_copy is excluded on DAX.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [x] #2 Fixed, the test red again with the fix taken out
- [x] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [x] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03, verified on main (dcbdd6247 plus the mutants alone). Fixed by d8d068da0: a write has its bytes in kernel memory before blocks move that lose what they held (fxfs_dax_source_needed, -ENOTBLK, fxfs_dax_source_take). The fix taken out (scripts/guest-mutants-gce.py moves_before_bytes) fails fxfs-guest-checks-dax at test_write_from_same_page and test_short_copy, GCE fxdax-test-t18-ac, us-east1-b: killed. With it, fxfs-guest-checks and fxfs-guest-checks-dax pass on the same host. Blind spot: no check wrote from the file's own mapped pages, and test_short_copy was excluded on DAX, so a write's copy stopping short, or reading its own moved pages, was never tried where whole pages move before their bytes. Tests of the class: test_write_from_same_page and test_short_copy (DAX now too), guest/sim's short_copy, short_copy_pages and same_page scenarios, and the moves_before_bytes mutant. Search around it: a fault keeps what it moves (held, TASK-79.11), and zeroing is meant to lose it; the class's structural cause, the move committed before the copy, is TASK-153.5.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A write whose whole pages move before it has their bytes no longer loses them: d8d068da0 has the bytes in kernel memory first. Verified 2026-11-03 on main: the fix taken out (mutant moves_before_bytes) fails test_write_from_same_page and test_short_copy; with it, fxfs-guest-checks and fxfs-guest-checks-dax pass (GCE fxdax-test-t18-ac). DoD: workspace on Linux at ab17d2d87 (GCE r530, 2,169 of 2,169) and macOS; upstream-changes --check passes; dax.c is fxdax's own.
<!-- SECTION:FINAL_SUMMARY:END -->
