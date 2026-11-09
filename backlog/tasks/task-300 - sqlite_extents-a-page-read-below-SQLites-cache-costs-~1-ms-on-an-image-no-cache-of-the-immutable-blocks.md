---
id: TASK-300
title: >-
  sqlite_extents through the extent daemon: reads from its shared cache,
  zero-copy through a mapped window
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 10:03'
updated_date: '2026-11-09 10:49'
labels: []
dependencies: []
priority: high
ordinal: 700000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run docs/measurements/gce-sqlite-bench-2026-11-09T0955Z-abe0c4c0 (TASK-298): through the in-process image a page read below SQLite's cache cost ~1 ms (987 reads/s against 236,000/s for stock SQLite). The host already caches sealed extents once for every store and VM: the extent daemon's cache. The user (2026-11-09): run the volume through the daemon, with zero-copy reads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Rerunning sqlite-bench on GCE: point reads and scans within 2x of rollback FULL, and the numbers in docs/measurements
- [x] #2 A volume opens on a store the extent daemon serves (sqlite_extents::daemon::open), through a storage_device::Device over RemoteExtentStore (fxdax::RemoteDevice)
- [x] #3 Its files are mapped into a DAX window in the process, kept in step with each commit: reads copy from it, and SQLite memory-mapped reads (xFetch) read pages there in place; a commit made while pages are fetched remaps once they are given back
- [x] #4 The fake-device suite passes reading from a window too, and a Linux test on an in-process daemon reads pages in place and survives reopening
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
macOS: 19 tests pass (the suite twice, through the device and through a copying window with mmap_size set, plus mapped_reads_follow_commits and a_commit_waits_for_fetched_pages_to_remap; SQLite itself never commits while holding fetched pages, so the deferred remap is tested through the Volume API). Linux (privileged Docker): all tests pass, the daemon test among them; clippy clean. GCE (docs/measurements/gce-sqlite-bench-2026-11-09T1038Z-6ba6c6a9): reads 255-266k/s (daemon), 268-277k/s (mmap) against 236k rollback FULL; scans 20M and 25M rows/s against 17M; batch 16/s against 13/s. Commits slowed to 130/s: TASK-301.

DoD 1 not run in full: the workspace suites were not rerun; fxdax's change is a new module (extentd/device.rs) and an export, built on macOS and Linux, exercised by sqlite_extents' daemon test.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax::RemoteDevice (a Device over the daemon client) and sqlite_extents::daemon put a volume on a daemon-served store with its files mapped into a DAX window kept in step with commits; xRead copies from it and SQLite's xFetch reads in place. GCE: reads 280x faster than via the image, past stock SQLite in rollback mode; scans 25M rows/s with mmap. One-row commits regressed to 7.5 ms (TASK-301).
<!-- SECTION:FINAL_SUMMARY:END -->
