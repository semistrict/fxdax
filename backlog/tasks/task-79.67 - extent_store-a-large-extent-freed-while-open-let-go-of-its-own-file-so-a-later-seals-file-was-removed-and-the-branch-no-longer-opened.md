---
id: TASK-79.67
title: >-
  extent_store: a large extent freed while open let go of its own file, so a
  later seal's file was removed and the branch no longer opened
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 318000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.1 (round 1). A free of an open extent written to a raw file of its own released its own file to the lineage, though only a seal makes one. Freed, written anew and sealed by the next commit, the extent's new own file, claimed by the seal, was then let go of by that commit and removed once durable, and the branch no longer opened.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A free of an open large extent lets go of its pieces in segments only; its raw file goes with the commit
- [ ] #2 Freeing, rewriting and sealing a large extent in one window leaves a branch that opens and reads the new data, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
