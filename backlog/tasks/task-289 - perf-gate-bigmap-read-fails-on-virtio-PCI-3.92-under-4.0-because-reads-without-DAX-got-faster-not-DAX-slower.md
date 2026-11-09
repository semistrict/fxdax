---
id: TASK-289
title: >-
  perf gate: bigmap-read fails on virtio-PCI (3.92 under 4.0) because reads
  without DAX got faster, not DAX slower
status: To Do
assignee: []
created_date: '2026-11-09 08:26'
labels:
  - perf
  - gate
  - pci
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-275 ran scripts/perf-gate-gce.sh on one n2-standard-4 (fxdax-keep-n2-standard-4-us-east1-b-1) for 557b49a70 (MMIO) and main 0f011561a (PCI). Main fails one gate, bigmap-read (bigmap-dax cold reads over bigmap-no-dax): 3.919 against at least 4.0, where MMIO gave 4.920 (and 5.0-5.6 in restructure-before-after-2026-11-07.md). Both sides got faster over PCI: without DAX cold reads 16.3 to 22.0 MiB/s (+35%, each page fault one device read, which PCI serves faster), with DAX 80.1 to 86.2 MiB/s (+8%). The gate measures DAX against a baseline that the transport moved; as it stands, main's perf gate is red on a speedup. Results: docs/measurements/perf-suite-pci-vs-before-2026-11-08/gate-{old,new}.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 bigmap-read gate rethought for PCI (threshold re-measured on several PCI runs, or the gate made absolute) and docs/measurements/dax-cow-gate.md updated
- [ ] #2 scripts/perf-gate-gce.sh passes on main on GCE n2-standard-4
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
