---
id: TASK-111.8
title: >-
  Guest builds ship System.map and an unstripped vmlinux, and a nokaslr
  profiling mode or the KASLR offset in the boot log
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: low
ordinal: 422000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team samples guest instruction pointers on VM exits and could not name them after the fact because of KASLR (TASK-111). Related: TASK-64 (guest kernel bundle).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each guest kernel build produces System.map and an unstripped vmlinux next to the image
- [ ] #2 A profiling mode boots the guest with nokaslr, or the boot log prints the KASLR offset
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
