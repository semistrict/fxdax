---
id: TASK-273
title: >-
  Remove fxdax's own virtio-MMIO paths: every fxdax VM on virtio-PCI, x86-64 and
  aarch64
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 02:26'
updated_date: '2026-11-09 04:28'
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
- [x] #1 fxdax has no FXDAX_VIRTIO_TRANSPORT and no MMIO launch path: every Firecracker launch passes --enable-pci on both architectures
- [x] #2 The Firecracker series has no fxdax-only MMIO code (SHM registers, MMIO extent attach/save/restore, placement past the MMIO gap); an extent device on MMIO is refused with a clear error; a build without fxdax is upstream's
- [x] #3 aarch64 compiles (fxdax and Firecracker, both ways) and its guest kernel config has PCI, PCI_HOST_GENERIC, VIRTIO_PCI, PCI_MSI and GICv3 ITS; what stays unverified at runtime is recorded on TASK-268
- [x] #4 GCE: Firecracker KVM tests, the 11 PCI guest cases, then release and debug guest rounds pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Firecracker: a new release of the series, fxdax-v1.16.1-r2 (branch fxdax-v1.16.1-pci), rather than removal commits on fxdax-pci-r1: the MMIO shared-memory commit is in the series itself, and the extent device would otherwise be added MMIO-only, made PCI-capable, then made PCI-only. r2 has the four general PCI commits with the other general fixes before the feature (VirtioShmRegion and shm_regions introduced by the PCI shared-memory commit, ungated), the MMIO commit cut to device types of u32, and the extent device PCI-only from its commit on: its window always a BAR in the 64-bit MMIO gap (no window::Place), no MMIO save/restore, MMIO attach refused (StartMicrovmError::ExtentDevicesNeedPci). Upstream's MMIO transport untouched but for its test DummyDevice.
2. fxdax: vm.rs passes --enable-pci always, Transport and FXDAX_VIRTIO_TRANSPORT gone; test.sh's filter loses test_shm_regions; fxdax.config drops CONFIG_VIRTIO_MMIO, so a device left on MMIO is not found; README and the measurement doc; vendor/firecracker.rewrites names 517d438c5's rewrite.
3. aarch64: Docker arm64 series check both ways; the guest config has PCI, PCI_HOST_GENERIC, PCI_ECAM, VIRTIO_PCI, PCI_MSI, ARM_GIC_V3_ITS; Firecracker's FDT pci node (ranges covering the 64-bit gap, msi-map to the ITS, linux,pci-probe-only) checked by reading. Unverified at runtime recorded on TASK-268.
4. Verify: series check (Docker, every commit both ways); GCE held x86 host: FXDAX_FIRECRACKER_TESTS KVM tests, then the 11 PCI guest cases, then release and debug rounds. Then rebase, push to main, CI green incl. firecracker-series.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed: 751ced7b5 (code, submodule fxdax-v1.16.1-r2 / branch fxdax-v1.16.1-pci at 68d597175) and 0f011561a (backlog). Series r2 rather than removal commits on fxdax-pci-r1: the MMIO shared memory commit was in the series itself, and the extent device would have been added MMIO-only, made PCI-capable, then made PCI-only. Verified: firecracker-series-check.sh all 14 commits both ways with clippy (Docker arm64, and CI's firecracker-series on x86-64); GCE x86-64 (fxdax-keep-n2-standard-4-us-east4-a-1): VMM KVM-free 29/29 and KVM 31/31 with fxdax, the default build's PCI tests 15/15 with test_config_space_as_upstream, the 11 PCI guest cases 11/11 (t273b), release round 16/16 (t273rel), debug round 15/16 (t273dbg; the one failure TASK-257's flaky a_removed_fill_gives_back_all_but_the_reservation, its known signature, noted there); guest configs resolved by olddefconfig: aarch64 PCI, PCI_HOST_GENERIC, PCI_ECAM, PCI_MSI, VIRTIO_PCI, ARM_GIC_V3_ITS, IRQ_MSI_LIB on, VIRTIO_MMIO off on both arches. Main CI 42556688045 green at 0f011561a, every job. scripts/upstream-changes.py --check passes. Removed test: vm.rs's the_transport_is_named_or_the_architectures, with the knob it tested.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every fxdax VM runs on virtio-PCI, x86-64 and aarch64: vm.rs always passes --enable-pci, FXDAX_VIRTIO_TRANSPORT gone, the guest kernel built without VIRTIO_MMIO; README and the integrator guide say Firecracker needs --enable-pci. Firecracker fxdax-v1.16.1-r2 (branch fxdax-v1.16.1-pci, 68d597175), replacing fxdax-pci-r1: the general PCI fixes with the others before the fxdax feature, no MMIO shared memory registers, the extent device PCI-only from its commit (window always a BAR in the 64-bit gap; no MMIO save/restore/overrides; MMIO attach refused with ExtentDevicesNeedPci); upstream's MMIO transport and the default build as v1.16.1's (seccomp, swagger, .cargo unchanged; test_config_space_as_upstream passes). Net: Firecracker -316/+100 against fxdax-pci-r1, fxdax -60/+32. Verified by the series check both ways on every commit, GCE KVM tests, the 11 PCI guest cases, release 16/16 and debug 15/16 rounds (TASK-257's flake), main CI 42556688045 green. aarch64 compiles and its config is wired for PCI but has not run (TASK-268, which lists what is unverified).
<!-- SECTION:FINAL_SUMMARY:END -->
