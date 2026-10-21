---
id: TASK-79.52
title: Thaw takes INFO's capacity into block_count but never resizes the allocator
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
ordinal: 240000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
statfs follows the new capacity, but the allocator keeps the old device_size, and later resizes see no difference. This is latent: a restore currently rebuilds its config from the snapshot, so the capacity cannot differ yet.

Where: guest/rust/fxfs-linux/src/device.rs:789, guest/rust/fxfs-linux/src/fs.rs:428, guest/rust/fxfs-linux/src/fs.rs:463, guest/rust/fxfs-linux/src/fs.rs:485, vendor/firecracker/src/vmm/src/devices/virtio/extent/persist.rs:225.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-37.4, TASK-39.1, TASK-26.19.7. Latent. No task covers thaw adopting INFO's capacity without resizing the allocator. TASK-37.4 added set_device_size for online resize. TASK-39.1 added thaw reading INFO.
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
