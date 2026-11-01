---
id: TASK-153.3
title: 'An extent goes minted, open, sealed, and never back'
status: To Do
assignee: []
created_date: '2026-11-01 22:52'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 547000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Open is not one fact and can go backwards: the Rust open set, region->writable, the device seal and per-block sharing can disagree, and on the host ExtentStore.open and the store uncommitted are two owners too. Explains TASK-99, 140, 44.21, 91.1, 91.9, 150.11, 79.46, 121, 150.7. A write to an extent not known to be open retries rather than reopens, shared blocks never go through an open extent region, and DaxExtent says which of sealed, shared or not known open it means.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An extent can not read as open after sealed
- [ ] #2 The checks around open that this makes dead are gone
- [ ] #3 guest/sim passes, those past bugs put back failing or unbuildable
- [ ] #4 Suites and pgbench-dax pass on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
