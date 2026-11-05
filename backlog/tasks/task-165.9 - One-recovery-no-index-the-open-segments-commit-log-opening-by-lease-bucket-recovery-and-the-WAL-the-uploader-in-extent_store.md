---
id: TASK-165.9
title: >-
  One recovery: no index; the open segment's commit log; opening by lease,
  bucket recovery and the WAL; the uploader in extent_store
status: To Do
assignee: []
created_date: '2026-11-05 03:17'
labels: []
dependencies:
  - TASK-165.5
  - TASK-165.8
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 631000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 9. The last of the second engine: the local index (checkpoints and deltas) and the marker, Plan, diff and adopt that reconcile it with the bucket. Afterwards a store directory is its description, its open segment with its log, and its WAL, and opening one is the same whether the branch was here, elsewhere or never anywhere.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit is durable locally when the flush has synced the open segment and appended its record to the open segment's log; index.rs and the delta codec are gone
- [ ] #2 Opening a store takes the lease, recovers the branch, closes a segment left open from its log, and rebases the WAL; a diverged WAL refuses (ESTALE) until fxdaxctl take drops it
- [ ] #3 remote.rs keeps only the host's configuration, budgets, runtime and errno mapping; the uploader is extent_store's; Marker, InFlight, Plan, diff, take, adopt, spend and snapshot are gone
- [ ] #4 The crash rounds recover a prefix at least as late as the last durable commit with the disk, and at least as late as the last acknowledged without it; Shuttle scenarios cover the uploader acknowledging while commits, closes and views race
- [ ] #5 The audit feature asserts at every close that the live state equals the replay of the segment's footer over the state before it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
