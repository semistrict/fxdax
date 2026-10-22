---
id: TASK-79.1
title: >-
  A FREE racing an in-flight WRITE panics extentd while it holds the store lock,
  and drains the shared pool
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
priority: high
type: bug
ordinal: 189000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Store::free removes the open placement and entry without checking for writes in flight. finish_write re-creates the entry and hits unreachable! with the state mutex held when the placements disagree (Segment vs Own). The poisoned mutex makes every later request on the store panic, and each panic kills a pool thread without decrementing threads, so the shared pool ends at its cap with no live threads and every tenant hangs. When the placements agree, the extent is never sealed: commits fail, or the store never opens again.

Where: crates/extent_store/src/store.rs:1643, crates/extent_store/src/store.rs:1448, crates/extent_store/src/store.rs:1478, crates/extent_store/src/store.rs:1022, crates/fxdax/src/extentd/pool.rs:68, crates/fxdax/src/extentd/server.rs:384.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.4, TASK-34. No task covers FREE racing an in-flight WRITE, finish_write's unreachable!, or pool threads lost to panics. TASK-44.4 (Done) runs the store under Shuttle (writer, closer, uploader) but has no free-vs-write race. TASK-34 (Done) built the shared pool (pool.rs).
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
