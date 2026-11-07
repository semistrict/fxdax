---
id: TASK-165.10
title: >-
  Measured and written down: doc-7 rewritten, the perf gate against main before,
  the lines removed counted
status: Done
assignee: []
created_date: '2026-11-05 03:17'
updated_date: '2026-11-07 19:15'
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
- [x] #1 doc-7 describes the host as it is: WAL, part cache, one recovery; no local lineage or marker remains in it
- [x] #2 The perf gate on GCE (pgbench tps, first read of /usr, fork and restore latency, memory per VM) against main before the restructure, recorded in docs/measurements; a regression past noise is a task or a dead end
- [x] #3 The lines removed and added, by module, recorded in TASK-165's notes against doc-14's estimates; the review's section 2 marked done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Real bucket against file bucket, 2026-11-06 (main f81174090, GCE r740-r744, jailed): every guest case's time on GCS and on a file bucket, the daemons' time per request (open 884 ms vs 15 ms, fault 28 ms vs 0.31 ms, read 2.2 ms vs 0.076 ms, commit 23 ms vs 4.5 ms) and their bucket request counts: docs/measurements/real-bucket-guest-suites-2026-11-06.md. What it found: TASK-165.11 (durable commits and opens on GCS), TASK-165.12 (pgbench-dax-forked's sum after its restart; no part cache in the test daemons), TASK-209 (fixed).

The tidy pass's list of what's stale (its report to the user, 2026-11-06, before TASK-165.9; to re-check against the code):
- doc-7: still describes local forks, claims and handles, and the local NVMe cache as the store's own files; says format 1 (now 2), 32 uploads in flight (now 16) and multipart uploads (none); its cache section gets the eviction policy, the floor and the read-ahead size wrong; cites 'doc-11, rule 3', which is rule 1 (extent_remote/src/layout.rs:20 likewise).
- doc-14: its 'Today' columns describe deleted code; says format 6 (8 after step 9) and that copy_bases stays (removed); two function signatures wrong; missing decisions: a child takes its state in O(extents) rather than sharing it in memory, Record::Acknowledged, <image>.bucket; Remote::unfork remains.
- Stale code comments: fxdax-extentd.rs:12, extentd.proto:197 (handles), storage/mod.rs:84, storage/kept.rs:42, pages.rs:5.
- And: record the user's acceptance of TASK-165.9's trade-offs in doc-14.

Done (land-x2 9bbf94459 on main, CI 42401937262 green; macOS workspace 2103/2103 on its tree). AC #1: doc-7 rewritten for the host as it is: the WAL, the part cache, one recovery; no local lineage, index, marker or claim; format 2, 16 uploads in flight, no multipart, the cache's least-recently-read eviction, 4 GiB floor and 16-part read-ahead, doc-11 rule 1. doc-14 brought to the code (local format 8, the paths, the replay's signatures, Remote::unfork staying, copy_bases gone, the decisions: a child's state in O(extents), Record::Acknowledged, <image>.bucket) with the user's acceptance of TASK-165.9's trade-offs. The stale comments the tidy pass listed fixed, each re-checked (fxdax-extentd.rs, extentd.proto's ForkStore, storage/mod.rs, storage/kept.rs, pages.rs, host_store.rs's take_branch, doc-11 rule 3 -> 1 in layout.rs, lib.rs and extent_frame). AC #2: docs/measurements/restructure-before-after-2026-11-07.md, the perf gate, fork bench and snapshot cases on main before TASK-165.6 (aee7cad93, GCE r914), before 165.9 (394fe7535, r910, r912) and after (213504c4b, r911): every gate passes after; forked DAX 1,309 tps against 826 pre-restructure, whose two failing gates now pass; a VM 557 MiB cheaper with DAX; forks 12 ms against 18 and 26; restores 29-39 ms against 46-50; bucket requests per workload. The one regression past noise, a cold host's page-ins of a fork's data (the first read of /usr 3.6 s against 3.1), is TASK-225's. AC #3: the lines removed and added by module in TASK-165's notes against doc-14's estimates (extent_store's code 10,757 -> 9,580, remote.rs 1,687 -> 178, about 2.4k fewer in all against 7-8k estimated), doc-14's What went, and the 2026-11-04 review's section 2 marked done. DoD: macOS workspace; Linux workspace, sim, Shuttle, guest and go in CI 42401937262, the guest suites on GCE in r910-r914; scripts/upstream-changes.py --check passes on main 9bbf94459; no upstream file changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
doc-7 rewritten for the host as it is since TASK-165 and doc-14 brought to the code, with the user's acceptance of 165.9's trade-offs; the stale comments fixed; the perf gate, forks and restores measured on main before the restructure, before 165.9 and after (docs/measurements/restructure-before-after-2026-11-07.md: every gate passes, forked DAX 58% faster, forks 12 ms; a cold host's fork page-ins slower, TASK-225's); the lines removed counted in TASK-165's notes. Verified by CI 42401937262 on land-x2 9bbf94459, the macOS workspace and GCE r910-r914.
<!-- SECTION:FINAL_SUMMARY:END -->
