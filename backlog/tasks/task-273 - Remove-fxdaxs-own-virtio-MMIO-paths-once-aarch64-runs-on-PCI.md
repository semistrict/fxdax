---
id: TASK-273
title: >-
  Remove fxdax's own virtio-MMIO paths: every fxdax VM on virtio-PCI, x86-64 and
  aarch64
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 02:26'
updated_date: '2026-11-09 03:06'
labels:
  - firecracker
dependencies: []
ordinal: 700000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
PCI is the default on x86-64 since TASK-267 (docs/measurements/virtio-pci-vs-mmio-2026-11-09.md: requests ~40% faster, boot and restore unchanged). The user decided (2026-11-09) to remove fxdax's MMIO-only code now, on both architectures, without waiting on TASK-268: there are no deployments, so no snapshot taken over MMIO needs restoring, and no compatibility shim or migration is kept.

- fxdax: FXDAX_VIRTIO_TRANSPORT goes; every launcher passes --enable-pci on x86-64 and aarch64 alike; docs stop describing MMIO as an option.
- Firecracker series: no MMIO shared-memory (SHM_SEL) registers, no MMIO extent-device attach/save/restore, no window placement past the MMIO gap; the extent device refuses MMIO attach clearly. Upstream's MMIO transport stays exactly as upstream has it, and a build without the fxdax feature stays upstream's (resources/seccomp, swagger, .cargo unchanged from v1.16.1; test_config_space_as_upstream passes).
- aarch64 goes to PCI untested on KVM (fxdax has no arm64 KVM host): it must compile, and its guest kernel config must have what Firecracker's aarch64 PCI path needs (PCI, PCI_HOST_GENERIC, VIRTIO_PCI, GICv3 ITS, PCI MSI). Running it is TASK-268's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxdax has no FXDAX_VIRTIO_TRANSPORT and no MMIO launch path: every Firecracker launch passes --enable-pci on both architectures
- [ ] #2 The Firecracker series has no fxdax-only MMIO code (SHM registers, MMIO extent attach/save/restore, placement past the MMIO gap); an extent device on MMIO is refused with a clear error; a build without fxdax is upstream's
- [ ] #3 aarch64 compiles (fxdax and Firecracker, both ways) and its guest kernel config has PCI, PCI_HOST_GENERIC, VIRTIO_PCI, PCI_MSI and GICv3 ITS; what stays unverified at runtime is recorded on TASK-268
- [ ] #4 GCE: Firecracker KVM tests, the 11 PCI guest cases, then release and debug guest rounds pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Firecracker: a new release of the series, fxdax-v1.16.1-r2 (branch fxdax-v1.16.1-pci), rather than removal commits on fxdax-pci-r1: the MMIO shared-memory commit is in the series itself, and the extent device would otherwise be added MMIO-only, made PCI-capable, then made PCI-only. r2 has the four general PCI commits with the other general fixes before the feature (VirtioShmRegion and shm_regions introduced by the PCI shared-memory commit, ungated), the MMIO commit cut to device types of u32, and the extent device PCI-only from its commit on: its window always a BAR in the 64-bit MMIO gap (no window::Place), no MMIO save/restore, MMIO attach refused (StartMicrovmError::ExtentDevicesNeedPci). Upstream's MMIO transport untouched but for its test DummyDevice.
2. fxdax: vm.rs passes --enable-pci always, Transport and FXDAX_VIRTIO_TRANSPORT gone; test.sh's filter loses test_shm_regions; fxdax.config drops CONFIG_VIRTIO_MMIO, so a device left on MMIO is not found; README and the measurement doc; vendor/firecracker.rewrites names 517d438c5's rewrite.
3. aarch64: Docker arm64 series check both ways; the guest config has PCI, PCI_HOST_GENERIC, PCI_ECAM, VIRTIO_PCI, PCI_MSI, ARM_GIC_V3_ITS; Firecracker's FDT pci node (ranges covering the 64-bit gap, msi-map to the ITS, linux,pci-probe-only) checked by reading. Unverified at runtime recorded on TASK-268.
4. Verify: series check (Docker, every commit both ways); GCE held x86 host: FXDAX_FIRECRACKER_TESTS KVM tests, then the 11 PCI guest cases, then release and debug rounds. Then rebase, push to main, CI green incl. firecracker-series.
<!-- SECTION:PLAN:END -->
