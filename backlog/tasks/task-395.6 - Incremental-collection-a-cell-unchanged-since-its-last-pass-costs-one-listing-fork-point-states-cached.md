---
id: TASK-395.6
title: >-
  Incremental collection: a cell unchanged since its last pass costs one
  listing, fork-point states cached
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.4
parent_task_id: TASK-395
priority: medium
ordinal: 778000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most cells are idle between sweeps: a stopped VM, a base image, a snapshot. With everything a cell's collection depends on under its prefix, a listing identical to the last pass's means the pass would decide the same, so the recoveries can be skipped. A parent's state at a fork point never changes, so it can be kept across passes (only what compaction moves of it changes, which is the parent cell's own doing).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A pass over a cell whose listing matches the one its last pass recorded makes no request beyond the listing and its lease, deleting what that pass deferred to a grace period once due
- [ ] #2 What a pass records to skip by (the listing's digest, deferred deletes, cached fork-point states) is kept under the cell's prefix, so that any collector process can use it, and is never itself a candidate
- [ ] #3 A test shows a sweep over many idle cells making one LIST per cell, and a write to any of them (a segment, a reference, a record) making the next pass recover it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
