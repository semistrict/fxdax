---
id: TASK-165.8
title: >-
  Uploaded data leaves the store: acknowledged segments join the part cache;
  local cleaning and the 3x bound go
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
labels: []
dependencies:
  - TASK-165.4
  - TASK-165.7
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 630000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 8. Two cleaners (Store::clean_behind and the collector) and a bound on local dead space (TASK-79.89) exist because the store keeps every segment after it is uploaded. Once acknowledged segments move into the part cache, the collector is the only cleaner and local disk is the WAL, the open segment and the cache. Lands in one batch with TASK-165.7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An acknowledged segment and its own extents move into the part cache (Cache::adopt) and their extents are read through the fetcher; nothing else of them stays in the store
- [ ] #2 Store::clean, clean_behind, cleaning_due, the files bound and their callers (host_store, extentd's cleaning behind commits) are gone; the extentd cleaning tests assert the collector compacting what a guest freed
- [ ] #3 Views read extents freed after their commit from the WAL or the bucket, and a view whose extent the collector reclaimed is cut off (a test)
- [ ] #4 Local disk stays within the lag bound, the open segment's room and the cache's capacity under a churning guest (a test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
