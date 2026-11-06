---
id: TASK-149.32
title: >-
  extent_remote's simulation: every acknowledged object exists, every upload,
  sync and durable commit woken
status: To Do
assignee: []
created_date: '2026-11-06 03:35'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Split from TASK-149.18, which the user took to guest/sim (2026-11-05): its invariants there are done. What is left is extent_remote's sim, as TASK-149.18 first said: it checks what its runs read back, so a loss shows only if a read reaches it, and a waiter never woken shows only as a hung run, if at all. Stating both as invariants, and generating own extents and branches of children, reaches what the audit traced by hand. The extent_store side needs a Shuttle or quiescence check of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 After each step, every object an acknowledged segment names exists, or is known lost by the rules
- [ ] #2 Runs include children of their own branch uploading inherited extents of 4 MiB or more
- [ ] #3 At quiescence no upload, sync or durable commit still waits
- [ ] #4 The collector and upload bugs, put back, each fail it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
