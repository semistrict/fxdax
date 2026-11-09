---
id: TASK-290
title: >-
  fs bench stat-warm: on virtio-PCI twice as many runs in the slow mode (~0.75M
  against ~1.2M stats/s)
status: To Do
assignee: []
created_date: '2026-11-09 08:26'
labels:
  - perf
  - bench
  - pci
dependencies: []
priority: low
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-275, one c3-standard-8 (fxdax-keep-c3-standard-8-us-east4-a-1), 557b49a70 (MMIO) against main 0f011561a (PCI): stat-warm is bimodal on both, ~0.7-0.85M or ~1.1-1.4M stats/s a run, but main lands in the slow mode in 20 of 47 runs against 10 of 47 (all seven bench-fs cases, two rounds and a rerun), ext4-guest-kernel 6 of 11 against 0 of 11 (gce-fs-bench-pci-ab-*-2026-11-08, *-lookup-2026-11-09). stat-warm (test_fs_bench.rs) times one pass of fs::metadata over the metadata group's files right after stat-cold, ~1.7 ms in all, so one interrupt or preemption halves it: a late device completion or MSI-X interrupt landing in it over PCI may be what differs, or a real per-stat cost. Unknown which.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cause of the slow mode found (guest perf or a longer stat-warm, several passes, on GCE over PCI)
- [ ] #2 stat-warm made long enough to measure stats rather than one interruption, or the PCI cost fixed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
