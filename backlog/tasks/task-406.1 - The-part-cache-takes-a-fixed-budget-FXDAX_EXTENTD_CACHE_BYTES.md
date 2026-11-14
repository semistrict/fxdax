---
id: TASK-406.1
title: 'The part cache takes a fixed budget: FXDAX_EXTENTD_CACHE_BYTES'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-14 12:17'
updated_date: '2026-11-14 14:11'
labels:
  - sandbox
  - cache
dependencies: []
parent_task_id: TASK-406
priority: high
ordinal: 793000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The daemon opens its part cache with a capacity of `u64::MAX` (`remote_cache` in crates/fxdax/src/extentd/stores.rs), so it grows until 4 GiB of its filesystem are free. The sandbox team keeps it on the SSD their sandbox stores use, and needs it to take no more than a fixed budget. `Cache::open` already takes a capacity; nothing sets it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 With `FXDAX_EXTENTD_CACHE_BYTES=N`, the part cache keeps at most N bytes of objects (parts and tails), evicting those read least recently; the 4 GiB floor of free space still holds
- [x] #2 Unset or empty, the cache is bounded only by the floor, as before
- [x] #3 A value that is not a positive number of bytes stops the daemon at start with an error naming the variable
- [x] #4 A test shows a daemon-opened cache capped by the variable; the daemon docs and doc-7 name it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extentd: parse FXDAX_EXTENTD_CACHE_BYTES at start (fxdax-extentd.rs, as FXDAX_EXTENTD_UPLOAD_MEMORY), into a process-wide setting in stores.rs that remote_cache passes to Cache::open as its capacity; u64::MAX when unset.
2. Test in stores.rs tests: remote_cache with the budget set keeps no more than it after keeping more parts than fit.
3. Docs: fxdax-extentd.rs header, doc-7.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: extentd::stores::tests::the_cache_takes_its_budget (capacity from the variable), budgets_the_cache_cant_take_are_refused (0, -1, 1G, x, no cache dir), stores_opened_at_once_share_the_cache (unset: u64::MAX), tests/peers.rs a_cache_budget_the_daemon_cant_take_stops_it (the binary exits naming the variable); eviction within a capacity is extent_remote::cache's existing tests. Also: Cache::capacity(), a 'cache:' line in the stats file and fxdax_extentd_part_cache_bytes / _capacity_bytes in /metrics. Suites: macOS scripts/test.sh clippy+workspace+shuttle all pass (workspace 2365/2365); ubu1 clippy pass, workspace 2643/2644 (the one failure is TASK-315, failing at main 123a2cbb4 too), shuttle parts all pass; upstream-changes --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FXDAX_EXTENTD_CACHE_BYTES caps the daemon's part cache (Cache::open's capacity, which was u64::MAX); unset it is bounded by the 4 GiB floor as before; a bad value, or one without FXDAX_EXTENTD_CACHE, stops the daemon at start. The cache's bytes and budget are in the stats file and /metrics. Documented in fxdax-extentd's docs and doc-7. Verified by unit tests and a test of the binary on ubu1; full suites on macOS and ubu1 (only TASK-315, pre-existing, fails).
<!-- SECTION:FINAL_SUMMARY:END -->
