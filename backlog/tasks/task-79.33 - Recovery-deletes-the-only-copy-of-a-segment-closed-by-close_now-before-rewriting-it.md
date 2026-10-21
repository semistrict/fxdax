---
id: TASK-79.33
title: >-
  Recovery deletes the only copy of a segment closed by close_now before
  rewriting it
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 221000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
close_now frames with an epoch that may be only ordered and unlinks the raw file. After a crash, close_left_open removes the framed file before rewriting it, so a second crash during the rewrite loses durable data and the branch no longer opens.

Where: crates/extent_store/src/store.rs:2247, crates/extent_store/src/store.rs:2263, crates/extent_store/src/store.rs:825, crates/extent_store/src/store.rs:839.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-29.2, TASK-26.18.4, TASK-44.4. No task covers close_left_open removing the framed file before rewriting it. TASK-29.2 (Done) defined recovery's reframing. TASK-26.18.4 made commits complete once ordered, so close_now can frame with a non-durable epoch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
