---
id: TASK-277
title: >-
  fxfs-guest-snapshot-dax hangs after a PCI restore: the restored guests never
  reach the host over vsock
status: To Do
assignee: []
created_date: '2026-11-09 05:02'
labels:
  - bug
  - pci
  - snapshot
dependencies: []
priority: high
ordinal: 703000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE TASK-275 boot-new run (fxdax-keep-c3-standard-8-us-east4-a-1, main 0f011561a, Firecracker fxdax-v1.16.1-r2 68d597175, release guest kernel): fxfs-guest-snapshot-dax passed round 1, then timed out in round 2 of 10 (120 s). Both restored VMs (a, b) load in ~25 ms and print `restored`, then nothing: no `thawing`. The guests idle (no D-state task, guest-agent in S, CPU 0 in safe_halt). The host test thread sits in accept4 on the vsock listener where each restored guest asks its name on a new connection (test_fxfs_guest_snapshot.rs: the restore resets the connection), so neither guest's connect reached the host. Firecracker logged only VcpuEvent::Resume with immediate_exit. The same host and case passed 10/10 on 557b49a70 (MMIO), and on PCI in TASK-267's boot-pci run (10/10). Suspect: the vsock device's queue notification or MSI-X interrupt lost across a PCI snapshot restore. Artifacts: docs/measurements/perf-suite-pci-vs-before-2026-11-08/boot-new/e2e/fxfs-guest-snapshot-dax/round-2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Root cause of the lost post-restore vsock connection found and fixed
- [ ] #2 fxfs-guest-snapshot-dax passes 30 rounds in a row over PCI on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
