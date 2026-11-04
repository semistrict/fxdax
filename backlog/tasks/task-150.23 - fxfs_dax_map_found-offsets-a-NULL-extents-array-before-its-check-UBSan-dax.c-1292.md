---
id: TASK-150.23
title: >-
  fxfs_dax_map_found offsets a NULL extents array before its check (UBSan,
  dax.c:1292)
status: Done
assignee: []
created_date: '2026-11-03 16:34'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: low
type: bug
ordinal: 604000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the hunt (TASK-160.28), run 41667054404, mutant task-99-region-open in: failures 52299 200 failed with 'dax.c:1292:51: runtime error: applying non-zero offset 8624 to null pointer'. fxfs_dax_map_found computes pieces = lagging ? found + capacity : found before 'if (!found) return -ENOMEM', so a failed fxfs_dax_alloc_extents with lagging is pointer arithmetic on NULL: undefined, though the kernel's -fno-delete-null-pointer-checks keeps the check. Main without the mutant passes seed 52299 (run 41675269814): the mutant's interleaving reached the allocation failure with lagging, which main's seeds haven't. Replay: KERNEL=<tree> BUILD=<dir> guest/sim/zloop.sh -s 0 -p mutants/task-99-region-open.patch, then $BUILD/mutants/zloop/failures 52299 200. Same seed hunt, interleave 54042 200 read zeros, which main without the mutant also passes (run 41675274637): taken as the mutant's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pieces is computed after the NULL check, so no failed allocation offsets NULL
- [ ] #2 A sim test reaches an extents allocation that fails with lagging, red with the order back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxfs_dax_map_found takes its lagging pieces only once the allocation is known to have succeeded (7ae639eac); failures seeds 408 and 448 at 1,000 operations were UBSan-red before.
<!-- SECTION:FINAL_SUMMARY:END -->
