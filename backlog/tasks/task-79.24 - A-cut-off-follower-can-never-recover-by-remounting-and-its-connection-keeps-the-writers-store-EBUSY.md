---
id: TASK-79.24
title: >-
  A cut-off follower can never recover by remounting, and its connection keeps
  the writer's store EBUSY
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
