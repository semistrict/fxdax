---
id: TASK-27.1
title: 'Firecracker: virtio shared memory regions on the MMIO transport'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 23:42'
updated_date: '2026-10-16 00:44'
labels: []
dependencies: []
parent_task_id: TASK-27
priority: high
type: feature
ordinal: 76000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's virtio extent driver finds its DAX window with virtio_get_shm_region (virtio 1.2's SHMSel/SHMLen/SHMBase MMIO registers), which libkrun's MMIO transport implements and Firecracker's doesn't. Keeping the driver unchanged across VMMs needs them in Firecracker, with the region's guest-physical space and KVM memory slot allocated as Firecracker's pmem device does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A Firecracker virtio device can expose shared memory region 0 at a guest-physical address it allocates, backed by host memory in a KVM memory slot
- [ ] #2 The MMIO transport answers SHMSel, SHMLenLow/High and SHMBaseLow/High as the virtio 1.2 spec says, with length all-ones for a region that doesn't exist
- [ ] #3 Firecracker's unit tests for the MMIO transport cover the registers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. VirtioDevice::shm_regions (default none) and VirtioShmRegion in devices/virtio/device.rs.
2. MMIO transport: SHMSel 0xac (write/read), SHMLen 0xb0/0xb4 and SHMBase 0xb8/0xbc reads of the selected region, length -1 when none; reset clears the selection; unit test test_shm_regions.
3. KvmMemSlot moved from pmem into vstate/vm.rs, shared by pmem and the extent DAX window.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: VirtioDevice::shm_regions + VirtioShmRegion; MMIO SHMSel/SHMLen/SHMBase (0xac-0xbc), length -1 when absent, reset clears selection (test_shm_regions passes). KvmMemSlot moved to vstate/vm.rs (VmError::NoKvmSlotAvailable), pmem maps errors as before; pmem tests pass in Lima.
<!-- SECTION:NOTES:END -->
