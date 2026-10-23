---
id: TASK-79.41
title: Resizes are neither serialized nor matched to their answers
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 229000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Concurrent resize_held calls can leave the store smaller than the guest was told. In the VMM, Resizing::answered unconditionally clears a newer shrink and adopts a stale answer's capacity.

Where: crates/fxdax/src/store_requests.rs:137, crates/fxdax/src/extentd/server.rs:735, vendor/firecracker/src/vmm/src/devices/virtio/extent/mod.rs:257, vendor/firecracker/src/vmm/src/devices/virtio/extent/mod.rs:273, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:640.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.6, TASK-37, TASK-37.2, TASK-43.6. TASK-44.6 (Done) built store_requests.rs resize_held under Shuttle but did not serialize concurrent resizes. No task covers the VMM's Resizing::answered taking stale answers.
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
