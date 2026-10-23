---
id: TASK-79.13
title: >-
  Mount fails after a legal shrink: Allocator::open checks stale persisted
  allocated_bytes against the new, smaller device size
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 201000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
set_device_size admits a shrink by checking live usage, but mount runs upstream's check of allocated_bytes as of the last compaction against the new size, before replay. Write, compact, delete, shrink and reboot, and the mount fails with 'Allocated bytes exceeds device size' until the store is grown back.

Where: crates/fxfs/src/object_store/allocator.rs:1236, crates/fxfs/src/object_store/allocator.rs:2151, crates/fxfs/src/object_store/allocator.rs:2400, crates/fxfs/src/object_store/allocator.rs:1121, guest/rust/fxfs-linux/src/fs.rs:464.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-37, TASK-37.4. No task covers mount failing after a legal shrink. Online shrink and the mounted allocator resize were TASK-37 and TASK-37.4 (Done). Their remount test does not hit the compact, delete, shrink order.
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

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A filesystem shrunk legally after a write, compaction and delete now mounts again. Upstream's check of allocated bytes against the device size moved from Allocator::open, where it read the counts from the last compaction, to on_replay_complete, which uses the counts replay leaves. The move is marked as an fxdax deviation in the ported allocator. Verified by a remount test that failed before the fix with 'Allocated bytes exceeds device size', and by a test that a device really too small is still refused as Inconsistent. Both pass on macOS and Linux (GCE), and so do the guest tests.
<!-- SECTION:FINAL_SUMMARY:END -->
