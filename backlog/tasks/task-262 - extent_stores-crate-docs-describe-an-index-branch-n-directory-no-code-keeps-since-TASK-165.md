---
id: TASK-262
title: >-
  extent_store's crate docs describe an index/<branch>/<n> directory no code
  keeps since TASK-165
status: To Do
assignee: []
created_date: '2026-11-08 14:19'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 692000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/extent_store/src/lib.rs's module docs list `index/<branch>/<n>` (a checkpoint plus deltas) among a store's files, but no code in extent_store reads or writes an index directory: since TASK-165 (doc-14) the index is the bucket's. Found by the codebase guide's brainstorm, 2026-11-08. Restate the docs as the store's files are.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The module docs list only the files a store keeps
- [ ] #2 Nothing else in the crate's docs describes the local index
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
