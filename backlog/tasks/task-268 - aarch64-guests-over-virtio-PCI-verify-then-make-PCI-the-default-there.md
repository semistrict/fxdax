---
id: TASK-268
title: 'aarch64 guests over virtio-PCI: verify, then make PCI the default there'
status: To Do
assignee: []
created_date: '2026-11-08 22:31'
labels:
  - firecracker
dependencies: []
priority: low
ordinal: 698000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-267 put x86-64 VMs on virtio-PCI by default and left aarch64 on MMIO (guest/tests/test_cases/src/vm.rs Transport): fxdax has no arm64 KVM host (GCE nests KVM on Intel only; CI is x86-64), so no aarch64 guest has run over PCI. The aarch64 guest config has PCI_HOST_GENERIC, VIRTIO_PCI, PCI_MSI and GICv3 ITS, and Firecracker v1.16 wires PCI on aarch64 (linux,pci-probe-only in the FDT), so it should work, unverified. The extent device's DAX window over PCI is a BAR in the 64-bit MMIO gap, which on aarch64 also needs the FDT ranges to cover it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An aarch64 guest suite (fxfs-guest-checks, snapshot) passes with FXDAX_VIRTIO_TRANSPORT=pci on an arm64 KVM host
- [ ] #2 Transport defaults to PCI on aarch64 once it does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
