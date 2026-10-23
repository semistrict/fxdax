---
id: TASK-79.78
title: >-
  A fork whose acknowledgement fails is deleted locally though its PUT took
  effect: the collector keeps its orphan's inheritance for ever
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 09:17'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 336000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.72 (round 2a). When Remote::forked fails, ExtentStore::fork (crates/fxdax/src/extents.rs) deletes the child locally, even if the fork's segment PUT took effect (only the acknowledgement failed, or a dead writer's PUT lands later). The parent's next writer then gives the deleted child a branch record; the collector treats that orphan as live and keeps what it inherited for ever.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork whose PUT took effect is either kept or deleted remotely too, whatever failed after it
- [ ] #2 The collector frees what an orphaned fork inherited once nothing else holds it, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 2c: a failed fork's cleanup that stops partway leaves the fork's handle (deleting it finishes the deletion), so this path makes no orphan; unfork failing is tested too. AC #2 is not met as written: the collector cannot tell a branch no host holds from one held idle somewhere, so orphans made before the fix are not reclaimed (TASK-79.85). Child-ID ownership, which unfork and the leftover handle's deletion rely on, is TASK-79.82.
<!-- SECTION:NOTES:END -->
