---
id: TASK-75
title: >-
  Snapshots: a zygote snapshotted before any tenant store is attached; live
  migration's pre-copy rounds
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: low
ordinal: 184000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Snapshot and restore with the extent device work once Fxfs is frozen. Show a zygote snapshotted before any tenant store is attached, and live migration's pre-copy rounds.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
