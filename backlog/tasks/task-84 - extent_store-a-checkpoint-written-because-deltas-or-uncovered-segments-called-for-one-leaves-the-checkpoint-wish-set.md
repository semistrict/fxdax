---
id: TASK-84
title: >-
  extent_store: a checkpoint written because deltas or uncovered segments called
  for one leaves the checkpoint wish set
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: low
type: bug
ordinal: 258000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while moving the index log into IndexLog (integrate-2): when a commit writes a checkpoint because the index holds enough deltas or the uncovered segments need one, a checkpoint wish set meanwhile stays set, so the next commit writes another, redundant checkpoint. Kept as it was in the refactor to keep behaviour; it costs a checkpoint's write per such commit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A checkpoint, whatever called for it, answers the wish; a test shows the next commit writes a delta
- [ ] #2 The extent_store suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
