---
id: TASK-283
title: 'guest/sim zloop seed 215500: PANIC in worker0: a punch: -12'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 07:26'
updated_date: '2026-11-09 08:56'
labels:
  - bug
  - sim
dependencies: []
priority: high
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scheduled Hunt run 42554112366 (2026-11-09, at b6ba50fb3), zloop shard 2: seeds 213001-218000 of interleave, 1 failed, not known: 'interleave 215500 200: sim: PANIC in worker0: a punch: -12'. -12 is ENOMEM: a hole punch in the sim's real dax.c failed out of memory. The run's artifact keeps the logs and replay says how to run it again (guest/sim/kernel-tree.sh KERNEL, then replay with KERNEL and BUILD set).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The seed replays and the cause is found
- [x] #2 Fixed, with the seed added to the sim's known regressions or a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replay seed 215500 in OrbStack Docker (guest/sim/linux.sh); find where -ENOMEM comes from.
2. Fix the cause in dax.c; scenario + mutant as the regression.
3. Seed passes; zloop over seeds 214001-217000 and 1-3000; mutants --check; scripts checks.
4. Push to main; CI guest-sim green.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause: the budget's worker (fxfs_dax_relieve) sealed only after writing something back itself. Writeback by other paths (here fxfs_dax_resize_begin's, as the 4 MiB write of interleave's PMD cycle stopped short at ENOSPC from Fxfs failing, and resized) seals nothing after it: its 256 pages (all of private_max 256) were clean, private, in extent 64, open. With nothing dirty the worker never sealed, and fxfs_dax_rebind_range skips pieces not sealed, so 3,054 rounds rebound none; the punch's partial page (fxfs_dax_zero_part) waited FXFS_DAX_NO_ROOM_WAITS rounds in fxfs_dax_throttle and failed -ENOMEM. A real dax.c bug, not a sim limit or a leak: on a guest, pages the flusher or a truncate wrote back would block the budget likewise until some fsync/sync sealed. The page cache's balance_dirty_pages waits for writeback and never fails a write ENOMEM; the worker's job is to make room, so it must seal.
Fix: fxfs_dax_rebind_range counts pages it left private (open extents); fxfs_dax_rebind seals the file's volume and rebinds again when it left some and private is still above target.
Regression: scenario cow_rebind_seals_open_extents (truncate's writeback leaves 7 clean private pages open; dax_rebind_now must rebind them), red before the fix ('the worker left 7 clean pages private'); mutant guest/mutants/task-283-rebind-leaves-open.patch killed by it. task-164.4-rebind-shared regenerated (context moved).
TASK-286 (KCSAN folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite) is unrelated: page-cache folio flags on a weak-memory race the sim can't see; TASK-283 is the DAX budget's sealing.

Validation: seed 215500 passes at 200 and 1,000 operations (was PANIC a punch: -12, trace a41d0ec0d53b9eb3); zloop.sh -f 214001 -s 3000 and -s 3000 (every test and scenario, 3,000 seeds each of interleave and failures): 0 failed; mutants.sh --check 45/0 broken, task-283-rebind-leaves-open and task-164.4-rebind-shared killed; FXDAX_TEST_PARTS=scripts scripts/test.sh passes (upstream-changes.py --check, guest-mutants-gce.py --check). CI 42586721957 at 3354e6aae green, every job (workspace on Linux and macOS, guest-sim, guest-sim-sweep). No GCE run: a C-only change of dax.c, CPU-only, as asked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Root cause: the DAX budget's worker sealed only after writing something back itself. Writeback by other paths (here the resize of a 4 MiB write stopped short at ENOSPC) left all 256 private pages clean in an open extent; with nothing dirty the worker never sealed, rebound none, and a punch waiting for room failed ENOMEM after 3,000 rounds. Fix (3354e6aae): fxfs_dax_rebind_range counts pages it leaves private for open extents, and fxfs_dax_rebind seals the volume and rebinds again when room is still needed. Regression: scenario cow_rebind_seals_open_extents (red before the fix), killing new mutant task-283-rebind-leaves-open. Verified: the seed, zloop around it and 1-3000, mutants, scripts checks, CI green. Unrelated to TASK-286 (a page-cache folio KCSAN race).
<!-- SECTION:FINAL_SUMMARY:END -->
