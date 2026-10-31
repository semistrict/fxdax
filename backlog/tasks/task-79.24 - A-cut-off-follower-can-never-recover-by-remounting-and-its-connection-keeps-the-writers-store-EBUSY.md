---
id: TASK-79.24
title: >-
  A cut-off follower can never recover by remounting, and its connection keeps
  the writer's store EBUSY
status: Done
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 212000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The device reopens a follower's store only on a reset followed by activation, but the driver exports no reset and mount never triggers one. A remount reuses the cut-off connection and fails with ESTALE forever, and the open connection makes the writer's reopen fail with EBUSY.

Where: guest/linux/drivers/virtio_extent/driver.c:734, guest/linux/drivers/virtio_extent/driver.c:657, guest/linux/include/linux/virtio_extent.h:248, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:409.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.3, TASK-42.6, TASK-42.2. Not tracked. TASK-42.3 defines ESTALE once cut off and reopening only on a device reset. TASK-42.6 is the guest follower mount. Nothing gives the guest a reset path or covers the EBUSY on the writer's reopen.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. driver: virtio_extent_close at unmount resets a follower's device, dropping its connection; virtio_extent_open at mount reopens, -EBUSY retriable (reset_prepare/reset_done; kernel builds on 6.18). Firecracker: reset drops the store at once, reopen at activation. fxfs-guest-follow-remount(-dax) NOT YET RUN; vendor/firecracker change uncommitted too.

Mutant GCE r297, no virtio_extent_close at unmount: fxfs-guest-follow-remount fails, the next writer's open EBUSY for as long as it tries. r293: follow-remount and -dax 3 rounds each.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A follower's unmount resets its device, letting the store go; mounting opens it anew. fxfs-guest-follow-remount[-dax] 3 rounds each on r293; without the close at unmount the next writer stays EBUSY (r297). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
