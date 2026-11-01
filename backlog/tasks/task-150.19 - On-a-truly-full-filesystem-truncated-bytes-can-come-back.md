---
id: TASK-150.19
title: 'On a truly full filesystem, truncated bytes can come back'
status: To Do
assignee: []
created_date: '2026-11-01 21:43'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 542000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-149.8's hunts, confirmed by the model test: zero_tail gets NoSpace on a cut because the cut's frees wait for a sync; a sync and retry, now in setattr, narrows it, but with no block free even after a sync the bytes past the new end stay, and a later grow exposes them. Failed 2 of 2,880 200-op sequences and 0 of 16,800 at 48 ops, so the default model test could very rarely go red. Not pinned, as a red test can't land; the replay sequence is in TASK-149.8's notes. Two ways out, a decision for the user: every non-DAX growth through one path that zeroes the tail in its own transaction, dropping the eager zero, which is the proper fix (TASK-80's reserved writeback would pay for that block, and it touches Fxfs, the page cache and writeback accounting); or a mid-block shrink returns NoSpace when no block exists, reversing TASK-60 for that case. Same owner of size question as TASK-150.15 and TASK-133, and explains generic/130 and TASK-79.53.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The user has chosen how to close it
- [ ] #2 A test fails on the code as it stands, pinned once it can land
- [ ] #3 Fixed, the test red again with the fix taken out
- [ ] #4 The notes name the blind spot and what a search around it found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
