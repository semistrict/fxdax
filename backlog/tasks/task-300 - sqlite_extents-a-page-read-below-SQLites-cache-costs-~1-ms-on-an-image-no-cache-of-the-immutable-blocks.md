---
id: TASK-300
title: >-
  sqlite_extents: a page read below SQLite's cache costs ~1 ms on an image: no
  cache of the immutable blocks
status: To Do
assignee: []
created_date: '2026-11-09 10:03'
labels: []
dependencies: []
priority: high
ordinal: 700000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run docs/measurements/gce-sqlite-bench-2026-11-09T0955Z-abe0c4c0 (TASK-298): point reads 987/s through the VFS against 236,000/s for stock SQLite on ext4, scans 250x slower, and 500-update transactions 3/s against 13/s, all from reads of pages SQLite's 2 MiB cache misses: each goes to the image (ImageDevice::read_with_opts), whose acknowledged data is read through its bucket's fetcher. The extents are write-once, so blocks cached by (extent, block) never go stale.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reads of blocks the volume read lately are answered from memory, bounded by an option
- [ ] #2 Rerunning sqlite-bench on GCE: point reads and scans within 2x of rollback FULL, and the numbers in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
