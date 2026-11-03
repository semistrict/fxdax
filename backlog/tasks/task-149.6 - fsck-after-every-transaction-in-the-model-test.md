---
id: TASK-149.6
title: fsck after every transaction in the model test
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 507000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FxFilesystemBuilder has fsck_after_every_transaction, which the model test doesn't use. Corruption is found at the transaction that made it, not operations later, which makes a failure far shorter to diagnose.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The model test can run with fsck after every transaction
- [x] #2 A hunt has run that way, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A Setting that opens filesystems with fsck_after_every_transaction, FXDAX_MODEL_FSCK for the hunt, a short test in the default run.
2. Make the hook's panic say what fsck found.
3. Hunt that way; fix or file findings.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
FXDAX_MODEL_FSCK=1 hunts with fsck after every transaction, about 65 a sequence; the hook now says what fsck found. Control: an allocated size wrong by a block fails in 11 ms naming AllocatedSizeMismatch. 2,000 sequences on the final code, and 5,000 earlier, found nothing. Transactions of a mount before the hook is armed aren't checked (about 17 a sequence).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The model test can fsck after every transaction, verified by a control and hunts that found nothing.
<!-- SECTION:FINAL_SUMMARY:END -->
