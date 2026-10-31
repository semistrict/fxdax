---
id: TASK-79.52
title: Thaw takes INFO's capacity into block_count but never resizes the allocator
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
Freezer::unfreeze no longer takes INFO's capacity: resize() compares the device's with what the filesystem has, and unfreeze_fs runs the resize work once thawed, flushed, as if told, so the allocator grows. A store found smaller isn't taken (asked is how a shrink comes). Test: fxfs-guest-suspend-dax grows the store while the VM is away (control API), the guest says GROWN in statvfs and fallocates past the old capacity. GCE r291 pending; mutant pending.

GCE r292: fxfs-guest-suspend-dax passed, statvfs at the grown size and fallocate past the old capacity. r295 is the mutant.

Mutant GCE r297: statvfs still says the old size with no resize at thaw. r299 is the code before the fix exactly, INFO's capacity taken, which statvfs follows and only the allocator shows.

GCE r299: the code before the fix passed the test as it stood: fallocate reserves nothing, so the allocator's bound never came into it. The test now writes 1.25x the old capacity, on a 128 MiB store. r300 the fix, r301 the code before it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Thaw runs the resize work, flushed, rather than taking INFO's capacity into block_count. fxfs-guest-suspend-dax writes past the old capacity, r300; the code before the fix hits ENOSPC there (r301). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
