---
id: TASK-151
title: closed_segments_files_close_least_lately_read_first fails about 1 run in 80
status: To Do
assignee: []
created_date: '2026-11-01 19:11'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 540000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_store test, 30 failures in 2400 at the base commit, 27 in 2400 with fix/host-audit, so it predates that work: 17 files opened where 8 were expected. Cause not found.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is found and fixed, or the test states what it can hold
- [ ] #2 It passes 2400 runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
