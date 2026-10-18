---
id: TASK-28
title: 'Object storage: branches persisted to S3 and GCS (milestone 3)'
status: Done
assignee: []
created_date: '2026-10-17 01:31'
updated_date: '2026-10-18 21:33'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
priority: high
type: feature
ordinal: 80000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2 milestone 3: the host keeps every branch in object storage, so durability goes beyond one disk, capacity is cheap, and a VM can resume or fork on another host (m-6). doc-7 is the agreed design, from a survey of SlateDB, Neon, Graft, ZeroFS, lsvd, JuiceFS, mountpoint-s3, Litestream, AutoMQ and others. Today the extent store (crates/extent_store) is a local directory whose layout mirrors the remote one, and forks copy it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A branch on GCS, and on S3 (RustFS in tests), survives the loss of its host, losing at most the configured lag, shown by a test
- [x] #2 A branch opens on a host with no local copy of it and serves its guest from object storage, shown on GCE
- [x] #3 Freed extents are reclaimed in object storage, forks included, shown by the simulation and on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Evidence (2026-10-18). AC1: remote::tests::branches_survive_their_hosts_loss: cut off from object storage, the host's store commits until the lag bound (max_lag 2) and then waits; lost then (store, uploader and disk), another host's empty store takes the branch: every durable commit, at most the lag's commits lost, and it writes on. In memory, on RustFS (scripts/test-rustfs.sh) and on GCS from a GCE host (FXDAX_S3_URL=gs://echophase-fxdax-bench/tests). AC2: scripts/migrate-gce.sh (TASK-28.11): a VM restored on a second GCE host, which had nothing of its store, from GCS, filesystem intact; and the 18 guest cases with branches on GCS. AC3: the simulation (extent_remote sim: forks_and_frees, forks_racing_collection, bases_deleted_under_live_forks, ...) and store::tests::branch_lifecycle, extended: the collector keeps what a fork inherited while it holds it, and reclaims the objects once the parent and the fork freed them; red with the fork not freeing them. Run on GCS from GCE (rustfs test with FXDAX_S3_URL=gs://...).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every branch is kept in object storage (S3 and GCS through object_store): subtasks 28.1-28.11. Shown: a branch survives its host's loss losing at most the lag (in memory, RustFS, GCS), opens and serves its guest on a host without it (GCE, two hosts through GCS), and what it and its forks freed is reclaimed (simulation, and the collector on GCS).
<!-- SECTION:FINAL_SUMMARY:END -->
