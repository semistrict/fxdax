---
id: TASK-149.5
title: 'Every crash of a short sequence, not a sample'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 09:29'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 506000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Random cuts took minutes to reach several of the 2026-10-31 bugs. For short sequences every barrier and every subset of lost writes can be tried, which is complete up to a bound, as ALICE and CrashMonkey do.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 For sequences up to a bound, every barrier and every subset of the writes since the last, up to a limit, is replayed and checked
- [ ] #2 Reintroducing the super-block offsets bug fails
- [ ] #3 The bounded run takes under a few minutes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Crashes::every: at each barrier, every subset of what it may lose (a free, or an extent's writes), up to a bound, and past it nothing, all, each alone, all but each, counted.
2. Short sequences in the model test, with compactions on their own and journal extensions from a small reclaim size and chunk, and fills racing a compaction, with a store flushing.
3. Show the super-block offsets bug, put back, fails. Time the bounded run.
<!-- SECTION:PLAN:END -->
