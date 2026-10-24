---
id: TASK-92.17
title: >-
  The 200 ms stats poll and every metrics scrape walk each store's whole extent
  map, base layers included, under its state lock
status: In Progress
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 12:29'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 292000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
follower_stats calls Store::usage() only to read kept_for_views (extents.rs:771-773). usage holds the state mutex while Extents::iter collects every extent, including the inherited base chain (store.rs:2554-2571; layers.rs:311-318, 165-181). The stats thread does this for every store, writers included, every 200 ms, whether stats are configured or not (stores.rs:636, 653-662, 693-696), and every scrape repeats it (599-608). At hundreds of forks this costs 5-90% of a core plus a ms-scale lock hold on every VM's store 5x/s, breaking TASK-34's rule (mod.rs:30-33).

Where: crates/fxdax/src/extents.rs:771-773, crates/extent_store/src/store.rs:2554-2571, crates/extent_store/src/layers.rs:165-181, crates/extent_store/src/layers.rs:311-318, crates/fxdax/src/extentd/stores.rs:599-608, crates/fxdax/src/extentd/stores.rs:636, crates/fxdax/src/extentd/stores.rs:653-662, crates/fxdax/src/extentd/stores.rs:693-696, crates/fxdax/src/extentd/metrics.rs:156, crates/fxdax/src/extentd/mod.rs:30-33.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. follower_stats calls Store::usage() only to read kept_for_views (crates/fxdax/src/extents.rs:739-741). usage holds the state lock while summing over state.extents.iter(), base layers included (crates/extent_store/src/store.rs:1467-1480), though info() already reads kept.for_views() (1462-1465). The stats thread runs it for every store every 200 ms whether or not a stats file is set (crates/fxdax/src/extentd/stores.rs:636, 653-666, 683-700). follower_views does it again on every scrape (598-608). Related: TASK-42.7, TASK-34, TASK-47, TASK-72.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): fixed on branch review/r2b-extentd (48 commits on main b5eb2fc8, rebased onto 709a7017), not on main. Verified on Linux (GCE): workspace 2614/2614 and every Shuttle suite; guest fxfs-guest, -dax, -checks, extent-guest, -follow, -live-fork, -daemon-lost, -snapshot, -snapshot-dax, -pgbench, -pgbench-dax OK. fxfs-guest-checks-dax failed on TASK-79.104's guest crash (fixed since on main), and fxfs-guest-follow-torture failed 1 of 3 on main alone and 1 of 3 with these commits (TASK-79.58's flake). Blocked only on a push: the branch's vendor/firecracker points at 6610d88f1, three commits on semistrict/firecracker fxdax-v1.16.1 past 1fbc852f9 (535defc6e TASK-79.41, 8ea641a6d and 6610d88f1 TASK-92.13), in /Users/ramon/src/fxdax/.claude/worktrees/r2bm/vendor/firecracker; push those first, then the branch to main, rebasing onto main (guest-only changes since).
<!-- SECTION:NOTES:END -->
