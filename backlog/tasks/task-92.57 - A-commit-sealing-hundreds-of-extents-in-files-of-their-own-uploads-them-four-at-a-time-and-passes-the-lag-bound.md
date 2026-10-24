---
id: TASK-92.57
title: >-
  A commit sealing hundreds of extents in files of their own uploads them four
  at a time and passes the lag bound
status: To Do
assignee: []
created_date: '2026-10-24 07:32'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 377000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found measuring TASK-91.17 with PostgreSQL's fsync off. A commit that seals hundreds of 1 MiB extents into their own files makes as many objects, which the writer uploads OWN_AT_ONCE=4 at a time (extent_remote writer.rs), so the upload lag passes max_lag_bytes (1,449-1,543 MB in run 1) and commits wait 0.48-1.19 s each. Related to TASK-92.41 (commits spanning segments would seal small extents in place).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Such a commit's own extents upload within the host's upload budget at a rate that keeps the lag under its bound, measured on GCE with fxfs-guest-pgbench-nofsync
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
