---
id: TASK-79.48
title: Unbinding the driver frees the device under a mounted Fxfs
status: Done
assignee: []
created_date: '2026-10-21 20:59'
updated_date: '2026-10-31 02:23'
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
virtio_extent_find takes a use, virtio_extent_put gives it back: a superblock and fxfs_device hold one each. vext_remove waits (idle) for the uses to go, the filesystem working meanwhile, finding no more, then lets the queues go unless a follower's close did (a second del_vqs would free its IRQ twice). What it frees waits on a wait queue of its own, not the device's. Tests: fxfs-guest-checks' unbind_waits_for_unmount (waits, works, ENOENT, binds again) and fxfs-guest-follow-remount's follower unbinding a closed device. Arm64 kernel builds; GCE r291 pending.

GCE r292: fxfs-guest-checks and -dax, with unbind_waits_for_unmount, and fxfs-guest-follow-remount passed. r294 is the mutant without the wait.

Mutant GCE r296, no wait in vext_remove: unbind_waits_for_unmount fails, 'unbound while mounted'.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Finding a device takes a use; unbinding waits for its uses, the filesystem working meanwhile; a closed device's queues aren't let go twice. unbind_waits_for_unmount and follow-remount's unbind of a closed device, r292; without the wait 'unbound while mounted' (r296). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
