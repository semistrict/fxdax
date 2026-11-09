---
id: TASK-268
title: 'aarch64 guests over virtio-PCI: verify once an arm64 KVM host exists'
status: To Do
assignee: []
created_date: '2026-11-08 22:31'
updated_date: '2026-11-09 03:10'
labels:
  - firecracker
dependencies: []
priority: low
ordinal: 698000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-273 put every fxdax VM on virtio-PCI, aarch64 too, and removed fxdax's MMIO paths, without an aarch64 guest having run over PCI: fxdax has no arm64 KVM host (GCE nests KVM on Intel only; CI is x86-64). What TASK-273 could check without one: aarch64 compiles, and the aarch64 guest kernel config has what Firecracker's aarch64 PCI path needs. This task runs the aarch64 guest suites on PCI once an arm64 KVM host exists, and fixes what they find. The specific runtime unknowns are in TASK-273's notes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An aarch64 guest suite (extent-guest, fxfs-guest(-dax), fxfs-guest-checks, the snapshot cases) passes on PCI on an arm64 KVM host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-09, TASK-273 put aarch64 on PCI without running it. Checked without an arm64 KVM host: the Firecracker series (fxdax-v1.16.1-r2) builds on aarch64 both ways, every commit, clippy -D warnings (Docker arm64); the aarch64 guest config, resolved by olddefconfig on the kernel release, has PCI, PCI_HOST_GENERIC (pci-host-ecam-generic), PCI_ECAM, PCI_MSI, VIRTIO_PCI, ARM_GIC_V3_ITS and IRQ_MSI_LIB, and VIRTIO_MMIO off (fxdax.config); Firecracker's FDT has the pci node's ranges cover the whole 64-bit gap (256 GiB at 256 GiB, where the extent device's window BAR is allocated), msi-map to the ITS (msic), linux,pci-probe-only; the ITS is created with GICv3.

Unverified at runtime, for this task to run:
- No fxdax aarch64 guest has ever booted, on either transport: the aarch64 guest kernel (with fs/fxfs's Rust) has not been built for this config.
- Linux binding the extent device (transitional ID 0x103f, subsystem 0x4658) through pci-host-ecam-generic with probe-only, and claiming BAR 2, a 64-bit prefetchable BAR, in the bridge's non-prefetchable 64-bit range (pci_find_parent_resource accepts it on x86's ACPI path; DT's is the same code, unrun).
- MSI-X through the ITS for the extent device's four queues and config vector; interrupts after a snapshot restore (the ITS's saved state, Firecracker's).
- The window's KVM memslot at 256-512 GiB fits the VM's IPA size, and userfaultfd faults on it; no KVM_PRE_FAULT_MEMORY there (x86-64 only), so first touches cost an exit each.
- Snapshot and fork restore over PCI on aarch64: the BAR and window back at the same address.
- TASK-268.1: the FDT's 64-bit range size drops its low 32 bits, harmless at 256 GiB.
<!-- SECTION:NOTES:END -->
