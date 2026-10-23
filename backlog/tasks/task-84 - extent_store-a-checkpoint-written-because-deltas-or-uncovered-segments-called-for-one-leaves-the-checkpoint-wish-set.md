---
id: TASK-84
title: >-
  extent_store: a checkpoint written because deltas or uncovered segments called
  for one leaves the checkpoint wish set
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A checkpoint, whatever called for it, answers the wish; a test shows the next commit writes a delta
- [x] #2 The extent_store suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
IndexLog::append took the branch's wish for a checkpoint only when neither the deltas nor the uncovered segments called for one first. A checkpoint written for those reasons left the wish set, and the next commit wrote another, redundant checkpoint. The wish is now taken whatever calls for the checkpoint (b4acf948). a_checkpoint_due_answers_a_wish_for_one shows that after a checkpoint the deltas called for, with a wish set, the next commit writes a delta. Before the fix it wrote a second checkpoint. The test and the extent_store suites pass on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
