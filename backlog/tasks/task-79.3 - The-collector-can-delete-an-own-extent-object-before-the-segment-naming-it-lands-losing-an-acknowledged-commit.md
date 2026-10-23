---
id: TASK-79.3
title: >-
  The collector can delete an own-extent object before the segment naming it
  lands, losing an acknowledged commit
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
ordinal: 191000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
next_sequence counts writes to open extents, and close_now writes footers with the live value between commits. So segment S-1 covers an extent X that only S names. The uploader creates X's object before S's segment, and a GC pass in that gap deletes X as an orphan. S then lands, COMMIT_DURABLE returns, and a host taking the branch over gets Error::Lost reading X.

Where: crates/extent_store/src/store.rs:1480, crates/extent_store/src/store.rs:2168, crates/extent_store/src/store.rs:2232, crates/extent_remote/src/collector.rs:263, crates/extent_remote/src/collector.rs:317, crates/extent_remote/src/writer.rs:356.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.10, TASK-28.1, TASK-44.5, TASK-29.2, TASK-44.1. No task covers footers taking the live next_sequence between commits, which lets GC delete an own-extent object as an orphan. Related: collector and sim (TASK-28.10, TASK-28.1, Done), the Footing that close_now uses (TASK-29.2), the planned TLA+ of doc-7 (TASK-44.5, To Do). TASK-44.1 lists orphan()'s epoch arithmetic as a surviving mutant.
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
The collector can no longer delete an own-extent object before the segment that names it lands. Such an object's key now names its segment, and the collector deletes an unnamed one only once that segment is in the branch, so it no longer relies on the live next_sequence that footers record between commits. Verified by a collector test that failed before the fix with the object deleted ahead of its segment, which now passes on macOS and Linux (GCE), together with the workspace runs on both.
<!-- SECTION:FINAL_SUMMARY:END -->
