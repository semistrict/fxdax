---
id: TASK-79.19
title: >-
  Uploads are not bounded in memory: gather takes every closed segment at once,
  and a first attach uploads the whole store as one commit
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
ordinal: 207000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
IN_FLIGHT only gates whether gather runs, after which it reads every closed segment into memory and PUTs them all, so after a bucket outage a host holds every store's whole lag about three times over. The first attach of an imported store reads all its data into one commit and one segment PUT, which over 5 GiB is retried forever while attach blocks.

Where: crates/fxdax/src/remote.rs:34, crates/fxdax/src/remote.rs:598, crates/fxdax/src/remote.rs:813, crates/fxdax/src/remote.rs:744, crates/fxdax/src/remote.rs:849, crates/fxdax/src/remote.rs:385, crates/fxdax/src/extentd/stores.rs:377.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.5, TASK-43.6, TASK-61, TASK-50.3. No task covers gather loading every closed segment at once, or the first attach uploading the whole store as one commit. TASK-28.5 bounds lag, not memory. TASK-43.6 notes the attach upload stays on a blocking thread. TASK-61 (To Do) is upload latency.
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
Uploads now have a memory bound. A store takes at most as many closed segments at once as the host's upload budget holds (sixteen), reads each object from its file only when its PUT starts, and lets it go once it has uploaded. A first attach uploads its diff in 32 MiB batches, the last one making the commit, rather than one segment of the whole store. Verified by three tests that failed before the fix (nine own-extent creates at once, one gather taking all nine segments, one 40 MiB attach segment), which pass with the new tests on macOS and Linux (GCE), together with the workspace runs.
<!-- SECTION:FINAL_SUMMARY:END -->
