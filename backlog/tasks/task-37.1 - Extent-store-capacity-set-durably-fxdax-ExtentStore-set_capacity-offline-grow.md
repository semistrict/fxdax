---
id: TASK-37.1
title: >-
  Extent store: capacity set durably; fxdax ExtentStore::set_capacity; offline
  grow
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
updated_date: '2026-10-17 16:56'
labels: []
dependencies: []
parent_task_id: TASK-37
type: feature
ordinal: 106000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Lineage::set_capacity rewrites the branch's record (as resolved() does); Store's capacity becomes settable, checked by begin_write and info(); fxdax::ExtentStore::set_capacity. A shrink is refused below what the store holds plus reserves (NoSpace's sibling error). fxdax-branch resize DIR CAPACITY grows a store no daemon has open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Store::set_capacity grows, and shrinks no lower than the extents' bytes plus reservations, durably: reopened, the store has the new capacity
- [x] #2 fxdax-branch resize grows a closed store, and refuses to shrink one
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Store::set_capacity (durable, in the branch record via Lineage::set_capacity; shrink refused below held with TooSmall), ExtentStore::set_capacity, fxdax-branch resize (offline: grow only). Verified by extent_store capacity_resizes (reopen and fork keep it) and extentd resizes_closed_stores.
<!-- SECTION:FINAL_SUMMARY:END -->
