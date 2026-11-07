---
id: TASK-149.20.3
title: >-
  The page-cache checks don't run on DAX mounts, whose new files are in the page
  cache
status: To Do
assignee: []
created_date: '2026-11-07 13:36'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: high
type: task
ordinal: 668000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. fxfs-guest-checks' large_folios, scattered_writeback, partial_writeback and writeback_out_of_memory run in the page-cache suites alone (Suites::PageCache, guest/tests/test_cases/src/test_fxfs_guest_checks/mod.rs: 'Files on DAX are in no page cache'). Since copy-on-write DAX (doc-13, TASK-164) a DAX mount's new files are in the page cache till read in again (cow_created_in_page_cache, TASK-44.15's notes), so those writeback paths run on DAX mounts too, unchecked there: as test_short_copy, which covered a data loss bug, was off on DAX (TASK-149.20's description).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The four checks run in the DAX suites too (Suites::All, or a variant whose files are made on the DAX mount), on GCE
- [ ] #2 What they find on DAX is fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
