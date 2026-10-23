---
id: TASK-79.84
title: An OPEN with `from` opens a leftover fork handle instead of forking
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 343000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2c (TASK-79.78). Since TASK-79.78 a failed fork whose deletion stops partway leaves the fork's handle, which DeleteStore finishes deleting. An OPEN with `from` (extentd/server.rs, the open path) finds that handle and opens it instead of forking, so the guest gets the stale half-made fork. In TASK-42's area of extentd.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An OPEN with `from` whose target is a leftover handle of a failed fork forks anew or fails, never opens the leftover
- [ ] #2 A test covers it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
