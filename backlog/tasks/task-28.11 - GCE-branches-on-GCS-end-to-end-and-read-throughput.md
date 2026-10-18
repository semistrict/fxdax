---
id: TASK-28.11
title: 'GCE: branches on GCS end to end, and read throughput'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-18 20:21'
labels: []
milestone: m-5
dependencies:
  - TASK-28.9
  - TASK-28.10
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: task
ordinal: 91000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 decision 9 and Testing: perf and real-store behaviour are measured on GCE only. The part size (256 KiB to 4 MiB) and GETs in flight are chosen by measurement.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The guest suites pass with branches on GCS
- [x] #2 A VM resumes on a second GCE host from GCS and its filesystem is intact
- [x] #3 Ranged-GET throughput per host and the part-size sweep are recorded in docs/measurements, with the chosen settings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. test-gce.sh: FXDAX_EXTENTD_REMOTE (gs://) forwarded, under a prefix of the run's own; run the guest suites with branches on GCS.
2. Snapshot test Migrate mode: FXDAX_MIGRATE_PHASE source/target with a bundle (snapshot, memory, rootfs); the bucket from the environment. scripts/migrate-gce.sh: two c3 hosts, source on one, bundle through GCS, target on the other.
3. fxdax-gets: ranged-GET throughput by part size and GETs in flight; scripts/gets-gce.sh records it in docs/measurements, with the chosen settings.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC1: test-gce.sh with FXDAX_EXTENTD_REMOTE=gs://echophase-fxdax-bench/e2e (per-run prefix), FXDAX_GCE_STORAGE=1: 18 guest cases pass (extent-guest, fxfs-guest(-format, -crash, -dax, -crash-dax), resize, live-fork, follow(-dax), import(-dax), snapshot(-dax), suspend-dax, migrate-dax, checks(-dax)); the daemon's bucket counters: 287 creates, 122 ranged GETs, 7.3 MB of segments uploaded. AC2: scripts/migrate-gce.sh, two c3-standard-8 hosts in us-east1-b: fxfs-guest-migrate-dax's source phase on A (store in GCS, snapshot), the bundle through GCS, target phase on B with nothing of the store: B took the snapshot's point from the bucket (remote markers in its store) and the restored guest read what it wrote before, through the mapping made before the snapshot too, wrote, remounted: OK moved. AC3: docs/measurements/gce-gets-2026-10-18T1925Z (fxdax-gets on c3-standard-8, us-east4): GETs cost ~20 ms up to 1 MiB, the host tops out at ~2.2 GiB/s; settings kept: 1 MiB parts, 64 GETs (64 MiB) in flight. Quotas: the project has 32 vCPUs and 500 GB of SSD per region, so the runs went one after another, in us-east1 for the pair of hosts.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Guest tests keep their branches in GCS with test-gce.sh (FXDAX_EXTENTD_REMOTE, per-run prefix): all 18 cases pass. A VM moves between two GCE hosts through GCS (migrate-gce.sh; the snapshot test's source and target phases): restored on a host with nothing of its store, its filesystem intact. fxdax-gets and gets-gce.sh measure ranged GETs by part size and GETs in flight; recorded in docs/measurements/gce-gets-2026-10-18T1925Z, which keeps 1 MiB parts and 64 GETs in flight.
<!-- SECTION:FINAL_SUMMARY:END -->
