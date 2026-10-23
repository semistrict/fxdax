---
id: TASK-79.76
title: >-
  PMM relief uses the kernel's evictor only when memory is out: an arena at its
  cap full of its own clean pages stays exhausted
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 334000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.70 (round 2a). crates/zircon/src/vm/pmm/relief.rs asks the kernel's evictor only for Starved::Total. An arena at its own cap that is full of its own clean pager-backed pages, with a relief evictor that frees nothing of it, is exhausted: allocations fail NO_MEMORY although the pages could be evicted (a probe gives NO_MEMORY before and after TASK-79.70). extentd does not reach it today: tenant arenas' pager-backed pages are cached extents, which its evictor frees, and imports run in the default arena.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An arena at its cap whose pages are clean and evictable gets room by eviction when the relief evictor frees nothing
- [x] #2 A zircon test shows it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An arena at its own cap, full of its own clean pager-backed pages, now gets room by eviction of those pages on demand when the relief evictor frees nothing; before, it stayed exhausted and failed NO_MEMORY. The same happens without relief, and other arenas' pages are not evicted for it. Review follow-ups made the eviction take the oldest first, each chosen page alone (DONT_NEED first), and visit only chunks that hold reclaimable pages. They also bounded the eviction lock's holds and skipped asks made stale, each pinned by a zircon test and measured with the arena and page-queue benches on macOS. The arena_unittest and Shuttle tests pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
