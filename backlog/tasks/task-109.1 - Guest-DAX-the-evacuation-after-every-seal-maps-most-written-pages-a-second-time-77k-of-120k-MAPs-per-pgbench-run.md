---
id: TASK-109.1
title: >-
  Guest DAX: the evacuation after every seal maps most written pages a second
  time (77k of 120k MAPs per pgbench run)
status: To Do
assignee: []
created_date: '2026-10-25 23:26'
updated_date: '2026-10-30 04:15'
labels: []
dependencies: []
parent_task_id: TASK-109
priority: high
ordinal: 410000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-91.10's evacuation (fxfs_dax_evacuate_work) moves every sealed staging extent's pages into its range's own space with MAP_MANY after each seal; the writes after the seal restage. Under an fsync per transaction a page is mapped once as staging and again as an evacuee: per 60 s pgbench run (sandbox team, b746104) 77,010 evacuate and 33,233 restage MAPs of 120,091, and fxfs_dax_evacuee_gather, walking every range of each affected file page by page, is 4% of guest CPU samples. Each MAP costs the VMM ~175 us of host kernel time (TASK-91.16), so the evacuation alone is a large part of DAX's gap to no DAX (TASK-109).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Evacuate MAPs per pgbench-dax 60 s run cut to under a tenth of today's, with the window still not filling (window_full and regions pruned reported before and after)
- [x] #2 fxfs_dax_evacuee_gather no longer walks a file's every range page by page per seal
- [ ] #3 fxfs-guest-checks-dax (regions_evacuated, evacuation_rewrites, seal_race) green on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Hypothesis: much of the evacuation maps pages rewritten right after (the WAL's tail page, written at every commit, moves to the next staging region at the next write, so its evacuation MAP is wasted), and pages written once (WAL) are rarely read through the window again. Experiment exp/evacuate-delay (5492cf523, not for main): fxfs.dax_evacuate_delay_ms, the evacuation skipping regions sealed less than that long ago. r112: pgbench-dax, 1 VM, 0 / 200 / 2000 ms.

r112 (exp/evacuate-delay 5492cf523, pgbench-dax 1 VM, per run): delay 0: 1104/1027/1006/961/958 tps, evacuate MAPs 79-92k, MAP requests 99-110k, regions ~300-440. 200 ms: 1154/1092/1036/1002/981, evacuate 73-89k, requests 56-69k, regions ~530-690. 2000 ms: 1213/1144/1058/1011/1031, evacuate 75-94k, requests 58-72k, regions 1,700-2,500 (regions' pool 680-740 MB free, never full). The hypothesis is wrong: evacuated pages are not rewritten soon after (the WAL tail isn't what's evacuated); they're cold, and deferring doesn't cut their MAPs. Deferring batches them: MAP requests -40%, tps +5-10% (one VM each, so a hint). Dropping cold pages instead of mapping them isn't possible as ranges stand: a page not set in range->mapped reads as a hole (fxfs_dax_iomap_begin's read path), so a dropped page would need its range forgotten. One MAP per written run is what the staging design costs, written-at-own-space or evacuated alike; cutting the count needs the host to place a seal's staging pages at their ranges' addresses as part of the seal's remap (a protocol change), or each MAP made cheaper (TASK-91.16).

Parked (2026-10-25): placing a seal's staging pages at their ranges' addresses inside the seal would put ~2 host mappings (~0.35 ms) on every fsync's critical path, where the async evacuation now costs none; the per-MAP host cost (TASK-91.16) is the root and is worked first.

AC2 on main (88ded8774): fxfs_dax_set_moved counts each range's pages moved and keeps FXFS_DAX_RANGE_MOVED (XA_MARK_1) on it in the file's ranges while it has any; fxfs_dax_evacuee_gather walks xa_for_each_marked. Verified at 3364435c6 (the batch): macOS extent_remote and fxdax 371 passed; Linux workspace on GCE r264 1965 of 1965; guest suites on GCE r263 (release), fxfs-guest-dax, -checks-dax, -crash-dax, -checks-dax-small, -checks-dax-delay, -pgbench-dax 2 rounds each passed (pgbench-dax 1051.6 -> 863.4, 997.4 -> 889.5 tps).
<!-- SECTION:NOTES:END -->
