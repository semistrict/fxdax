---
id: TASK-42.4
title: >-
  Fxfs follower: apply the writer's commits to a mounted read-only Fxfs, with
  change sets and a follower graveyard
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:17'
labels: []
dependencies:
  - TASK-42.1
  - TASK-42.2
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 124000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The follower itself (doc-8, The follower in Fxfs): after a read-only mount on a view, it applies each later transaction to the live trees as the writer's own commits apply theirs, and says what changed so the kernel can invalidate. Built and tested in the simulation (TASK-42.1) before any guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A follower applies flushes (EndFlush), journal moves to new extents (a new super-block), deletions (graveyard, tombstones) and per-object cached state, shown by the per-batch comparison over many seeds
- [ ] #2 Each batch yields a change set: attributes changed, directory entries changed, data ranges changed, objects deleted
- [ ] #3 A file deleted while a follower's reader has it open reads to its end until closed (the follower's graveyard pins its extents), then its pins go
- [ ] #4 The simulation and history checker pass over SIM_SOAK seeds with mutation checks for each rule
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
