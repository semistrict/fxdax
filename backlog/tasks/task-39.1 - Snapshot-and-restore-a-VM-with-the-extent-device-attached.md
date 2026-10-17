---
id: TASK-39.1
title: Snapshot and restore a VM with the extent device attached
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 111000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Firecracker refuses to snapshot a VM with the extent device (vendor/firecracker/src/vmm/src/device_manager/persist.rs: 'extent-store devices do not support snapshotting'), so zygote restore, suspend-to-RAM, live migration and memory snapshots all break. A snapshot must capture the device's state (queues, in-flight requests, the DAX window's mappings of extentd memory) and pin the store at the point the guest's memory reflects; a restore must reattach to that store, or a fork of it, and remap the window.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Firecracker snapshots and restores a VM with the extent device, DAX window included, and the restored guest reads and writes its filesystem, shown by a guest test
- [ ] #2 A snapshot names the store point it needs (a fork made at the snapshot), so restoring twice from one snapshot gives two independent VMs (zygote), shown by a test
- [ ] #3 Suspend-to-RAM and live migration to another host (store taken over through object storage) work, shown by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
