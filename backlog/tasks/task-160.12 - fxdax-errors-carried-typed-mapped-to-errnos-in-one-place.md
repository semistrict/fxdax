---
id: TASK-160.12
title: 'fxdax: errors carried typed, mapped to errnos in one place'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 583000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Failure::Other(String) turns errors into text; extent_remote::Error::Displaced becomes EIO though the docs treat it as losing the branch (EROFS); two downcast lists, any new error type quietly EIO. Explains 894b4c1a1, 0c7e7c8cc. Proposal: Failure::Remote(extent_remote::Error) and one errno mapping with a test per variant. See also the Fxfs errno table task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One errno mapping for the host, tested per variant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
