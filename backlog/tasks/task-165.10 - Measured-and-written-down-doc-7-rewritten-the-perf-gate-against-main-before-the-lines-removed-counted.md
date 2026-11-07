---
id: TASK-165.10
title: >-
  Measured and written down: doc-7 rewritten, the perf gate against main before,
  the lines removed counted
status: To Do
assignee: []
created_date: '2026-11-05 03:17'
updated_date: '2026-11-07 11:09'
labels: []
dependencies:
  - TASK-165.9
documentation:
  - docs/measurements/real-bucket-guest-suites-2026-11-06.md
parent_task_id: TASK-165
priority: medium
ordinal: 632000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 10. The restructure trades local copies for the part cache and the bucket; its cost must be measured by the gate the DAX work used, and doc-7 still describes local lineages, local checkpoints and markers.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 doc-7 describes the host as it is: WAL, part cache, one recovery; no local lineage or marker remains in it
- [ ] #2 The perf gate on GCE (pgbench tps, first read of /usr, fork and restore latency, memory per VM) against main before the restructure, recorded in docs/measurements; a regression past noise is a task or a dead end
- [ ] #3 The lines removed and added, by module, recorded in TASK-165's notes against doc-14's estimates; the review's section 2 marked done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Real bucket against file bucket, 2026-11-06 (main f81174090, GCE r740-r744, jailed): every guest case's time on GCS and on a file bucket, the daemons' time per request (open 884 ms vs 15 ms, fault 28 ms vs 0.31 ms, read 2.2 ms vs 0.076 ms, commit 23 ms vs 4.5 ms) and their bucket request counts: docs/measurements/real-bucket-guest-suites-2026-11-06.md. What it found: TASK-165.11 (durable commits and opens on GCS), TASK-165.12 (pgbench-dax-forked's sum after its restart; no part cache in the test daemons), TASK-209 (fixed).

The tidy pass's list of what's stale (its report to the user, 2026-11-06, before TASK-165.9; to re-check against the code):
- doc-7: still describes local forks, claims and handles, and the local NVMe cache as the store's own files; says format 1 (now 2), 32 uploads in flight (now 16) and multipart uploads (none); its cache section gets the eviction policy, the floor and the read-ahead size wrong; cites 'doc-11, rule 3', which is rule 1 (extent_remote/src/layout.rs:20 likewise).
- doc-14: its 'Today' columns describe deleted code; says format 6 (8 after step 9) and that copy_bases stays (removed); two function signatures wrong; missing decisions: a child takes its state in O(extents) rather than sharing it in memory, Record::Acknowledged, <image>.bucket; Remote::unfork remains.
- Stale code comments: fxdax-extentd.rs:12, extentd.proto:197 (handles), storage/mod.rs:84, storage/kept.rs:42, pages.rs:5.
- And: record the user's acceptance of TASK-165.9's trade-offs in doc-14.
<!-- SECTION:NOTES:END -->
