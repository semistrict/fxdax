---
id: TASK-112.2
title: >-
  Bench: fxfs DAX against ext4 dax=always on virtio-pmem and ext4 on pmem, first
  and second read of a base and pgbench, bare metal and nested
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 02:30'
updated_date: '2026-10-28 03:35'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: medium
ordinal: 425000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team (TASK-112) estimates the gap to static DAX; they ask that we measure it in the same bench VMs (bench-fs-ext4-dax-pmem, bench-fs-ext4-pmem, f3188ebc), reporting bare metal and nested separately, as KVM first-touch and window-change TLB shootdowns differ a lot.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One bench run reports fxfs DAX, ext4-dax-pmem and ext4-pmem for a first read, a second read of the same files, and pgbench
- [ ] #2 Results recorded for GCE nested and for bare metal
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE n2-standard-8 nested (us-east4-a), bench VMs 2 vCPU 2 GiB, medians of 3. Base read (r201-112r, cases bench-fork-*): first / second read of /usr (464 MB) and pgbench -S tps: Fxfs DAX (MAP_AHEAD) 0.687 / 0.092 s, 27.2k; Fxfs without DAX 2.653 / 0.030 s, 21.5k; ext4 dax=always on virtio-pmem 2.051 / 0.081 s, 23.9k; ext4 on pmem 4.138 / 0.029 s, 23.1k. Mount: 0.25 / 2.0 / 3.78 / 3.73 s. pgbench read-write (r202-112p, bench-fs-* FXDAX_BENCH_GROUPS=pgbench, scale 10, 30 s): ext4 DAX pmem 1496 tps (init 4.38 s), ext4 pmem 1456 (2.68 s), Fxfs 1791 (1.76 s), Fxfs DAX 1364 (3.75 s). Bare metal not run: needs a metal host (GCE c3 *-metal).
<!-- SECTION:NOTES:END -->
