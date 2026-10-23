---
id: TASK-79.43
title: A MAPPINGS reply over MAX_PAYLOAD tears down the connection and kills the VM
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 231000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
MAPPINGS sends every window mapping in one unbounded reply. Over 64 MiB, the client treats the reply as the daemon going and exits, so a window with more than about 1.4M mappings cannot be snapshotted.

Where: crates/fxdax/src/extentd/client.rs:382, crates/fxdax/src/extentd/wire.rs:344, crates/fxdax/src/extentd/client.rs:584, crates/fxdax/src/extentd/server.rs:530.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-39.1. TASK-39.1 (Done) added the MAPPINGS op for snapshots. No task covers the unbounded reply over MAX_PAYLOAD.
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
