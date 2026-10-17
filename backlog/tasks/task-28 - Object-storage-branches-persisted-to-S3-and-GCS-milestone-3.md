---
id: TASK-28
title: 'Object storage: branches persisted to S3 and GCS (milestone 3)'
status: To Do
assignee: []
created_date: '2026-10-17 01:31'
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
- [ ] #1 A branch on GCS, and on S3 (RustFS in tests), survives the loss of its host, losing at most the configured lag, shown by a test
- [ ] #2 A branch opens on a host with no local copy of it and serves its guest from object storage, shown on GCE
- [ ] #3 Freed extents are reclaimed in object storage, forks included, shown by the simulation and on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
