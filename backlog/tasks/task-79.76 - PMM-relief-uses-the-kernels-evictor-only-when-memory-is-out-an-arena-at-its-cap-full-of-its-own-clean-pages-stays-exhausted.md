---
id: TASK-79.76
title: >-
  PMM relief uses the kernel's evictor only when memory is out: an arena at its
  cap full of its own clean pages stays exhausted
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 06:48'
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
- [ ] #1 An arena at its cap whose pages are clean and evictable gets room by eviction when the relief evictor frees nothing
- [ ] #2 A zircon test shows it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
