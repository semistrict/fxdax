---
id: TASK-92.52
title: >-
  The 'commits wait for object storage' warning does not say what the stall
  waits for
status: To Do
assignee: []
created_date: '2026-10-23 19:35'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: task
ordinal: 361000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.90 (r5/regress). extentd's warning that commits wait for object storage (crates/fxdax/src/remote.rs) says neither the bytes unreleased against the lag bound nor the commits waiting, so its logs cannot tell a lag-bound stall from a slow upload.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The warning names what it waits for: the bytes past the bound and the commits waiting
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
