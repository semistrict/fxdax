---
id: TASK-79.48
title: Unbinding the driver frees the device under a mounted Fxfs
status: To Do
assignee: []
created_date: '2026-10-21 20:59'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 236000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
virtio_extent_find takes no reference, and remove neither waits for nor fails requests in flight. Waiters hang, and later submits use freed devm memory.

Where: guest/linux/drivers/virtio_extent/driver.c:74, guest/linux/drivers/virtio_extent/driver.c:734, guest/linux/include/linux/virtio_extent.h:248.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.5, TASK-77. No task covers virtio_extent unbind or remove lifetime.
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
