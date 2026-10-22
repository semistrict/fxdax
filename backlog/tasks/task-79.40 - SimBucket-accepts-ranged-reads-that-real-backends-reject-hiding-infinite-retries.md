---
id: TASK-79.40
title: >-
  SimBucket accepts ranged reads that real backends reject, hiding infinite
  retries
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 228000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sim clamps out-of-range ranges, while real stores reject them as Generic, which maps to Unavailable and is retried forever. A stale-tail read hangs in production but shows as Corrupt in the tests.

Where: crates/extent_remote/src/sim/bucket.rs:142, crates/extent_remote/src/store.rs:233, crates/extent_remote/src/bucket.rs:125, crates/extent_remote/src/fetch.rs:396.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-46, TASK-28.4, TASK-28.1, TASK-44.5. TASK-46 (Done) fixed a sibling: Azure suffix reads mapped to Unavailable and retried forever. It did not cover out-of-range ranged reads or SimBucket's clamping. No existing task.
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
