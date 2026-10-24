---
id: TASK-79.107
title: >-
  Guest DAX: a fault's keep=true map_write writes through the device into a
  staging extent the window may not have opened yet
status: To Do
assignee: []
created_date: '2026-10-24 14:08'
labels:
  - bug
dependencies: []
parent_task_id: TASK-79
priority: medium
ordinal: 381000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found bisecting TASK-79.11 (2026-10-24): having txn_map_write write a fault's page into its moved block through the device killed the VM in fxfs-guest-checks-dax (Firecracker: vcpu run: Bad address). fxfs_dax_iomap_write passes keep=true when it can't allocate the held copy, and txn_map_write then writes the old blocks' contents into the new ones the same way: device WRITEs to the staging extent, which may not be open in extentd yet (the restage maps it after a seal, in the background), so the write goes to the store and not the open extent's memory the window maps. Likely the same failure, on a rare path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test drives a fault's keep=true path (the held allocation failing, by fault injection) and shows the failure
- [ ] #2 Device writes to a staging extent before the window opens it are coherent with the window's later writable mapping
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
