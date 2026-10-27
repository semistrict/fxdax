---
id: TASK-79.7
title: >-
  A shared cache entry pages in through whichever Store cached it first: that
  store's FREE breaks the other VMs, and closed Stores are never released
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-27 13:51'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 195000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An FxExtent shared across forks reads through the first store's Store. When that fork FREEs an inherited extent, the entry goes and only its own window is zeroed. A sibling still mapping the extent then pages in through a store that answers NoExtent, and the sibling's VM aborts. Entries also hold an Arc<Store> and are never purged on close or delete, so closed Stores, their file descriptors and deleted forks' disk space stay held until the daemon exits.

Where: crates/fxdax/src/extents.rs:1110, crates/fxdax/src/extents.rs:1690, crates/fxdax/src/extents.rs:304, crates/fxdax/src/extents.rs:96, crates/fxdax/src/extents.rs:557, crates/fxdax/src/dax/uffd.rs:658.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-39.4, TASK-34, TASK-38, TASK-26.17. No task covers a shared FxExtent paging in through the first Store, breaking siblings after that store's FREE, or cache entries pinning closed Stores and their fds and disk. Related: delete a base while forks live (TASK-39.4), bounded files per store (TASK-34), FxExtent page-ins (TASK-38).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Cache entries keep the stores that asked for them (Holders), weakly: page-ins read through the first open one that holds the extent, or keeps it for a view; closed stores are not kept alive.
2. FREE drops the entry only if no other open holder holds the extent, and maps zeros in its own window either way.
3. Idle entries no open store holds go first when the cache trims.
4. Tests: forks_page_in_what_a_sibling_freed (EPIPE before), the_cache_keeps_no_store_closed (held before).
<!-- SECTION:PLAN:END -->
