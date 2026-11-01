---
id: TASK-149.12
title: Differential testing against ext4 in a guest
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 513000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The same random operations on Fxfs and ext4, results compared, finds POSIX mistakes without an oracle written by hand, the class xfstests found in Fxfs one at a time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The same seeded operations run on both in a guest
- [ ] #2 Differences are reported, those allowed listed with a reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
