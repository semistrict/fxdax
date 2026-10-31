---
id: TASK-110
title: >-
  Flaky: extentd::tests::hundreds_of_writers_flush_on_threads_they_share exceeds
  its 200 ms durable-commit lag under the full Linux workspace run
status: Done
assignee: []
created_date: '2026-10-26 03:47'
updated_date: '2026-10-30 18:15'
labels:
  - bug
dependencies: []
priority: low
ordinal: 413000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r123 (19b799cc1, full Linux workspace): '300 stores' guests committing: 46 threads kept, [14, 32, 0] ...; a guest's 9 ordered commits durable within 86 ms, the median, and 249.6 ms', failing 'lag < 200 ms' (four 50 ms turns). It passed in r113, r115b, r115c and r121 (-p fxdax, or the workspace), the same code. The bound is wall-clock under nextest's parallel load (extent_remote's sims and zircon-core-tests run beside it); after the panic, teardown logged 'uploading branch ...: closing the open segment: No such file or directory' and an EIO making the last commits durable, the test's directory gone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The lag bound holds under the full workspace's load, or is stated relative to what the load allows, without the test losing what it checks (four turns of the flush threads)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Again in r141 (9f77e3799, GCE n2-standard-8, full workspace): 257.4 ms (median 57 ms), 300 stores.

2026-10-29, OrbStack arm64 container: failed alone too, 2 of 3 runs passing at a27e50fd8+1 (lags 220, 319 ms, the bound 200), 3 of 3 at a27e50fd8; the test makes no window.

On main (aff4349fd): ExtentStore counts its daemon's flush turns (flush_turned, from flush_behind each turn); the test bounds the probe's lag at 4 of its store's turns, however long the load made them, and reports the time. Verified at 3364435c6 (the batch): macOS extent_remote and fxdax 371 passed; Linux workspace on GCE r264 1965 of 1965; guest suites on GCE r263 (release), fxfs-guest-dax, -checks-dax, -crash-dax, -checks-dax-small, -checks-dax-delay, -pgbench-dax 2 rounds each passed (pgbench-dax 1051.6 -> 863.4, 997.4 -> 889.5 tps).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed on aff4349fd: the test bounds the probe's lag at 4 of its store's flush turns, however long load makes them. Verified: Linux workspace on GCE r264, 1965 of 1965.
<!-- SECTION:FINAL_SUMMARY:END -->
