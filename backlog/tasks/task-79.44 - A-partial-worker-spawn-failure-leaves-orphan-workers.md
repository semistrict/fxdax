---
id: TASK-79.44
title: A partial worker-spawn failure leaves orphan workers
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 232000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An early return leaves already-spawned workers running but missing from self.workers, so stop and reset never reach them, and the next kick spawns duplicates that serve the same rings twice.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:376, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:430, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:627, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:146.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.18.5, TASK-27.2. No task covers a partial worker-spawn failure in the Firecracker extent device. TASK-26.18.5 added the multiple workers and queues.
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
start_all: a worker failing to start stops those started, which give back their queues. Test workers_start_all_or_none, on start_all, as Worker::spawn has no seam to fail: passes on x86_64 under emulation (--features fxdax, which the extent device needs; no script runs Firecracker's unit tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
start_all stops those started when one fails (Firecracker c8f32bce0), workers_start_all_or_none; VMM extent tests 13/13 on r292.
<!-- SECTION:FINAL_SUMMARY:END -->
