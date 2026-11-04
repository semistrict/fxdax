---
id: TASK-160.1
title: 'Guest DAX: one retry loop for every way into a file'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 07:34'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 572000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Error codes are the control channel between the iomap callbacks and the four ways in: fxfs_dax_read_faulting_between, fxfs_dax_write_iter, fxfs_dax_do_fault, fxfs_dax_zero_part (dax.c). Each reads -ESTALE, -EXDEV, -ENOMEM its own way: read and zeroing retry -EXDEV with no limit, only write(2) sets no_slices (generic/638's fix). Rust's -ESTALE (a follower failed, a device cut off) is read as 'range behind'; dax.c:2905 turns -EIO/-ENOMEM/-ENOSPC into -ESTALE. Explains TASK-150.20, 153.1, 153.3's -ESTALE, 150.13, generic/638, each fixed one way in at a time. Proposal: struct fxfs_dax_retry and fxfs_dax_retry_next() owning drops, making room, limits, fatal signals, no_slices and the waits; errors from Rust mapped at the call boundary so dax.c's own codes come only from dax.c. Keep faults never waiting inside an iomap (TASK-79.12); don't cap -EBUSY/-EDEADLK.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every way in goes through one retry driver
- [ ] #2 A guest/sim scenario of zeroing over a slice that keeps refusing is bounded, red before
- [ ] #3 A mutant removing the driver's limit fails all four ways in
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
