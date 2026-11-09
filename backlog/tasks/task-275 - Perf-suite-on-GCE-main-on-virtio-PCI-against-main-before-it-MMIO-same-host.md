---
id: TASK-275
title: 'Perf suite on GCE: main on virtio-PCI against main before it (MMIO), same host'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 03:50'
labels:
  - perf
  - gce
dependencies: []
ordinal: 702000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-267 (1e8f3ff37) put every VM on virtio-PCI and TASK-273 (751ced7b5) removed fxdax's MMIO paths. virtio-pci-vs-mmio-2026-11-09.md measured the transports on one tree by FXDAX_VIRTIO_TRANSPORT; nothing has run the whole performance suite on main as it now is against main before the switch (557b49a70, MMIO, its own scripts and Firecracker fxdax-v1.16.1-r1), so a regression from the rest of the change (the Firecracker series, the kernel config, the window placement) would go unseen. The same-host A/B decides; docs/measurements history is context.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fs bench (scripts/bench-gce.sh, every bench-fs case, all groups, 3 iterations) run on one c3-standard-8 for 557b49a70 and current main, compared with scripts/bench-ab.py
- [ ] #2 perf gate (scripts/perf-gate-gce.sh single, density, gate) run for both trees on one n2-standard-4
- [ ] #3 boot and restore (extent-guest, fxfs-guest-snapshot-dax, 10 rounds) and the bench-fork cases run for both trees on one host
- [ ] #4 scripts/fork-time-gce.sh run for both trees on one host
- [ ] #5 docs/measurements/perf-suite-pci-vs-before-<date>.md: tables per suite (old, new, change, spread), history comparison, regressions, conclusions; fs-bench-history.md regenerated
- [ ] #6 each regression beyond noise rerun and filed as a backlog task
- [ ] #7 results pushed to main, CI green
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
