---
id: TASK-79.93
title: >-
  Without DAX, pgbench's segment closes are few, full and slow, and the upload
  lag reaches its bound: bisect the review's fixes
status: To Do
assignee: []
created_date: '2026-10-23 19:35'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 360000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.90 (r5/regress). On main after the review's fixes, no-DAX pgbench closes 312-347 segments a 60 s run at 150-199 ms each, where 66d56a43 (before them) closed about 475 per 30 s at 8-10 ms; and the upload lag peaks at 805-1024 MB a run, reaching max_lag_bytes (1 GiB) with cleaning off too (the guest tests' bucket is in memory). With cleaning off these cost no tps (1054-1083), but the lag makes commits wait for object storage. Candidates by reading, not shown: 213aa35e-equivalent (TASK-79.19: bounded uploads in flight, closing the open segment only when fewer segments wait than may upload) and 21d2be98-equivalent (TASK-92.15); reverting the latter alone did not bring the small closes back. Bisect between 66d56a43 and 1f6c8685 (241 commits, 97 in extent_store, remote.rs, extents.rs, extentd, host_store).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The commit that makes the closes few and full, and the lag reach its bound, is found
- [ ] #2 The lag stays under its bound under pgbench without DAX, or the bound is shown to be the uploader's real limit at 25 ms latency, with a test or bench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
