---
id: TASK-160.3
title: 'Guest DAX: a gate ticket in place of four copies of the seals check'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 07:34'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 574000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The snapshot-seals, ask Rust, enter the gate, compare, leave-and-retry sequence is copied in map_found, new_slice, iomap_write and restage_work ('As in fxfs_dax_map_chunk'); new_slice leaves the gate conditionally in three places. Explains TASK-121, 133, 79.11, 44.21, 150.11. Proposal: struct fxfs_gate_ticket with fxfs_gate_ticket(), fxfs_gate_enter() (0, -EAGAIN after leaving, -EBUSY) and fxfs_gate_leave(). After TASK-153.5, which rewrites iomap_write's gate loop.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every user of the seals check takes a ticket
- [ ] #2 The task-121 and task-133 mutants still fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded by TASK-164 (read-only DAX, decided 2026-11-04): DAX maps only sealed extents and writes go through the guest page cache, so the write and store paths this restructured, the gate and the per-write state go. TASK-160.1's driver, written but never compiled, is archived as tag archive/task-160.1-retry-driver (dead-ends/one-retry-driver-for-writable-dax.md).
<!-- SECTION:FINAL_SUMMARY:END -->
