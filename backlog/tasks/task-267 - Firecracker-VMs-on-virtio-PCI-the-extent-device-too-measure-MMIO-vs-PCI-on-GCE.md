---
id: TASK-267
title: >-
  Firecracker VMs on virtio-PCI, the extent device too; measure MMIO vs PCI on
  GCE
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 22:16'
updated_date: '2026-11-09 02:25'
labels:
  - perf
  - firecracker
dependencies: []
priority: medium
ordinal: 697000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax runs every Firecracker VM over virtio-MMIO. The extent device is MMIO-only (pci_mngr.rs: `unreachable!("extent-store devices are not attached over PCI")`): its DAX window is a virtio shared-memory region, which only fxdax's MMIO SHM_SEL registers expose, and the PCI transport has no shared-memory capability. MMIO costs exits per completion that MSI-X removes (TASK-91.15: interrupt status read and ack) and may weigh on the VMM request path (TASK-164.11.1). LangSmith asked that the fxdax series be opt-in behind the `fxdax` cargo feature with general fixes split out, so the generic PCI shared-memory capability goes in its own Firecracker commit. Whether MMIO support can then go is decided from GCE numbers, not here.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The extent device attaches over PCI: a 64-bit prefetchable BAR, naturally aligned and a power of two, from the mmio64 range, holds its DAX window, which a VIRTIO_PCI_CAP_SHARED_MEMORY_CFG (virtio_pci_cap64) capability describes; the guest driver finds it through virtio_get_shm_region
- [x] #2 Snapshot restore and fork restore put the BAR and the window back at the same guest address over PCI; MMIO keeps working
- [x] #3 Every Firecracker launch in the repo passes --enable-pci by default, with a knob to run on MMIO; aarch64 status checked and stated
- [x] #4 VMM unit tests cover the capability layout and the BAR; the guest suites pass on PCI on GCE
- [x] #5 MMIO vs PCI measured on one GCE instance, same kernel: boot time, fork time, per-request latency/fs bench, exits per request if cheap; recorded in docs/measurements/ with a recommendation on removing MMIO
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Firecracker, general: a restored PCI device that cannot activate fails the restore; the PCI transport describes a device's shared memory region as BAR 2 with a virtio_pci_cap64 SHARED_MEMORY_CFG capability (VirtioShmRegion and shm_regions un-gated; a device without regions upstream's to the register, test_config_space_as_upstream); device types past 63 under transitional ID 0x103f; PCI reset no longer self-deadlocks.
2. Firecracker, fxdax-gated: the DAX window placed per transport (window::Place), PCI save/restore of extent devices, snapshot-load overrides over PCI.
3. fxdax: vm.rs spawn passes --enable-pci by FXDAX_VIRTIO_TRANSPORT (pci default on x86-64), guests say their boot time, test.sh runs the PCI tests (KVM ones one at a time) through firecracker-cargo.sh; submodule to fxdax-pci-r1.
4. Verify: per-commit builds with/without fxdax; KVM tests and the 11 PCI guest cases on one held GCE host; release and debug guest rounds.
5. Measure MMIO vs PCI on one c3 host: bench (io, files, fsync), exits per request (perf stat), boot and restore (10 rounds); docs/measurements/virtio-pci-vs-mmio-2026-11-09.md.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Firecracker stack on fxdax-pci over 06c161526, kept apart for the series restructure: 8b7bc887d (pci restore propagates activation errors, generic), af92d4018 (virtio-pci shared memory BAR 2 + virtio_pci_cap64 SHARED_MEMORY_CFG caps, generic), 1f32698eb (extent window placed per transport, window::Place; PCI save/restore of extent devices; fxdax-gated). Each commit checks with and without fxdax on aarch64 (Docker). KVM-free unit tests pass (windows_are_placed_as_their_transport_needs, test_shm_bar); KVM ones (test_shared_memory_capabilities etc.) run on GCE via scripts/test.sh's firecracker part when /dev/kvm is writable. Launcher: vm.rs spawn passes --enable-pci by FXDAX_VIRTIO_TRANSPORT (pci default on x86-64; aarch64 stays mmio: untested, no arm64 KVM host).

