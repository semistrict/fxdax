---
id: TASK-79.8
title: >-
  A follower's device reset leaves the old connection's arena pages mapped in
  its DAX window
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-27 13:51'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 196000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
reopen() drops the old store before the queues validate and the new window is created, and only the new stage unmaps the window. If activation fails, for example on an invalid queue, the window keeps pages the daemon frees and reuses for other VMs in the arena, which the guest can then read. The client's Drop also leaves the receiver thread applying queued STAGE_MAP/UNMAP to the same memory. And the window is guest-writable anonymous memory from VM build until activation.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:409, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:331, vendor/firecracker/src/vmm/src/devices/virtio/extent/window.rs:72, crates/fxdax/src/extentd/client.rs:490, crates/fxdax/src/dax/uffd.rs:549, crates/fxdax/src/extentd/server.rs:214.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.3, TASK-39.2, TASK-42.9. Not tracked. TASK-42.3 (In Progress) added 'a reset follower reopens its store at activation', where this defect lives. TASK-39.2 is the tenant-isolation goal it breaks. TASK-42.9 (Done) changed the stage's withholding of unmapped pages.
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. UffdSecondStage::release: the whole window replaced with PROT_NONE anonymous memory, and no change made after (a released flag under the pages lock), so the receiver thread's late STAGE_MAPs land nowhere.
2. RemoteExtentStore's Drop releases the window first.
3. Firecracker: the window's host memory PROT_NONE until a store takes it over; activation reopens a follower's store only after its queues initialize.
4. Tests (Linux): extentd windows_let_go_of_the_daemons_pages_as_their_connection_goes, uffd released_stages_change_nothing.
<!-- SECTION:PLAN:END -->
