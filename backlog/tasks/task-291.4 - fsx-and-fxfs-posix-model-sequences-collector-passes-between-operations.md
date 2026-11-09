---
id: TASK-291.4
title: 'fsx and fxfs-posix model sequences: collector passes between operations'
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
labels: []
dependencies: []
parent_task_id: TASK-291
priority: low
ordinal: 695000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The model-checked sequences (fxfs-posix model_tests, guest fsx scripts) remount and compact Fxfs between operations but never collect the bucket under them, so regressions where collection loses data a filesystem still reads are caught only by the soak.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Model sequences can run a collector pass (grace 0, compact_below > 0) as one of their operations, and do by default in some seeds
- [ ] #2 The sequences pass with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
