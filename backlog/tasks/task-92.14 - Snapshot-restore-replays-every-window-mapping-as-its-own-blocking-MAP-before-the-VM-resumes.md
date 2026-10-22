---
id: TASK-92.14
title: >-
  Snapshot restore replays every window mapping as its own blocking MAP before
  the VM resumes
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 289000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
map_again issues one call_blocking MAP per recorded mapping (device.rs:494-500; client.rs:411-438), inside activate_confined (425-427), which restore runs synchronously (persist.rs:636-656). A zygote with 500-1,000 mapped regions adds 30-110 ms to every restore, against a few ms for a plain Firecracker restore.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:425-427, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:494-500, crates/fxdax/src/extentd/client.rs:411-438, vendor/firecracker/src/vmm/src/device_manager/persist.rs:636-656.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. ExtentDevice::map_again calls store.map_again for each recorded mapping in sequence (vendor/firecracker/.../extent/device.rs:494-499). It runs inside activate_confined (424-426). Each call is a blocking MAP, via blocking(map_async) or call_blocking for zeros (crates/fxdax/src/extentd/client.rs:478-490). Related: TASK-39.1, TASK-75, TASK-79.43, TASK-91.4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
