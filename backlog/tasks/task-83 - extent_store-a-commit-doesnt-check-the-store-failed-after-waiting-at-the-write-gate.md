---
id: TASK-83
title: >-
  extent_store: a commit doesn't check the store failed after waiting at the
  write gate
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: bug
ordinal: 257000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while restructuring commit_locked (integrate-2, extent_store's commit as phases): commit_locked waits at the gate for the writes in progress, and doesn't check `failed` again afterwards. A write or flush that fails the store meanwhile leaves the commit going on over a store whose state may differ from its files.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails the store while a commit waits at the gate, and the commit fails then
- [ ] #2 Fixed; the extent_store suites, its Shuttle suites included, pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
