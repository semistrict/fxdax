---
id: TASK-79.78
title: >-
  A fork whose acknowledgement fails is deleted locally though its PUT took
  effect: the collector keeps its orphan's inheritance for ever
status: To Do
assignee: []
created_date: '2026-10-23 06:12'
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
