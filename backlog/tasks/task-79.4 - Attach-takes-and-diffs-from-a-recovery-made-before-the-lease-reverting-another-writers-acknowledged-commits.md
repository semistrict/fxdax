---
id: TASK-79.4
title: >-
  Attach takes and diffs from a recovery made before the lease, reverting
  another writer's acknowledged commits
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 192000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
start() decides ownership, and takes, from a recovery made before Writer::open takes the lease. The diff then runs against the newer post-lease state without re-checking its last commit. If the old writer acknowledges T6 in that window, B frees T6's extents and puts the root back to T5, or fails with RemoteData after it has already fenced A.

Where: crates/fxdax/src/remote.rs:709, crates/fxdax/src/remote.rs:712, crates/fxdax/src/remote.rs:731, crates/fxdax/src/remote.rs:742, crates/fxdax/src/remote.rs:895.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.7, TASK-39.7, TASK-28.1. No task covers attach deciding the take or diff from a recovery made before Writer::open's lease. Leases, takeover and fencing were TASK-28.7 and TASK-39.7 (Done). TASK-28.1's sim has the 'check the lease after recovering' rule, but remote.rs start() is not checked against it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Attaching now decides whether to take or diff from the state recovered after the lease, not from one recovered before it. A store whose branch moved on in between refuses, and the other writer's acknowledged commits stay in the branch. Verified by two tests that hold the attaching store's lease write while another writer commits: before the fix one failed reading a freed extent and the other reverted the commit, and both now pass on macOS and Linux (GCE), together with the workspace runs.
<!-- SECTION:FINAL_SUMMARY:END -->
