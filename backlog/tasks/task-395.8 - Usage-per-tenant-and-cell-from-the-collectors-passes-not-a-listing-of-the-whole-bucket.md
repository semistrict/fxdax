---
id: TASK-395.8
title: >-
  Usage per tenant and cell from the collector's passes, not a listing of the
  whole bucket
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
labels:
  - bucket
  - collector
  - tools
dependencies:
  - TASK-395.4
  - TASK-395.6
parent_task_id: TASK-395
priority: low
ordinal: 780000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-394's `fxdax-gc --usage` lists the whole bucket into memory: 30-40 GB and hours at 100M objects. Each cell pass lists the cell anyway, so it can record the cell's objects and bytes, and usage becomes reading those records.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each cell pass records its cell's objects and bytes (by kind) under the cell, with the time of its listing
- [ ] #2 `fxdax-gc --usage` reads the recorded usage of every cell (or of one tenant's), streaming one JSON object a line per tenant and per cell, in memory bounded independently of the bucket's size, each with when it was measured
- [ ] #3 A test shows usage after writes, compaction and deletion matching a listing of the cells
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
