---
id: TASK-79.25
title: >-
  A guest can abort its VMM with a chain whose writable lengths sum past
  u32::MAX
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
ordinal: 213000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Buffers::new accepts aliased descriptors and sums the writable lengths with no bound. handle then calls u32::try_from(..).expect(..), and with panic=abort that kills the VMM, which retires its store and cuts its followers off.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:338, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:497, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:503, vendor/firecracker/Cargo.toml:35.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-77, TASK-26.18.5. No task covers Buffers::new summing writable lengths past u32::MAX and the expect that aborts the VMM. TASK-77 (To Do) would fuzz the extent protocol from the guest.
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
