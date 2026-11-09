---
id: TASK-275
title: 'Perf suite on GCE: main on virtio-PCI against main before it (MMIO), same host'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 03:50'
updated_date: '2026-11-09 12:14'
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
- [x] #1 fs bench (scripts/bench-gce.sh, every bench-fs case, all groups, 3 iterations) run on one c3-standard-8 for 557b49a70 and current main, compared with scripts/bench-ab.py
- [x] #2 perf gate (scripts/perf-gate-gce.sh single, density, gate) run for both trees on one n2-standard-4
- [x] #3 boot and restore (extent-guest, fxfs-guest-snapshot-dax, 10 rounds) and the bench-fork cases run for both trees on one host
- [x] #4 scripts/fork-time-gce.sh run for both trees on one host
- [x] #5 docs/measurements/perf-suite-pci-vs-before-<date>.md: tables per suite (old, new, change, spread), history comparison, regressions, conclusions; fs-bench-history.md regenerated
- [x] #6 each regression beyond noise rerun and filed as a backlog task
- [x] #7 results pushed to main, CI green
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Hold one c3-standard-8 (fs bench, boot/restore, Linux checkout) and one n2-standard-4 (perf gate, fork bench cases); fork-time-gce.sh on its own n2. 2. Old side: worktree at 557b49a70 with its Firecracker (r1), plus 1e8f3ff37's boot console line only. New: main 0f011561a. 3. Run each suite old then new on one host; fs bench two rounds ABAB. 4. Pool per case, compare medians and ranges, rerun anything slower beyond its ranges. 5. Curate results into docs/measurements, regenerate fs-bench-history.md, write perf-suite-pci-vs-before-2026-11-08.md, file each problem.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Hosts: fxdax-keep-c3-standard-8-us-east4-a-1, fxdax-keep-n2-standard-4-us-east1-b-1 (both held by create, stopped after), fxdax-keep-n2-standard-4-us-west1-a-1 (fork-time, all). Main's snapshot rounds hung on 0f011561a (TASK-277); restore times taken on 56cdb1ecd with its fix (Firecracker r3). Old side's first fork-bench run lost five cases to lua.org timing out (TASK-297), rerun. My first fork-bench runs skipped every case: FXDAX_BENCH_ITERATIONS was unset (the cases need it). ext4 lookup-missing 0.96x reran 0.97x overlapping: noise. Filed TASK-289 (bigmap-read gate 3.92 < 4.0 on PCI, a speedup of reads without DAX), TASK-290 (stat-warm slow mode 20/47 runs vs 10/47). Raw results (118 GB: images, buckets, memory) kept outside the repo, curated 13 MB committed.

AC #1: compared per case with the same pooling as scripts/bench-ab.py (medians, ranges, overlap), not bench-ab.py itself: it pools every case under one workload name. Table: docs/measurements/perf-suite-pci-vs-before-2026-11-08/fs-bench-ab.md. DoD: measurements and docs only, no code changed; scripts/upstream-changes.py --check passes; workspace tests not rerun (nothing they cover changed).

Pushed 8324d5061 (results f9d17a477, write-up 03f308deb, tasks). Its own CI run and two after it were cancelled by newer pushes; CI 42600287867 at df22d4e59, which contains it, green (42604132639 at bfb5c12e4 green too). Hosts stopped: fxdax-keep-c3-standard-8-us-east4-a-1, fxdax-keep-n2-standard-4-us-east1-b-1, fxdax-keep-n2-standard-4-us-west1-a-1 (the c3 and us-west1-a were started again by other runs afterwards). Raw results, 118 GB, kept outside the repo pending the user's decision.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Ran the performance suite on GCE for virtio-PCI main (0f011561a; restores at 56cdb1ecd, after TASK-277's fix) against main before the switch (557b49a70, MMIO), each pair on one host: fs bench (7 cases, two rounds and a rerun, c3-standard-8), boot and restore, Linux checkout, perf gate and fork bench cases (n2-standard-4), fork-time. PCI is as fast or faster everywhere: cold reads 1.2-2.0x, fsyncs 1.1-1.3x, pgbench 1.08-1.20x, a Linux checkout 4-19%; boot, restore and forks unchanged; its gains are the transport's alone (virtio-pci-vs-mmio-2026-11-09). No regression held up on a rerun. Filed TASK-277 (restore hang, fixed), TASK-289 (bigmap-read gate fails on a speedup), TASK-290 (stat-warm slow mode), TASK-297 (lua.org). Write-up docs/measurements/perf-suite-pci-vs-before-2026-11-08.md, results curated beside it and in gce-fs-bench-pci-ab-*, fs-bench-history.md regenerated; CI 42600287867 green.
<!-- SECTION:FINAL_SUMMARY:END -->
