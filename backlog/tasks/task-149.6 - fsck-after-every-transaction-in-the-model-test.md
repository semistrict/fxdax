---
id: TASK-149.6
title: fsck after every transaction in the model test
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 08:27'
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
- [ ] #1 The model test can run with fsck after every transaction
- [ ] #2 A hunt has run that way, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A Setting that opens filesystems with fsck_after_every_transaction, FXDAX_MODEL_FSCK for the hunt, a short test in the default run.
2. Make the hook's panic say what fsck found.
3. Hunt that way; fix or file findings.
<!-- SECTION:PLAN:END -->
