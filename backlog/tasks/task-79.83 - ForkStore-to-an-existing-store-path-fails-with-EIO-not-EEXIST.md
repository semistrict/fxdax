---
id: TASK-79.83
title: 'ForkStore to an existing store path fails with EIO, not EEXIST'
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 342000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2c (TASK-79.78). A ForkStore retried to an existing `to` gets EIO (INTERNAL): fork_host's create_dir io::Error carries no Errno, and extentd/stores.rs's errno_of maps it to EIO, while CreateStore and ImportImage check path.exists() and give EEXIST.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ForkStore to an existing path fails with EEXIST, as CreateStore and ImportImage do, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
