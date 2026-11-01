---
id: TASK-149.8
title: >-
  Directories, exchange, tmpfiles and open unlinked files across cuts in the
  model test
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 509000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The model has one directory and no open files. Unlinked but open files and tmpfiles are graveyard work, the area of the trim bug that spun forever and of orphans. Exchange and renames across directories are multi-object transactions.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequences use subdirectories, exchange, tmpfiles and files unlinked while open
- [ ] #2 Cuts check each against the model
- [ ] #3 A hunt has run, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
