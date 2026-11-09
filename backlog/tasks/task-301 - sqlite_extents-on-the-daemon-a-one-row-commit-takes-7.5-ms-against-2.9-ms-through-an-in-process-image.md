---
id: TASK-301
title: >-
  sqlite_extents on the daemon: a one-row commit takes 7.5 ms, against 2.9 ms
  through an in-process image
status: To Do
assignee: []
created_date: '2026-11-09 10:48'
labels: []
dependencies: []
priority: high
ordinal: 701000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run docs/measurements/gce-sqlite-bench-2026-11-09T1038Z-6ba6c6a9 (TASK-300): single-row transactions commit at 130/s (p50 7.5 ms) on a store the extent daemon serves, against 340/s through the in-process image and 327-394/s for stock SQLite (rollback, synchronous=FULL); WAL FULL does 1,050/s. A commit is the extent's WRITE, a durable COMMIT with the root, the frees after it, and the window's MAP_MANY of what changed: four round trips and the daemon's durable commit. Where the time goes is not yet measured.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A perf profile on GCE says where a one-row commit's time goes on the daemon
- [ ] #2 One-row commits on the daemon at least as fast as through the in-process image, shown by sqlite-bench on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
