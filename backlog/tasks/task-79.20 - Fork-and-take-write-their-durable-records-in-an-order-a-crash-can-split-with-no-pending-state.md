---
id: TASK-79.20
title: >-
  Fork and take write their durable records in an order a crash can split, with
  no pending state
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
priority: medium
type: bug
ordinal: 208000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A crash after a fork's segment lands but before its record or marker: if the child opens first it gets a root record and the parent then fails Corrupt on every open; if the parent opens first, the child is refused forever. A crash between take's durable commit and its marker leaves a store that refuses its branch. A crash between fork_host's branch record and its handle leaves a branch nothing names, which holds its inherited data forever.

Where: crates/fxdax/src/extents.rs:854, crates/fxdax/src/remote.rs:709, crates/fxdax/src/remote.rs:899, crates/extent_remote/src/branch.rs:30, crates/extent_remote/src/writer.rs:187, crates/extent_store/src/host.rs:78, crates/extent_store/src/host.rs:124.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.7, TASK-28.2, TASK-44.4, TASK-39.3. No task covers crash splits between a fork's segment and its record or marker, take's commit and its marker, or fork_host's record and its handle. TASK-44.4 (Done) fixed a different crash: a torn branch record during a fork.
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
A crash during a fork or a take no longer leaves state that makes the branch refuse forever. A fork's handle is written whole before its branch, a store whose marker was lost claims its branch's last commit with an empty diff when it holds what the branch holds, and a fork's child with no record takes the record that its parent's state names. Deleting a handle that a crash left without its description now removes it through extentd's and fxdaxctl's path too. Verified by tests of each crash split, all failing before the fix and passing on macOS and Linux (GCE), together with the workspace runs. The one narrower race left is TASK-79.72.
<!-- SECTION:FINAL_SUMMARY:END -->
