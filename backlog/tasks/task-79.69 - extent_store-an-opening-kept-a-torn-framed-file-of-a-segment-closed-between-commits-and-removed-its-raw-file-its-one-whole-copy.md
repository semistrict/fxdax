---
id: TASK-79.69
title: >-
  extent_store: an opening kept a torn framed file of a segment closed between
  commits and removed its raw file, its one whole copy
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 320000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.33 (round 1). A segment closed between commits names in its footer commits that may be durable before its framed file is written and synced. A crash or failure between the write and the sync left a framed file whose trailer, table and footer were whole, but a data part torn or not durable. The branch's next opening kept it and removed the raw file, and reads failed their CRCs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An opening keeps a framed file only if its data is whole, and syncs it before the raw file goes; otherwise it frames the raw file anew
- [x] #2 A crash test with a torn data part opens and reads every durable commit's data
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A segment closed between commits could be left with a framed file whose trailer, table and footer were whole but whose data part was torn or not durable. The next opening kept that file and removed the raw file, the one whole copy, and reads then failed their CRCs. Now an opening keeps a framed file only if its data verifies too, and syncs it before the raw file goes; otherwise it frames the raw file anew. Two seeded tests, a torn data part with a crash and a failure followed by a reopen and a crash, failed before with a CRC error. Both now read every durable commit's data on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
