---
id: TASK-79.75
title: >-
  A write whose DIRTY request fails for want of memory is retried for ever when
  nothing in memory can be evicted
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 06:48'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 333000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.70 (round 2a). With memory full of pages nothing can evict (anonymous pages), a page-cache write's DIRTY request that fails with NO_MEMORY is resolved by the kernel and asked again by the writer, for ever, instead of failing the write with ENOMEM. Upstream Zircon would OOM instead; fxdax's OomBehavior::Evict has no such end.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A write into a VMO when memory is full of unevictable pages fails with ENOMEM (or the writer's errno for it) rather than retrying
- [ ] #2 A test fills memory with anonymous pages and shows the write fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
