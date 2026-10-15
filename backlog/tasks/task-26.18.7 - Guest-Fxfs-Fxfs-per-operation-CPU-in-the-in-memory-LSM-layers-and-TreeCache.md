---
id: TASK-26.18.7
title: 'Guest Fxfs: Fxfs per-operation CPU in the in-memory LSM layers and TreeCache'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 61000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Natively (fxfs-metabench with perf on GCE), after 089d6fc an unlink still spends about 17% in skip-list searches, 10% in TreeCache invalidation and lookup (LinkedHashMap, value clones), and compaction takes about a quarter of the CPU while it runs. Each replace_or_insert erases and reallocates a node. Shrinking these moves every metadata operation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-metabench on GCE shows unlink + graveyard under 12 us per op (15.4 us at 089d6fc)
- [ ] #2 fxfs unit tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
