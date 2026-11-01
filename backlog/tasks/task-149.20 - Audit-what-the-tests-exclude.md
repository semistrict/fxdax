---
id: TASK-149.20
title: Audit what the tests exclude
status: To Do
assignee: []
created_date: '2026-11-01 16:54'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 537000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
test_short_copy already covered a data loss bug but was switched off on DAX. Nobody reviews what is excluded, and exclusions outlive their reasons.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A list of every if !dax, skipped check, ignored test and xfstests known failure, each with why
- [ ] #2 What no longer has a reason runs again, its failures filed
- [ ] #3 A new exclusion must say why, checked by scripts/check-guest.sh or the like
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
