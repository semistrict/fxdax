---
id: TASK-273
title: Remove fxdax's own virtio-MMIO paths once aarch64 runs on PCI
status: To Do
assignee: []
created_date: '2026-11-09 02:26'
labels: []
dependencies:
  - TASK-268
ordinal: 700000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
PCI is the default on x86-64 since TASK-267 (docs/measurements/virtio-pci-vs-mmio-2026-11-09.md: requests ~40% faster, boot and restore unchanged). Remove fxdax's MMIO-only code (window placement past the MMIO gap, MMIO extent save/restore, MMIO shared-memory registers, FXDAX_VIRTIO_TRANSPORT=mmio) once aarch64 runs on PCI and no snapshot taken over MMIO still needs restoring (a snapshot restores only on its own transport). Firecracker's own MMIO transport stays.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
