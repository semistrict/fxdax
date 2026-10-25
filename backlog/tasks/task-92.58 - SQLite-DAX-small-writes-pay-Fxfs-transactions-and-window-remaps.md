---
id: TASK-92.58
title: 'SQLite DAX: small writes pay Fxfs transactions and window remaps'
status: In Progress
assignee: []
created_date: '2026-10-24 21:20'
updated_date: '2026-10-25 00:34'
labels: []
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 378000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The SQLite benchmark shows that Fxfs with DAX is 2-5.7x slower than Fxfs without DAX on transactional writes, while its mmap scans are already at ext4-DAX parity (docs/measurements/gce-sqlite-2026-10-24/baseline). A guest-kernel and whole-host profile on GCE (docs/measurements/gce-fs-bench-2026-10-24T2105Z-07519990) separates two costs:

- WAL/NORMAL batch1 does almost no device I/O, but fxfs_dax_map_write covers 19.44% of all workload samples and Fxfs transaction commit 9.72%, about 80-85% of the pwrite path. Extending a file into an already-writable block must not need a full transaction (TASK-92.28).
- Unavoidable overwrites pay staging/evacuation remaps. In the VMM's extentd-client thread, UffdSecondStage::leave_out covers 27.9%, smp_call_function_many_cond 15.0%, install 10.5% and mprotect 6.1%. DELETE/FULL batch100 makes about 15 MAPs and 10 UNMAPs per transaction. Replace a stage mapping without a separate STAGE_UNMAP/STAGE_MAP pair and reduce evacuation requests (TASK-91.16); then bound commit's mapped-ahead copying (TASK-92.12).

The old 256-faults/MiB SQLite mmap investigation is stale: current DAX scans run at about 3.3 GiB/s with 5.6-7.6 faults/MiB. Do not weaken fsync barriers or compare WAL/NORMAL's commit rate with WAL/FULL's as though their durability were equal.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria

<!-- AC:BEGIN -->
- [ ] #1 TASK-92.28's extending-write fast path is implemented and improves SQLite WAL/NORMAL batch1 on GCE without changing recovery or durability semantics
- [ ] #2 The remaining post-seal MAP/UNMAP cost is reduced, with guest and VMM measurements on the SQLite DELETE/FULL and WAL/FULL workloads
- [x] #3 SQLite recovery, integrity and snapshot checks and the DAX guest suites pass
<!-- AC:END -->

## Definition of Done

<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Implement and A/B TASK-92.28 against the recorded SQLite profile.
2. Add stage-replacement attribution, then reduce replacement and evacuation costs on the current quarantined RemoteStage design.
3. Measure three iterations, verify recovery and the DAX suites, and record the result beside the baseline.
<!-- SECTION:PLAN:END -->

## Implementation Notes

2026-10-25: The first safe append-size implementation and regression coverage are complete locally. Same-host, reverse-order SQLite A/B shows repeatable 10–13% gains at batch100, while batch1 throughput is effectively flat. All ten iterations per kernel passed recovery/integrity/payload/snapshot checks; the five guest cases and macOS/Linux workspace suites passed. AC #1 remains open because meaningful WAL/NORMAL batch1 improvement is not established. Stage replacement and evacuation work (AC #2) remains unimplemented. See docs/measurements/sqlite-dax-appends-2026-10-24.md for raw runs, sample ranges, kernel identities, and validation.
