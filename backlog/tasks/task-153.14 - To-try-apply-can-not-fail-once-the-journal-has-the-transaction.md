---
id: TASK-153.14
title: 'To try: apply can not fail once the journal has the transaction'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 23:09'
updated_date: '2026-11-03 12:30'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 561000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: a live transaction that cannot apply can only stop the journal, so a panic becomes a dead filesystem (journal.rs:2211).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decided, with what a failed apply should do
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read what fails at live apply (journal.rs write_and_apply_mutations, object_manager.apply_transaction) and what a failure leaves. 2. Weigh: stop the journal (now), check-then-apply under the writer's lock, undo a partial apply. 3. Decide, record it in OPEN_QUESTIONS.md; code and a test only if the decision changes behavior.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Decision (2026-11-03): a failed live apply goes on stopping the journal

What happens now (5450852fe): the records are in the writer's buffer, without a Commit; apply_transaction applies mutations in order and fails on one (a corrupt image's allocator freeing what it doesn't hold, or a bug), leaving those before it applied in memory. The journal stops: that commit fails Inconsistent, later ones JournalFlushError, reads go on, and a remount replays without the transaction (test_transaction_that_cannot_be_applied_stops_the_journal).

Decided to keep it: it is ext4's errors=remount-ro. A failed apply means memory contradicts itself; writing on is how that spreads to disk. The alternatives:
- Check-then-apply under the writer's lock, failing only the transaction: every fallible apply needs a dry-run twin evaluated against state the transaction's own earlier mutations change (an allocation and a free of it in one transaction), a second apply to keep in step with the first.
- Undo a partial apply: an inverse for every mutation kind, in every store and the allocator.
Both buy going on writing on metadata already known wrong. No code change, so no test beyond the existing one.
Structure: the root cause is that apply can fail at all, which only a trust boundary at read (TASK-153.12) removes for corrupt images: a check of each record as it is read means memory never holds what apply would refuse.

Verified: test_transaction_that_cannot_be_applied_stops_the_journal passes on macOS and GCE (in the suites above, TASK-153.13); no code changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Decided: a live transaction that fails to apply goes on stopping the journal, ext4's errors=remount-ro: writes fail, reads go on, a remount replays without it. A failed apply means memory contradicts itself (a corrupt image or a bug), and the alternatives, a dry-run twin of every apply or an undo of each mutation kind, buy writing on over metadata known wrong. The real fix for corrupt images is checking records as they're read (TASK-153.12). Recorded in OPEN_QUESTIONS.md; no code change.
<!-- SECTION:FINAL_SUMMARY:END -->
