---
id: TASK-91.2
title: >-
  Never hold the guest's seal across an object-storage wait: durable commits
  split from sealing
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-22 22:29'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 267000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ExtentDevice::commit (guest/rust/fxfs-linux/src/device.rs) holds its Seal until the COMMIT completes; for COMMIT_DURABLE, ExtentStore::commit_and_seal replies after make_durable, which waits for the upload backlog (Remote::wait_uploaded). Every journal compaction ends in a durable root commit (commit_root), so the guest's fsyncs and DAX stores stop for as long as the backlog takes: 21-43 s of each 60 s pgbench run at 25 ms, and tps fell 764 to 441. doc-2 asks durability only of sync, syncfs and unmount.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A durable commit's object-storage wait happens after the guest's seal ends (or root commits are ordered, made durable later)
- [ ] #2 At 25 ms object-store latency, pgbench run 2's tps is within 10% of run 1's, and COMMIT requests take under 5% of a run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81351c5: root commits ordered (durable only within a sync). pgbench at 25 ms on GCE: DAX 606/404/304 tps (was 398/187/93), no DAX 837/598/431 (was 522/440/424); crash, follow, daemon-lost, live-fork and the default suite pass. Still decaying: extentd logs 'commits wait for object storage: 1 not uploaded', ordered commits stalling at the lag bound (max_lag_bytes / max_lag commits) inside the guest's seal. Remaining: (a) the stall must not be inside the seal, or not happen (TASK-91.3: the uploader keeping up); (b) sync/unmount's durable wait still holds the seal: needs a non-barrier request waiting for durability (protocol: guest driver, Firecracker device, extentd).

4317177, fd7a384: a checkpoint read each segment's moves serially, every 64 segments, inline in the uploader; at 25 ms that stopped uploads for tens of seconds by pgbench's third minute and commits stalled at the lag bound inside the seal. Now one listing per prefix, and reads of only the moves there are. f283bb0: the test daemon has 8 GiB, as the sandbox team's (the 1 GiB default caused fault/page-in churn). GCE, pgbench -s 50 at 25 ms: without DAX 979 / 1,010 / 1,014 tps (AC2 met: no decay, commit wait ~0); with DAX 1,001 / 862 / 171: upload lag and commit wait ~0 throughout, the fall is the DAX window filling (TASK-91.10). AC2 stays open on DAX until TASK-91.10. Still open here: sync/unmount's durable wait inside the seal (needs a non-barrier durability request).
<!-- SECTION:NOTES:END -->
