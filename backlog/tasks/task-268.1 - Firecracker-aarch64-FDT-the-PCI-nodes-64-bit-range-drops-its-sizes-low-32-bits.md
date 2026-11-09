---
id: TASK-268.1
title: >-
  Firecracker aarch64 FDT: the PCI node's 64-bit range drops its size's low 32
  bits
status: To Do
assignee: []
created_date: '2026-11-09 03:08'
labels:
  - firecracker
  - bug
dependencies: []
parent_task_id: TASK-268
priority: low
ordinal: 701000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Upstream Firecracker v1.16.1's create_pci_nodes (src/vmm/src/arch/aarch64/fdt.rs) writes the 64-bit window's size cells as (MEM_64BIT_DEVICES_SIZE >> 32) and ((MEM_64BIT_DEVICES_SIZE & 0xffff_ffff) >> 32): the second is always 0, so the size's low 32 bits are lost. Harmless while MMIO64_MEM_SIZE is 256 GiB, a multiple of 4 GiB, and the extent device's window BAR lies in that range on aarch64 (TASK-273), so it matters only if the layout changes. Found reading the aarch64 PCI path for TASK-273; report it upstream, or fix it in the series as a general fix, with a test of the node's ranges.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The PCI node's 64-bit range size is (size >> 32, size & 0xffff_ffff), tested; or reported upstream with a link here
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
