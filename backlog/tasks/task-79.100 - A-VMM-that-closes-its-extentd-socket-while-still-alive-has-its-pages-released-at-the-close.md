---
id: TASK-79.100
title: >-
  A VMM that closes its extentd socket while still alive has its pages released
  at the close
status: To Do
assignee: []
created_date: '2026-10-24 03:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 371000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.9 (round 2b). extentd releases a VM process's quarantined and parked pages when its connection closes, but a VMM that closes its socket while still running may still have them mapped for its guest, so they could be reused while the guest reaches them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Pages a VM process may still map are kept until that process is known to have exited
- [ ] #2 A test closes the socket of a live VM process and checks its pages are not reused
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
