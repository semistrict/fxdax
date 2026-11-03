---
id: TASK-160.21
title: 'Fxfs: one FxfsError to errno table'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 592000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The table is written twice and has drifted: guest/rust/fxfs-linux/src/lib.rs and crates/fxdax/src/error.rs disagree (AlreadyBound EBUSY or EIO, OutOfRange ERANGE or EIO); the tables use the outermost FxfsError, FxfsError::matches the root cause; Inconsistent and IntegrityError become EIO though TASK-153.11 and 153.13 now raise them on purpose (ext4 and xfs say EUCLEAN and EBADMSG). Proposal: fxfs_posix::errno(), exhaustive, one classification rule; both VFSs call it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One table, exhaustive, tested per variant
- [ ] #2 The choice for corruption errors recorded in OPEN_QUESTIONS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
