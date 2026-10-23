---
id: TASK-79.37
title: The physmap uses one host mapping per chunk and asserts when mmap fails
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 225000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With interleaved arenas, each chunk becomes its own VMA, so large configurations reach vm.max_map_count. The failed mmap is an assert, raised after the chunk has left free_chunks.

Where: crates/zircon/src/vm/physmap.rs:250, crates/zircon/src/vm/physmap.rs:136, crates/zircon/src/vm/physmap.rs:305, crates/zircon/src/vm/pmm.rs:327, crates/zircon/src/vm/pmm.rs:338.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-12, TASK-43.1. TASK-12 (Done) bounds VMAs of the uffd second stage for DAX windows, not the physmap's one-mmap-per-chunk. No task covers the physmap's VMA count or the assert on a failed mmap in assign_chunk.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
assign_chunk asserted on a failed mmap after the chunk had left the free chunks. With interleaved arenas, a large memory_bytes could reach vm.max_map_count. The physmap's map and take-back now return the host's error and leave the chunk table unchanged, allocations the host refuses fail with NO_MEMORY, and configure refuses memory past physmap::max_size with EINVAL. Arenas also stop asking a refusing host for spares and log the refusal once (8b630f7a, 993ef9d4, f681cae2). The map tests panicked before with 'map a chunk: Permission denied'. They and the Linux-only take-back and max_size tests pass on Linux (GCE), and the portable ones pass on macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
