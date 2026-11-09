---
id: TASK-276.5
title: >-
  virtio-blk / virtio-net multiqueue, if a single queue caps what fxdax relies
  on
status: Done
assignee: []
created_date: '2026-11-09 03:54'
updated_date: '2026-11-09 09:18'
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
- [x] #1 Done as a general Firecracker change, default behaviour unchanged when not configured, or skipped with the measured reason recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Skipped (phase 1, GCE c3-standard-8): ext4 on single-queue virtio-blk scales cold 4K random reads 27-30k (1 job) -> 48-89k IOPS (8 jobs) and stays above fxfs at every point (fxfs-guest 13-20k); random writes 22-44k. No single-queue ceiling that limits fxdax: fxdax's guests store through the extent device, not virtio-blk, and the control is not what caps them. Upstream Firecracker stays as it is.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Skipped: ext4 on single-queue virtio-blk scales cold reads to 48-89k IOPS at 8 jobs and stays above fxfs at every point; no single-queue ceiling that limits fxdax.
<!-- SECTION:FINAL_SUMMARY:END -->
