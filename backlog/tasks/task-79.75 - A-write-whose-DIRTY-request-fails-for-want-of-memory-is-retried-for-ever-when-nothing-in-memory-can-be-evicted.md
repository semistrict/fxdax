---
id: TASK-79.75
title: >-
  A write whose DIRTY request fails for want of memory is retried for ever when
  nothing in memory can be evicted
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A write into a VMO when memory is full of unevictable pages fails with ENOMEM (or the writer's errno for it) rather than retrying
- [x] #2 A test fills memory with anonymous pages and shows the write fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A write whose page cannot be dirtied for want of memory now fails with NO_MEMORY, which fxdax gives as ENOMEM, rather than asking for the page again for ever. Its DIRTY requests are failed rather than resolved spuriously, including when the response's wait for relief fails. With memory full of anonymous pages, a file write hung before and now fails with ENOMEM in 0.03 s. The fxdax test and both zircon-core-tests memory_out tests pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
