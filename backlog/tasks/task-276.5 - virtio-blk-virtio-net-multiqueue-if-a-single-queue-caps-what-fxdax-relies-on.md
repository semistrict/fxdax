---
id: TASK-276.5
title: >-
  virtio-blk / virtio-net multiqueue, if a single queue caps what fxdax relies
  on
status: To Do
assignee: []
created_date: '2026-11-09 03:54'
labels:
  - firecracker
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 707000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Upstream Firecracker virtio-blk and virtio-net are single-queue. Only worth a general (upstreamable) change if phase 1 shows ext4 on virtio-blk (or net) at a single-queue ceiling that matters to fxdax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Done as a general Firecracker change, default behaviour unchanged when not configured, or skipped with the measured reason recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
