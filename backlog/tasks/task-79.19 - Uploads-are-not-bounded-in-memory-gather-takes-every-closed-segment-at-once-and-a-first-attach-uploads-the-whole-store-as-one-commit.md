---
id: TASK-79.19
title: >-
  Uploads are not bounded in memory: gather takes every closed segment at once,
  and a first attach uploads the whole store as one commit
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
