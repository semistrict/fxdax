---
id: TASK-149.10
title: Concurrent VFS calls on the model under Shuttle
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 511000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Ranked low on evidence: Shuttle reaches only Rust, about 10% of recent fixes, and the model found the super-block race without it. follower_sim already runs real Fxfs under Shuttle, and fuchsia-async and fuchsia-sync have its features. Worth doing to protect the core. Timers don't fire under it, so timed work must become test operations.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The model runs under Shuttle with a writer and a compaction task
- [ ] #2 The same seed gives the same events, by hash
- [ ] #3 Reintroducing the super-block offsets bug is found by the search
- [ ] #4 Several writers run, each with its own files
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
