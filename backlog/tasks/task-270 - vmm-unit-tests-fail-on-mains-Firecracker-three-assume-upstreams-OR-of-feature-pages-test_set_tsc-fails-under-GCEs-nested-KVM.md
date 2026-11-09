---
id: TASK-270
title: >-
  vmm unit tests fail on main's Firecracker: three assume upstream's OR of
  feature pages, test_set_tsc fails under GCE's nested KVM
status: Done
assignee: []
created_date: '2026-11-08 23:35'
updated_date: '2026-11-09 01:35'
labels:
  - firecracker
  - bug
dependencies: []
priority: low
ordinal: 699000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The vmm crate's whole suite (cargo nextest run -p vmm --features fxdax, scripts/test.sh with FXDAX_FIRECRACKER_TESTS=all) on a GCE n2 host fails four tests at fxdax's Firecracker 06c161526 (main), the same at fxdax-pci 968783f04 (TASK-267 GCE kvm3/kvm4, 2026-11-08). test_ack_features_by_page (mmio.rs: left 8, right 24) and vsock's test_virtio_device and test_persist_uds_backend (acked features 38654705664 vs 39191576576) expect a features page written again to be ORed into what was acked; fxdax's ack_features_by_page replaces the page (TASK-164.1, 95391a748), and their expectations were never updated: nothing ran the whole suite. arch::x86_64::vcpu::tests::test_set_tsc expects set_tsc_khz to fail and it succeeds, likely as GCE's nested KVM scales TSCs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The three feature-page tests assert the replacing semantics fxdax chose, or the series keeps OR where the tests need it, decided in the series restructure
- [ ] #2 test_set_tsc passes or is gated on what it needs from the host
- [ ] #3 The vmm whole suite is green on a GCE host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-09: on fxdax-v1.16.1-r1 (TASK-269) and fxdax-pci-r1 cd07e39ae, the vmm suite on GCE (FXDAX_FIRECRACKER_TESTS=all, r kvmall) runs 772, 771 passing: test_ack_features_by_page and vsock's test_virtio_device and test_persist_uds_backend pass, the series' 8a3182e91 having made them assert the replacing semantics. test_set_tsc alone fails, which TASK-271 carries (AC 2, 3).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded: the opt-in series (TASK-269) fixed the three feature-page tests (verified passing on GCE in the vmm suite at cd07e39ae, 771/772); test_set_tsc, the one left, is TASK-271's.
<!-- SECTION:FINAL_SUMMARY:END -->
