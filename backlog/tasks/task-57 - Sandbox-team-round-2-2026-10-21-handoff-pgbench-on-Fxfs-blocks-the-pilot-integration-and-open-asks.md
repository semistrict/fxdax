---
id: TASK-57
title: >-
  Sandbox team, round 2 (2026-10-21 handoff): pgbench on Fxfs blocks the pilot;
  integration and open asks
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-21 19:55'
labels: []
dependencies: []
priority: high
ordinal: 166000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team (LangSmith) booted a sandbox guest on an fxdax store under their jailer and real seccomp filter (fxdax 0a3015b, Firecracker v1.16.1 + fxdax's 20 extent-device commits through 0a6f1e33 on their firecracker-internal tag, Amazon Linux kernel6.18-6.18.54-121.166 + fxdax's series + their smithbox.config). pgbench -i -s 50 on a forked store (2 vCPUs, 4 GiB, 2 GiB DAX window, 16 GiB capacity, base imported from postgres:17, extentd 8 GiB, branches in S3 on RustFS with 25 ms netem delay, nested in a Firecracker microVM on GCE) failed twice: a guest kernel panic (allocator.rs:277 Insufficient reservation space, in truncate), then a short read in a parallel index build. The same load takes 119 s on their JuiceFS ext4 rootfs. Reproduction steps are in the subtasks. This blocks any pilot; integration asks block production.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Its tasks: blockers TASK-58 (reservation panic in truncate), TASK-59 (short read in parallel index build), TASK-60 (no kernel panic on exhaustion), TASK-61 (40 s COMMITs, 518 ms WRITEs), TASK-62 (the workload in the guest suites); production blockers TASK-63 (Firecracker series outside fxdax), TASK-64 (guest kernel bundle); TASK-65 (HTTPS submodules); open asks TASK-66 to TASK-78 (durability contract, extentd-kill soak, hypothesis (d), jailer runs of every suite, VMM exit statuses, fxdaxctl grant, per-store memory, ListStores paging, bulk bytes export, zygote snapshots and live migration, lineage depth bound, guest-side fuzzing, real Azure Blob).
<!-- SECTION:NOTES:END -->
