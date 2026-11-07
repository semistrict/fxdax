---
id: TASK-67.2
title: >-
  A daemon's stores opened at once, its first, may each open the cache, and all
  but one fail EIO
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 12:53'
updated_date: '2026-11-07 23:54'
labels: []
dependencies: []
parent_task_id: TASK-67
priority: high
ordinal: 647000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67), GCE r755 round 9 (run FXDAX_SEED=14204650337043807922, round seed 14819699323713146238): the daemon killed at 179 s and started again, its five VMs booted again at once, and one, slot 1's, exited 1 at boot: opening its store failed EIO, the daemon logging `FXDAX_EXTENTD_CACHE: .../cache/fxdax-parts: another cache has it`. remote_cache (crates/fxdax/src/extentd/stores.rs:1211-1228) took the OnceLock's value if set, else opened the cache and then get_or_init: two stores opening at once each opened one, and the second's flock of the directory (crates/extent_remote/src/cache.rs:563-569) failed against the first's, in the same process.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Stores opened at once share one cache, shown by a test red before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A test: sixteen threads at a barrier ask remote_cache with FXDAX_EXTENTD_CACHE set; all get the one cache (red).
2. The first opening under a mutex, those meanwhile waiting for it; a failure the caller's, the next trying again.
3. Linux: the test in Docker; GCE workspace; the soak.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed: remote_cache opens the cache under a Mutex<Option<Option<Arc<Cache>>>>, the first caller opening it and the others waiting, a failure not kept. Test: extentd::stores::tests::stores_opened_at_once_share_the_cache, red in Linux Docker before (a cache: Errno(EIO)), green three times after; GCE r756 Linux workspace 2223/2223.

Blind spot: every test opens its daemon's first store from one thread, or sets the cache before others (handover's, the e2e daemon's first store is a control call's). Only a daemon started again under running VMs opens many at once, which no case did before the soak killed one with five VMs booting again.

Looking around: remote_bucket (stores.rs:1053) has the same check, open, get_or_init shape; opening the bucket twice is harmless (the loser is dropped, no lock), so left. Other OnceLocks in crates/fxdax are get_or_init closures already.

Structure: the class is a resource held exclusively, made lazily by check-then-init; the mutex makes the check and the making one step. Nothing above it needed changing.

Finalized in the backlog sweep: its commits are on origin/main. DoD #1 by Main CI 42423368353 at 878aa8848 (which holds this work) green, macos (workspace) and test (workspace) among it; scripts/upstream-changes.py --check passes on main (2026-11-08 sweep).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed on main (0de2dbff2): remote_cache opens the cache once, under one mutex, those opening meanwhile waiting for it (crates/fxdax/src/extentd/stores.rs:1319-1336), where stores opened at once each opened it and all but one failed its directory's flock with EIO. stores_opened_at_once_share_the_cache (stores.rs:1347) red before in Linux Docker, green after. Main's CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