2026-11-09: GCE found three real PCI bugs the KVM-free checks could not: (1) Linux's virtio_pci claims PCI device IDs 0x1000-0x107f only, so the extent device (type 0x4658, ID 0x1040+type) was never bound: types past 63 now take transitional ID 0x103f with the type as subsystem ID (8ac513fde); (2) PCI reset of a device implementing reset() self-deadlocked on the device lock and dropped the transport's MSI-X interrupt, hanging every guest's reboot (f283d43f6); (3) a snapshot load's extent overrides looked only in the MMIO state (folded into 87ddcbefa). Also: test_failed_reset_blocks_reinitialization relied on the dropped activation error (fixed in 8a8c8222e); only one virtio_pci_cap64 fits under Firecracker's 192-byte capability limit (MAX_SHM_REGIONS = 1); the persistence tests share a tap name, so test.sh runs KVM tests one at a time. Quick GCE loop (held n2, FXDAX_GUEST_TESTS=0 / FXDAX_E2E_TEST): KVM tests 30/30; vmm whole suite 767/771 (4 pre-existing, TASK-270, same at base 06c161526); extent-guest, fxfs-guest(-dax), snapshot(-dax, -volumes-dax), suspend(-dax, -read-only-volume), live-fork, follow-dax, daemon-lost pass on PCI.

2026-11-09: replayed onto fxdax-v1.16.1-r1 as fxdax-pci-r1 (cd07e39ae: ac91a8c69, f44b27580, d9db7b3ec, 5190c01f5 general; cd07e39ae fxdax-gated), each commit checked with and without --features fxdax through scripts/firecracker-cargo.sh, locked. test_config_space_as_upstream's golden read on GCE from the unmodified base (8e14cd916) and passing at f44b27580 and cd07e39ae. GCE: KVM tests 31/31 at the tip; vmm suite 771/772 (test_set_tsc, TASK-271); the 11 PCI guest cases green on 2f1eeb240 (same code, before the golden test); release round 15/16 (TASK-257's flake), debug round 16/16. Measurements in docs/measurements/virtio-pci-vs-mmio-2026-11-09.md.

The tip became 517d438c5 (fxdax-pci-r1, force-pushed over cd07e39ae before main named it): clippy's assertions_on_result_states in windows_are_placed_as_their_transport_needs, which scripts/firecracker-series-check.sh caught; every commit ok both ways in it (Docker), the extent and PCI KVM-free tests passing.

Landed: 1e8f3ff37 (code, submodule fxdax-pci-r1 517d438c5, docs/measurements/virtio-pci-vs-mmio-2026-11-09.md) and 86c6f78b9 (backlog); main CI 42549101699 green at 36dbbe2e9, test (firecracker) running the PCI tests and firecracker-series all 15 series commits both ways. Fork time: scripts/fork-time-gce.sh times a store's forks in the daemon, no VM, so the transport changes nothing it measures; a VM's fork, its snapshot restore, was measured (25.6 vs 26.1 ms). scripts/upstream-changes.py --check passes. Follow-up proposed, not filed: remove fxdax's MMIO paths (Place::PastMmio64, MMIO SHM registers, FXDAX_VIRTIO_TRANSPORT=mmio, MMIO extent save/restore) once TASK-268 runs aarch64 on PCI and no MMIO snapshot is kept.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
All of fxdax's Firecracker VMs run virtio-PCI on x86-64 (FXDAX_VIRTIO_TRANSPORT=mmio to go back; aarch64 stays MMIO, TASK-268). Firecracker fxdax-pci-r1 (517d438c5) on fxdax-v1.16.1-r1: four general commits (restore activation failure fails the restore; a device's shared memory region as BAR 2 with a virtio_pci_cap64 SHARED_MEMORY_CFG capability, devices without one upstream's to the register, test_config_space_as_upstream against v1.16.1's; types past 63 under transitional ID 0x103f, without which Linux never bound the extent device; reset without self-deadlock) and one fxdax-gated (the DAX window as that BAR, extent save/restore and snapshot overrides over PCI). Verified: every series commit both ways with clippy (firecracker-series-check.sh, CI); GCE KVM tests 31/31, vmm suite 771/772 (TASK-271), the 11 PCI guest cases, release 15/16 (TASK-257 flake) and debug 16/16 rounds; main CI 42549101699 green. Measured on one c3: extent requests 40% faster (write 121 to 73 µs), MMIO exits a third, cold reads and fsyncs up to 2.1x, boot and restore unchanged; recommendation in docs/measurements/virtio-pci-vs-mmio-2026-11-09.md: drop fxdax's MMIO paths after TASK-268 and once no MMIO snapshot is kept.
<!-- SECTION:FINAL_SUMMARY:END -->
